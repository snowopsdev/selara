//! Installed and running apps for the per-app lists in Settings (command
//! `apps` and `excluded_apps`): their icons, the apps open right now, and a
//! native picker. AppKit does the lookups on macOS; elsewhere every command
//! reports nothing so the UI falls back to monogram chips.
// The lookup helpers only have a caller in the macOS backend.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// An app as the Settings lists store it: what to show and what to match.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AppRef {
    pub name: String,
    pub bundle_id: String,
}

/// Points the icon is shown at; rendered at 2x for Retina displays.
const ICON_POINTS: usize = 64;
const ICON_PIXELS: usize = ICON_POINTS * 2;

/// Icons already rendered, keyed by the trimmed, lowercased query. Only hits
/// are kept so an app installed later still gets its icon.
static ICON_CACHE: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

/// The query an icon can be looked up for: trimmed, with globs (`com.foo.*`,
/// `*`) refused because they name many apps and so no single icon.
pub fn icon_query(app: &str) -> Option<&str> {
    let app = app.trim();
    (!app.is_empty() && !app.contains('*')).then_some(app)
}

/// Reverse-DNS shape (`com.apple.Safari`): at least two dot-separated parts
/// of letters, digits, `-`, or `_`. App names with spaces or a `.app` suffix
/// are treated as names instead.
pub fn looks_like_bundle_id(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    if lower.ends_with(".app") {
        return false;
    }
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() >= 2
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        })
}

/// Where an app called `name` is installed, most likely first. A name that
/// could escape those folders (a path separator or `..`) yields nothing.
pub fn candidate_app_paths(name: &str, home: Option<&Path>) -> Vec<PathBuf> {
    let name = name.trim();
    let stem = if name.to_ascii_lowercase().ends_with(".app") {
        &name[..name.len() - 4]
    } else {
        name
    };
    if stem.is_empty() || stem.contains('/') || stem.contains("..") {
        return Vec::new();
    }
    let bundle = format!("{stem}.app");
    let mut dirs: Vec<PathBuf> = vec![
        PathBuf::from("/Applications"),
        PathBuf::from("/Applications/Utilities"),
    ];
    if let Some(home) = home {
        dirs.push(home.join("Applications"));
    }
    dirs.extend(
        [
            "/System/Applications",
            "/System/Applications/Utilities",
            "/System/Library/CoreServices",
        ]
        .map(PathBuf::from),
    );
    dirs.into_iter().map(|d| d.join(&bundle)).collect()
}

/// `Visual Studio Code` for `/Applications/Visual Studio Code.app`.
pub fn bundle_stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Running apps as the picker lists them: no entry for `own_ids` (this app),
/// one entry per bundle id, sorted by name without regard to case.
pub fn tidy_app_list(apps: Vec<AppRef>, own_ids: &[&str]) -> Vec<AppRef> {
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<AppRef> = apps
        .into_iter()
        .filter(|a| !a.bundle_id.is_empty() && !a.name.trim().is_empty())
        .filter(|a| {
            !own_ids
                .iter()
                .any(|own| own.eq_ignore_ascii_case(&a.bundle_id))
        })
        .filter(|a| seen.insert(a.bundle_id.to_ascii_lowercase()))
        .collect();
    out.sort_by_cached_key(|a| (a.name.to_lowercase(), a.bundle_id.to_lowercase()));
    out
}

fn cached_icon(key: &str) -> Option<String> {
    ICON_CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()?
        .get(key)
        .cloned()
}

fn cache_icon(key: String, icon: String) {
    ICON_CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert(key, icon);
}

/// `data:image/png;base64,…` icon for an app name or bundle id, or `None` for
/// globs and apps that cannot be found. Renders off the main thread.
#[tauri::command]
pub async fn app_icon(app: String) -> Option<String> {
    let query = icon_query(&app)?.to_string();
    let key = query.to_lowercase();
    if let Some(hit) = cached_icon(&key) {
        return Some(hit);
    }
    let icon = tauri::async_runtime::spawn_blocking(move || platform::icon_png(&query))
        .await
        .ok()
        .flatten()?;
    use base64::Engine as _;
    let url = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(icon)
    );
    cache_icon(key, url.clone());
    Some(url)
}

/// Regular (Dock) apps running now, excluding Selara, sorted by name.
#[tauri::command]
pub fn running_apps(app: tauri::AppHandle) -> Vec<AppRef> {
    let identifier = app.config().identifier.clone();
    platform::running_apps(&identifier)
}

/// Pick an app bundle in a native open panel rooted at /Applications.
/// `None` when the panel was cancelled.
#[tauri::command]
pub async fn choose_app(app: tauri::AppHandle) -> Result<Option<AppRef>, String> {
    use tauri_plugin_dialog::DialogExt;
    tauri::async_runtime::spawn_blocking(move || {
        let Some(picked) = app
            .dialog()
            .file()
            .set_title("Choose an app")
            .set_directory("/Applications")
            .add_filter("Applications", &["app"])
            .blocking_pick_file()
        else {
            return Ok(None);
        };
        let path = picked.into_path().map_err(|e| e.to_string())?;
        platform::bundle_app_ref(&path).map(Some)
    })
    .await
    .map_err(|e| format!("app picker failed: {e}"))?
}

#[cfg(target_os = "macos")]
mod platform {
    use super::{bundle_stem, candidate_app_paths, looks_like_bundle_id, tidy_app_list, AppRef};
    use super::{ICON_PIXELS, ICON_POINTS};
    use objc2::rc::{autoreleasepool, Retained};
    use objc2::AnyThread;
    use objc2_app_kit::{
        NSApplicationActivationPolicy, NSBitmapImageFileType, NSBitmapImageRep,
        NSDeviceRGBColorSpace, NSGraphicsContext, NSImage, NSImageInterpolation, NSWorkspace,
    };
    use objc2_foundation::{NSBundle, NSDictionary, NSPoint, NSRect, NSSize, NSString};
    use std::path::{Path, PathBuf};

    /// Path of the app bundle `query` names: a bundle id through Launch
    /// Services, else a name in the usual install folders, else a running
    /// app with that name (its display name can differ from the file name).
    fn bundle_path(workspace: &NSWorkspace, query: &str) -> Option<PathBuf> {
        if looks_like_bundle_id(query) {
            let url = workspace.URLForApplicationWithBundleIdentifier(&NSString::from_str(query));
            if let Some(path) = url.and_then(|u| u.path()) {
                return Some(PathBuf::from(path.to_string()));
            }
        }
        let home = std::env::var_os("HOME").map(PathBuf::from);
        if let Some(found) = candidate_app_paths(query, home.as_deref())
            .into_iter()
            .find(|p| p.is_dir())
        {
            return Some(found);
        }
        workspace
            .runningApplications()
            .to_vec()
            .into_iter()
            .find(|app| {
                app.localizedName()
                    .is_some_and(|n| n.to_string().eq_ignore_ascii_case(query))
            })
            .and_then(|app| app.bundleURL())
            .and_then(|url| url.path())
            .map(|p| PathBuf::from(p.to_string()))
    }

    /// Draw `image` into a square RGBA bitmap and encode it as PNG.
    fn render_png(image: &NSImage) -> Option<Vec<u8>> {
        let px = ICON_PIXELS as isize;
        // SAFETY: null planes ask AppKit to allocate the pixel buffer; the
        // color space name is a framework constant.
        let rep = unsafe {
            NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
                NSBitmapImageRep::alloc(),
                std::ptr::null_mut(),
                px,
                px,
                8,
                4,
                true,
                false,
                NSDeviceRGBColorSpace,
                0,
                0,
            )
        }?;
        // Points, so the PNG carries 144 dpi and displays at ICON_POINTS.
        rep.setSize(NSSize::new(ICON_POINTS as f64, ICON_POINTS as f64));
        let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
        NSGraphicsContext::saveGraphicsState_class();
        NSGraphicsContext::setCurrentContext(Some(&context));
        context.setImageInterpolation(NSImageInterpolation::High);
        image.drawInRect(NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(ICON_POINTS as f64, ICON_POINTS as f64),
        ));
        context.flushGraphics();
        NSGraphicsContext::restoreGraphicsState_class();
        // SAFETY: an empty property dictionary is valid for every file type.
        let data = unsafe {
            rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
        }?;
        Some(data.to_vec())
    }

    pub fn icon_png(query: &str) -> Option<Vec<u8>> {
        // Called on a blocking-pool thread, which has no autorelease pool.
        autoreleasepool(|_| {
            let workspace = NSWorkspace::sharedWorkspace();
            let path = bundle_path(&workspace, query)?;
            // Some system apps sit in /Applications as symlinks (Safari), and
            // the icon of a link carries an alias arrow.
            let path = std::fs::canonicalize(&path).unwrap_or(path);
            let icon = workspace.iconForFile(&NSString::from_str(&path.to_string_lossy()));
            render_png(&icon)
        })
    }

    pub fn running_apps(identifier: &str) -> Vec<AppRef> {
        autoreleasepool(|_| {
            let own_pid = std::process::id() as i32;
            let own_bundle = NSBundle::mainBundle()
                .bundleIdentifier()
                .map(|b| b.to_string())
                .unwrap_or_default();
            let apps = NSWorkspace::sharedWorkspace()
                .runningApplications()
                .to_vec()
                .into_iter()
                .filter(|app| {
                    app.activationPolicy() == NSApplicationActivationPolicy::Regular
                        && app.processIdentifier() != own_pid
                })
                .filter_map(|app| {
                    Some(AppRef {
                        bundle_id: app.bundleIdentifier()?.to_string(),
                        name: app.localizedName()?.to_string(),
                    })
                })
                .collect();
            tidy_app_list(apps, &[identifier, own_bundle.as_str()])
        })
    }

    fn info_string(bundle: &NSBundle, key: &str) -> Option<String> {
        let value = bundle.objectForInfoDictionaryKey(&NSString::from_str(key))?;
        let text: Retained<NSString> = value.downcast().ok()?;
        let text = text.to_string();
        (!text.trim().is_empty()).then_some(text)
    }

    pub fn bundle_app_ref(path: &Path) -> Result<AppRef, String> {
        autoreleasepool(|_| {
            let bundle = NSBundle::bundleWithPath(&NSString::from_str(&path.to_string_lossy()))
                .ok_or_else(|| format!("{} is not an app bundle.", bundle_stem(path)))?;
            let bundle_id = bundle
                .bundleIdentifier()
                .map(|b| b.to_string())
                .ok_or_else(|| {
                    format!(
                        "{} has no bundle identifier, so Selara cannot match it.",
                        bundle_stem(path)
                    )
                })?;
            let name = info_string(&bundle, "CFBundleDisplayName")
                .or_else(|| info_string(&bundle, "CFBundleName"))
                .unwrap_or_else(|| bundle_stem(path));
            Ok(AppRef { name, bundle_id })
        })
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::AppRef;
    use std::path::Path;

    pub fn icon_png(_query: &str) -> Option<Vec<u8>> {
        None
    }

    pub fn running_apps(_identifier: &str) -> Vec<AppRef> {
        Vec::new()
    }

    pub fn bundle_app_ref(_path: &Path) -> Result<AppRef, String> {
        Err("Choosing an app is only available on macOS.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_and_blank_queries_have_no_icon() {
        assert_eq!(icon_query("  Safari "), Some("Safari"));
        for q in ["", "   ", "*", "com.microsoft.*", "Slack*"] {
            assert_eq!(icon_query(q), None, "{q:?}");
        }
    }

    #[test]
    fn bundle_ids_are_told_apart_from_names() {
        for id in [
            "com.apple.Safari",
            "com.tinyspeck.slackmacgap",
            "io.a_b.c-d",
        ] {
            assert!(looks_like_bundle_id(id), "{id}");
        }
        for name in [
            "Safari",
            "Visual Studio Code",
            "Safari.app",
            "com.",
            ".com",
            "a..b",
        ] {
            assert!(!looks_like_bundle_id(name), "{name}");
        }
    }

    #[test]
    fn app_names_resolve_to_the_usual_install_folders() {
        let home = Path::new("/Users/me");
        let paths = candidate_app_paths(" Safari.app ", Some(home));
        assert_eq!(paths[0], PathBuf::from("/Applications/Safari.app"));
        assert!(paths.contains(&PathBuf::from("/Users/me/Applications/Safari.app")));
        assert!(paths.contains(&PathBuf::from("/System/Applications/Safari.app")));
        assert!(paths.contains(&PathBuf::from("/System/Applications/Utilities/Safari.app")));
        assert!(candidate_app_paths("Notes", None)
            .iter()
            .all(|p| !p.starts_with("/Users")));
        for hostile in ["../../etc", "a/b", "", ".app"] {
            assert!(
                candidate_app_paths(hostile, Some(home)).is_empty(),
                "{hostile}"
            );
        }
    }

    #[test]
    fn running_list_drops_selara_duplicates_and_sorts_by_name() {
        let app = |name: &str, id: &str| AppRef {
            name: name.into(),
            bundle_id: id.into(),
        };
        let list = tidy_app_list(
            vec![
                app("zed", "dev.zed.Zed"),
                app("Safari", "com.apple.Safari"),
                app("Selara", "dev.snowops.selara"),
                app("Safari", "com.apple.safari"),
                app("Mail", "com.apple.mail"),
                app("", "com.example.blank"),
            ],
            &["dev.snowops.selara", ""],
        );
        let names: Vec<&str> = list.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["Mail", "Safari", "zed"]);
    }

    #[test]
    fn bundle_stem_drops_the_extension() {
        assert_eq!(
            bundle_stem(Path::new("/Applications/Visual Studio Code.app")),
            "Visual Studio Code"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn finder_icon_renders_as_a_png() {
        // Finder is present on every Mac, by bundle id and by name.
        for query in ["com.apple.finder", "Finder"] {
            let png = platform::icon_png(query).unwrap_or_else(|| panic!("{query}"));
            assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"), "{query}");
            // IHDR width and height, big-endian, right after the signature.
            let width = u32::from_be_bytes(png[16..20].try_into().unwrap());
            let height = u32::from_be_bytes(png[20..24].try_into().unwrap());
            assert_eq!((width, height), (ICON_PIXELS as u32, ICON_PIXELS as u32));
        }
        assert!(platform::icon_png("com.example.not-installed-anywhere").is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn finder_bundle_reads_its_identifier() {
        let finder =
            platform::bundle_app_ref(Path::new("/System/Library/CoreServices/Finder.app")).unwrap();
        assert_eq!(finder.bundle_id, "com.apple.finder");
        assert!(!finder.name.is_empty());
        assert!(platform::bundle_app_ref(Path::new("/nonexistent/Nope.app")).is_err());
    }
}
