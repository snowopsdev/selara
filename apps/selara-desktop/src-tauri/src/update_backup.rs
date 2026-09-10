//! Independent backup around the updater plugin's non-transactional app move.
//! Never uses an elevated installer: a failed update must be restorable by the
//! same user. Backups survive any error that prevents verified restoration.
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct Backup {
    _installation_lock: InstallationLock,
    app: PathBuf,
    directory: PathBuf,
    saved: PathBuf,
    old_version: String,
}

struct InstallationLock(std::fs::File);

impl Drop for InstallationLock {
    fn drop(&mut self) {
        // Closing our descriptor alone can leave flock held by a concurrent
        // fork until that child execs. Only this guard owns the transaction;
        // explicitly release it, including when backup preparation fails.
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
        }
    }
}

#[derive(Debug)]
pub struct InstallFailure {
    pub message: String,
    pub restored: bool,
}

trait BundleOps {
    fn copy(&self, from: &Path, to: &Path) -> Result<(), String>;
    fn verify(&self, app: &Path, version: &str) -> Result<(), String>;
    fn move_bundle(&self, from: &Path, to: &Path) -> Result<(), String> {
        std::fs::rename(from, to).map_err(|e| e.to_string())
    }
}

struct MacBundles;

fn run(program: &str, args: &[&std::ffi::OsStr]) -> Result<String, String> {
    let result = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if !result.status.success() {
        return Err(format!(
            "{program} failed: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&result.stdout).trim().to_string())
}

impl BundleOps for MacBundles {
    fn copy(&self, from: &Path, to: &Path) -> Result<(), String> {
        run("/usr/bin/ditto", &[from.as_os_str(), to.as_os_str()]).map(|_| ())
    }
    fn verify(&self, app: &Path, version: &str) -> Result<(), String> {
        run(
            "/usr/bin/codesign",
            &[
                "--verify".as_ref(),
                "--deep".as_ref(),
                "--strict".as_ref(),
                app.as_os_str(),
            ],
        )?;
        let info = app.join("Contents/Info.plist");
        let identifier = run(
            "/usr/libexec/PlistBuddy",
            &[
                "-c".as_ref(),
                "Print :CFBundleIdentifier".as_ref(),
                info.as_os_str(),
            ],
        )?;
        let actual = run(
            "/usr/libexec/PlistBuddy",
            &[
                "-c".as_ref(),
                "Print :CFBundleShortVersionString".as_ref(),
                info.as_os_str(),
            ],
        )?;
        if identifier != "dev.snowops.selara" || actual != version {
            return Err("Update bundle has an unexpected identifier or version".into());
        }
        Ok(())
    }
}

impl Backup {
    /// The caller supplies bytes already verified by Tauri's updater key.
    /// Extract on the app's volume and use ordinary renames only. The plugin's
    /// macOS installer can request administrator privileges after a failed
    /// rename, which cannot satisfy our same-user restoration guarantee.
    pub fn install_archive(&self, version: &str, bytes: &[u8]) -> Result<(), InstallFailure> {
        let staged = self.directory.join("staged");
        let prepare = (|| {
            extract_archive(bytes, &staged)?;
            MacBundles.verify(&staged.join("Selara.app"), version)
        })();
        if let Err(message) = prepare {
            return Err(InstallFailure {
                message: format!(
                    "Update archive validation failed: {message}. The installed app is unchanged."
                ),
                restored: true,
            });
        }
        self.install(version, || {
            std::fs::rename(&self.app, self.directory.join("replaced.app"))
                .map_err(|e| e.to_string())?;
            std::fs::rename(staged.join("Selara.app"), &self.app).map_err(|e| e.to_string())
        })
    }
    pub fn prepare(app: &Path, version: &str) -> Result<Self, String> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let metadata = std::fs::metadata(app).map_err(|e| e.to_string())?;
            if metadata.uid() != unsafe { libc::geteuid() } {
                return Err("This installation requires administrator access. Install the notarized DMG from GitHub instead.".into());
            }
        }
        Self::prepare_with(app, version, &MacBundles)
    }

    fn prepare_with(app: &Path, version: &str, ops: &impl BundleOps) -> Result<Self, String> {
        let app = app.canonicalize().map_err(|e| e.to_string())?;
        if app.extension().and_then(|s| s.to_str()) != Some("app") || !app.is_dir() {
            return Err(
                "In-app updates require an installed app bundle. Download the DMG from GitHub."
                    .into(),
            );
        }
        let parent = app.parent().ok_or("App has no installation directory")?;
        let lock_path = parent.join(format!(
            ".{}.update.lock",
            app.file_name().unwrap().to_string_lossy()
        ));
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let installation_lock = options
            .open(lock_path)
            .map_err(|e| format!("Cannot lock this installation: {e}. Use the DMG instead."))?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            if unsafe { libc::flock(installation_lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) }
                != 0
            {
                return Err(
                    "Another Selara process is updating this installation. Wait for it to finish."
                        .into(),
                );
            }
        }
        let installation_lock = InstallationLock(installation_lock);
        ops.verify(&app, version)?;
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        let directory = parent.join(format!(
            ".selara-update-backup-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).map_err(|_| "Selara cannot safely restore this installation. Install the DMG from GitHub instead.".to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| e.to_string())?;
        }
        let saved = directory.join("Selara.app");
        if let Err(error) = ops
            .copy(&app, &saved)
            .and_then(|_| ops.verify(&saved, version))
        {
            let _ = std::fs::remove_dir_all(&directory);
            return Err(format!("Could not verify the update backup: {error}"));
        }
        Ok(Self {
            _installation_lock: installation_lock,
            app,
            directory,
            saved,
            old_version: version.into(),
        })
    }

    pub fn install(
        &self,
        version: &str,
        install: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), InstallFailure> {
        self.install_with(version, install, &MacBundles)
    }

    fn install_with(
        &self,
        version: &str,
        install: impl FnOnce() -> Result<(), String>,
        ops: &impl BundleOps,
    ) -> Result<(), InstallFailure> {
        let outcome = install().and_then(|_| ops.verify(&self.app, version));
        if let Err(error) = outcome {
            let restore = (|| {
                // Keep a failed replacement for diagnosis; never delete the only
                // good copy, even when restoration itself fails.
                if self.app.exists() {
                    ops.move_bundle(&self.app, &self.directory.join("failed.app"))?;
                }
                ops.copy(&self.saved, &self.app)?;
                ops.verify(&self.app, &self.old_version)
            })();
            return Err(match restore {
                Ok(()) => InstallFailure { message: format!("Update failed and the previous app was restored: {error}"), restored: true },
                Err(restore_error) => InstallFailure { message: format!("Update failed: {error}. Restoration failed: {restore_error}. Your verified backup remains at {}. Install the DMG to recover.", self.saved.display()), restored: false },
            });
        }
        let _ = std::fs::remove_dir_all(&self.directory);
        Ok(())
    }
}

fn extract_archive(bytes: &[u8], destination: &Path) -> Result<(), String> {
    use std::path::Component;
    std::fs::create_dir(destination).map_err(|e| e.to_string())?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
    let mut expanded = 0u64;
    for (count, entry) in archive.entries().map_err(|e| e.to_string())?.enumerate() {
        if count > 100_000 {
            return Err("Too many update archive entries".into());
        }
        let mut entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path().map_err(|e| e.to_string())?.into_owned();
        if path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
            || !path.starts_with("Selara.app")
        {
            return Err(format!(
                "Update archive contains an unexpected path: {}",
                path.display()
            ));
        }
        let kind = entry.header().entry_type();
        if !(kind.is_file() || kind.is_dir() || kind.is_symlink()) {
            return Err("Unsupported update archive entry".into());
        }
        if kind.is_symlink() {
            let link = entry
                .link_name()
                .map_err(|e| e.to_string())?
                .ok_or("Missing symlink target")?;
            let mut resolved = path.parent().ok_or("Invalid symlink")?.to_path_buf();
            for component in link.components() {
                match component {
                    Component::Normal(part) => resolved.push(part),
                    Component::CurDir => {}
                    Component::ParentDir if resolved.components().count() > 1 => {
                        resolved.pop();
                    }
                    _ => return Err("Update symlink escapes the app".into()),
                }
            }
        }
        expanded = expanded
            .checked_add(entry.size())
            .ok_or("Update archive is too large")?;
        if expanded > 4 * 1024 * 1024 * 1024 {
            return Err("Update archive is too large".into());
        }
        if !entry.unpack_in(destination).map_err(|e| e.to_string())? {
            return Err("Update archive escaped its staging directory".into());
        }
    }
    Ok(())
}

/// Resolve the bundle enclosing the running binary rather than assuming it
/// was installed in /Applications or replacing a different copy of Selara.
pub fn running_app() -> Result<PathBuf, String> {
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    executable
        .ancestors()
        .find(|p| p.extension().and_then(|s| s.to_str()) == Some("app"))
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            "Run an installed Selara.app to update; development binaries cannot install updates."
                .into()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    struct FixtureOps {
        fail_restore: bool,
    }
    impl BundleOps for FixtureOps {
        fn copy(&self, from: &Path, to: &Path) -> Result<(), String> {
            if self.fail_restore
                && from
                    .parent()
                    .unwrap()
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(".selara-update-backup")
            {
                return Err("injected restore failure".into());
            }
            std::fs::create_dir_all(to).map_err(|e| e.to_string())?;
            std::fs::copy(from.join("version"), to.join("version")).map_err(|e| e.to_string())?;
            Ok(())
        }
        fn verify(&self, app: &Path, version: &str) -> Result<(), String> {
            if std::fs::read_to_string(app.join("version")).map_err(|e| e.to_string())? != version {
                return Err("wrong version".into());
            }
            Ok(())
        }
    }
    fn fixture() -> (PathBuf, PathBuf) {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "selara-backup-test-{}-{unique}-{sequence}",
            std::process::id()
        ));
        let app = root.join("Selara.app");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(app.join("version"), "1.0.0").unwrap();
        (root, app)
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn installs_a_different_real_signed_macos_bundle_from_an_archive() {
        let (root, app) = fixture();
        std::fs::remove_dir_all(&app).unwrap();
        fn bundle(path: &Path, version: &str) {
            std::fs::create_dir_all(path.join("Contents/MacOS")).unwrap();
            std::fs::copy("/usr/bin/true", path.join("Contents/MacOS/fixture")).unwrap();
            std::fs::write(path.join("Contents/Info.plist"), format!("<?xml version=\"1.0\"?><plist version=\"1.0\"><dict><key>CFBundleIdentifier</key><string>dev.snowops.selara</string><key>CFBundleExecutable</key><string>fixture</string><key>CFBundleShortVersionString</key><string>{version}</string><key>CFBundlePackageType</key><string>APPL</string></dict></plist>")).unwrap();
            run(
                "/usr/bin/codesign",
                &[
                    "--force".as_ref(),
                    "--sign".as_ref(),
                    "-".as_ref(),
                    path.as_os_str(),
                ],
            )
            .unwrap();
        }
        bundle(&app, "1.0.0");
        let incoming = root.join("incoming");
        bundle(&incoming.join("Selara.app"), "1.0.1");
        let archive = root.join("update.tar.gz");
        run(
            "/usr/bin/tar",
            &[
                "--disable-copyfile".as_ref(),
                "-czf".as_ref(),
                archive.as_os_str(),
                "-C".as_ref(),
                incoming.as_os_str(),
                "Selara.app".as_ref(),
            ],
        )
        .unwrap();
        let backup = Backup::prepare(&app, "1.0.0").unwrap();
        backup
            .install_archive("1.0.1", &std::fs::read(archive).unwrap())
            .unwrap();
        MacBundles.verify(&app, "1.0.1").unwrap();
        assert!(!backup.directory.exists());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn concurrent_installations_cannot_prepare_overlapping_backups() {
        let (root, app) = fixture();
        let ops = FixtureOps {
            fail_restore: false,
        };
        let first = Backup::prepare_with(&app, "1.0.0", &ops).unwrap();
        assert!(Backup::prepare_with(&app, "1.0.0", &ops)
            .err()
            .unwrap()
            .contains("Another Selara process"));
        // Model a descriptor briefly inherited by a concurrent fork before
        // close-on-exec runs. It must not retain ownership after our guard ends.
        let inherited = first._installation_lock.0.try_clone().unwrap();
        drop(first);
        Backup::prepare_with(&app, "1.0.0", &ops)
            .expect("the installation lock must be released when its owner finishes");
        drop(inherited);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn archive_rejects_outside_files_and_escaping_symlinks() {
        fn archive(path: &str, link: Option<&str>) -> Vec<u8> {
            let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            let mut tar = tar::Builder::new(encoder);
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o644);
            header.set_size(0);
            if let Some(link) = link {
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_link_name(link).unwrap();
            }
            header.set_cksum();
            tar.append_data(&mut header, path, std::io::empty())
                .unwrap();
            tar.into_inner().unwrap().finish().unwrap()
        }
        let (root, _) = fixture();
        assert!(extract_archive(&archive("other.app/file", None), &root.join("one")).is_err());
        assert!(extract_archive(
            &archive("Selara.app/escape", Some("../outside")),
            &root.join("two")
        )
        .is_err());
        assert!(extract_archive(
            &archive("Selara.app/Contents/relative", Some("../file")),
            &root.join("three")
        )
        .is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn restores_when_installer_loses_old_app_before_failed_replacement() {
        let (root, app) = fixture();
        let ops = FixtureOps {
            fail_restore: false,
        };
        let backup = Backup::prepare_with(&app, "1.0.0", &ops).unwrap();
        let failure = backup
            .install_with(
                "1.0.1",
                || {
                    std::fs::remove_dir_all(&app).unwrap();
                    Err("replacement failed".into())
                },
                &ops,
            )
            .unwrap_err();
        assert!(failure.restored);
        ops.verify(&app, "1.0.0").unwrap();
        assert!(backup.saved.exists());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn restores_after_post_install_validation_failure() {
        let (root, app) = fixture();
        let ops = FixtureOps {
            fail_restore: false,
        };
        let backup = Backup::prepare_with(&app, "1.0.0", &ops).unwrap();
        let result = backup
            .install_with(
                "1.0.1",
                || {
                    std::fs::write(app.join("version"), "wrong").unwrap();
                    Ok(())
                },
                &ops,
            )
            .unwrap_err();
        assert!(result.restored);
        ops.verify(&app, "1.0.0").unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn retains_backup_when_restore_fails_and_cleans_it_only_after_verified_success() {
        let (root, app) = fixture();
        let backup = Backup::prepare_with(
            &app,
            "1.0.0",
            &FixtureOps {
                fail_restore: false,
            },
        )
        .unwrap();
        let result = backup
            .install_with(
                "1.0.1",
                || Err("failure".into()),
                &FixtureOps { fail_restore: true },
            )
            .unwrap_err();
        assert!(!result.restored);
        assert!(backup.saved.exists());
        std::fs::remove_dir_all(root).unwrap();
        let (root, app) = fixture();
        let ops = FixtureOps {
            fail_restore: false,
        };
        let backup = Backup::prepare_with(&app, "1.0.0", &ops).unwrap();
        backup
            .install_with(
                "1.0.1",
                || {
                    std::fs::write(app.join("version"), "1.0.1").unwrap();
                    Ok(())
                },
                &ops,
            )
            .unwrap();
        assert!(!backup.directory.exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
