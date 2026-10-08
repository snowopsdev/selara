//! Copy and save the PNG the Usage page renders for sharing. The webview
//! draws the image; this side only validates it and hands it to the system.

use base64::Engine as _;
use std::path::PathBuf;

/// Generous for a 1080×1350 card, small enough that a bad caller cannot make
/// the app buffer something absurd.
const MAX_PNG_BYTES: usize = 32 * 1024 * 1024;
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
const DEFAULT_NAME: &str = "Selara.png";

/// Decode the base64 payload (a `data:image/png;base64,` prefix is allowed,
/// since that is what `canvas.toDataURL` produces) and check it is a PNG.
pub fn decode_png(png_base64: &str) -> Result<Vec<u8>, String> {
    let payload = png_base64.trim();
    let payload = payload
        .strip_prefix("data:image/png;base64,")
        .unwrap_or(payload);
    if payload.len() / 4 * 3 > MAX_PNG_BYTES {
        return Err("The image is too large to copy or save.".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .map_err(|_| "The image data is not valid base64.".to_string())?;
    if !bytes.starts_with(PNG_SIGNATURE) {
        return Err("The image data is not a PNG.".into());
    }
    Ok(bytes)
}

/// A file name safe to offer in the save panel: no path separators or
/// control characters, not hidden, and ending in `.png`.
pub fn png_file_name(suggested: &str) -> String {
    let cleaned: String = suggested
        .trim()
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' => '-',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let cleaned = cleaned.trim();
    let stem = if cleaned.to_ascii_lowercase().ends_with(".png") {
        &cleaned[..cleaned.len() - 4]
    } else {
        cleaned
    };
    let stem = stem.trim().trim_start_matches('.').trim();
    if stem.is_empty() {
        DEFAULT_NAME.into()
    } else {
        format!("{stem}.png")
    }
}

/// The save panel's filter normally adds the extension; keep it if not.
fn with_png_extension(path: PathBuf) -> PathBuf {
    let is_png = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("png"));
    if is_png {
        path
    } else {
        let mut name = path.into_os_string();
        name.push(".png");
        PathBuf::from(name)
    }
}

/// Put the PNG on the general pasteboard. Arguments keep the snake_case keys
/// of the implementation contract (`png_base64`), not Tauri's camelCase.
#[tauri::command(rename_all = "snake_case")]
pub fn copy_png(png_base64: String) -> Result<(), String> {
    let bytes = decode_png(&png_base64)?;
    platform::copy_png(&bytes)
}

/// Save the PNG where the user picks in a native save panel. Returns the
/// saved path, or `None` when the panel was cancelled. Arguments are
/// snake_case (`png_base64`, `suggested_name`), as for [`copy_png`].
#[tauri::command(rename_all = "snake_case")]
pub async fn save_png(
    app: tauri::AppHandle,
    png_base64: String,
    suggested_name: String,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let bytes = decode_png(&png_base64)?;
    let file_name = png_file_name(&suggested_name);
    tauri::async_runtime::spawn_blocking(move || {
        let Some(picked) = app
            .dialog()
            .file()
            .set_title("Save Image")
            .set_file_name(file_name)
            .add_filter("PNG image", &["png"])
            .blocking_save_file()
        else {
            return Ok(None);
        };
        let path = with_png_extension(picked.into_path().map_err(|e| e.to_string())?);
        std::fs::write(&path, bytes).map_err(|e| format!("Could not save the image: {e}"))?;
        Ok(Some(path.display().to_string()))
    })
    .await
    .map_err(|e| format!("save task failed: {e}"))?
}

#[cfg(target_os = "macos")]
mod platform {
    use objc2::rc::autoreleasepool;
    use objc2::AnyThread;
    use objc2_app_kit::{NSImage, NSPasteboard, NSPasteboardTypePNG, NSPasteboardTypeTIFF};
    use objc2_foundation::NSData;

    /// PNG for apps that read it, plus TIFF: many (Keynote, Mail, older
    /// Cocoa apps) only paste the TIFF flavor AppKit has always provided.
    pub fn copy_png(bytes: &[u8]) -> Result<(), String> {
        autoreleasepool(|_| {
            let png = NSData::with_bytes(bytes);
            let tiff = NSImage::initWithData(NSImage::alloc(), &png)
                .and_then(|image| image.TIFFRepresentation())
                .ok_or("The image could not be read.")?;
            let pasteboard = NSPasteboard::generalPasteboard();
            pasteboard.clearContents();
            // SAFETY: framework-provided pasteboard type constants.
            let (png_type, tiff_type) = unsafe { (NSPasteboardTypePNG, NSPasteboardTypeTIFF) };
            if pasteboard.setData_forType(Some(&png), png_type)
                && pasteboard.setData_forType(Some(&tiff), tiff_type)
            {
                Ok(())
            } else {
                Err("Could not put the image on the clipboard.".into())
            }
        })
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    pub fn copy_png(_bytes: &[u8]) -> Result<(), String> {
        Err("Copying images is only available on macOS.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A valid 1×1 transparent PNG.
    const PIXEL: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

    #[test]
    fn decode_accepts_plain_and_data_url_png() {
        let plain = decode_png(PIXEL).unwrap();
        assert!(plain.starts_with(PNG_SIGNATURE));
        let url = decode_png(&format!(" data:image/png;base64,{PIXEL}\n")).unwrap();
        assert_eq!(plain, url);
    }

    #[test]
    fn decode_rejects_garbage_and_other_formats() {
        assert!(decode_png("not base64!").unwrap_err().contains("base64"));
        let jpeg = base64::engine::general_purpose::STANDARD.encode(b"\xff\xd8\xff\xe0JFIF");
        assert!(decode_png(&jpeg).unwrap_err().contains("not a PNG"));
        assert!(decode_png("").unwrap_err().contains("not a PNG"));
        let huge = "A".repeat(MAX_PNG_BYTES / 3 * 4 + 8);
        assert!(decode_png(&huge).unwrap_err().contains("too large"));
    }

    #[test]
    fn file_names_are_safe_and_end_in_png() {
        assert_eq!(png_file_name("Selara Usage"), "Selara Usage.png");
        assert_eq!(png_file_name("receipt.PNG"), "receipt.png");
        assert_eq!(png_file_name("../../etc/passwd"), "-..-etc-passwd.png");
        assert_eq!(png_file_name("a\u{0}b:c"), "a b-c.png");
        assert_eq!(png_file_name(".hidden"), "hidden.png");
        for blank in ["", "   ", ".png", "..."] {
            assert_eq!(png_file_name(blank), DEFAULT_NAME, "{blank:?}");
        }
    }

    #[test]
    fn saved_paths_keep_or_gain_the_extension() {
        assert_eq!(
            with_png_extension(PathBuf::from("/tmp/a.png")),
            PathBuf::from("/tmp/a.png")
        );
        assert_eq!(
            with_png_extension(PathBuf::from("/tmp/a.Png")),
            PathBuf::from("/tmp/a.Png")
        );
        assert_eq!(
            with_png_extension(PathBuf::from("/tmp/a")),
            PathBuf::from("/tmp/a.png")
        );
    }
}
