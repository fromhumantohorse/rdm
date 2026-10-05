//! Registering the connector with the browsers: its manifest (which extension may start it) where
//! each browser family looks for it, pointing at the installed RDM.

use std::{io, path::Path};

use super::{CHROME_ORIGIN, FIREFOX_ID, HOST};

/// The host manifest: Chromium-based browsers list allowed origins, Firefox allowed extension IDs.
fn manifest(exe: &Path, firefox: bool) -> Vec<u8> {
    let mut m = serde_json::json!({
        "name": HOST,
        "description": "RDM — Rust Download Manager",
        "path": exe,
        "type": "stdio",
    });
    if firefox {
        m["allowed_extensions"] = serde_json::json!([FIREFOX_ID]);
    } else {
        m["allowed_origins"] = serde_json::json!([CHROME_ORIGIN]);
    }
    serde_json::to_vec_pretty(&m).unwrap_or_default()
}

/// Writes `bytes` to `path` unless it already holds them.
fn write_if_changed(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if std::fs::read(path).is_ok_and(|old| old == bytes) {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = crate::settings::with_suffix(path, ".tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(tmp, path)
}

/// At start (in the background): registers the connector with the browsers, pointing at the
/// installed RDM (or this copy when there is none). Cheap when nothing changed (compared first).
pub fn register() {
    let installed = crate::update::installed_exe().filter(|p| p.is_file());
    let Some(exe) = installed.or_else(|| std::env::current_exe().ok()) else { return };
    let _ = imp::register(&exe);
}

#[cfg(windows)]
mod imp {
    use std::{io, path::Path};

    use winreg::{RegKey, enums::HKEY_CURRENT_USER};

    use super::{HOST, manifest, write_if_changed};

    /// Where each browser family looks for hosts (per user), and whether to register there even
    /// before the browser's own key exists: Chrome's and Mozilla's are read by most derivatives
    /// (Opera, Waterfox…); the others only when that browser is used.
    const CHROMIUM_KEYS: [(&str, bool); 5] = [
        (r"Software\Google\Chrome", true),
        (r"Software\Chromium", false),
        (r"Software\Microsoft\Edge", false),
        (r"Software\BraveSoftware\Brave-Browser", false),
        (r"Software\Vivaldi", false),
    ];
    const FIREFOX_KEYS: [(&str, bool); 3] = [(r"Software\Mozilla", true), (r"Software\Waterfox", false), (r"Software\LibreWolf", false)];

    pub fn register(exe: &Path) -> io::Result<()> {
        let dir = crate::settings::config_file("native");
        let chrome = dir.join(format!("{HOST}.chromium.json"));
        let firefox = dir.join(format!("{HOST}.json"));
        write_if_changed(&chrome, &manifest(exe, false))?;
        write_if_changed(&firefox, &manifest(exe, true))?;
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        for (keys, file) in [(&CHROMIUM_KEYS[..], &chrome), (&FIREFOX_KEYS[..], &firefox)] {
            let value = file.to_string_lossy().into_owned();
            for (browser, always) in keys {
                if !always && hkcu.open_subkey(browser).is_err() {
                    continue;
                }
                let path = format!(r"{browser}\NativeMessagingHosts\{HOST}");
                let current: Option<String> = hkcu.open_subkey(&path).and_then(|k| k.get_value("")).ok();
                if current.as_deref() != Some(&value) {
                    let (k, _) = hkcu.create_subkey(&path)?;
                    k.set_value("", &value)?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod imp {
    use std::{
        io,
        path::{Path, PathBuf},
    };

    use super::{HOST, manifest, write_if_changed};

    /// Each browser's per-user host folder, under the folder that shows the browser is used.
    const CHROMIUM_DIRS: [(&str, &str); 7] = [
        (".config/google-chrome", "NativeMessagingHosts"),
        (".config/chromium", "NativeMessagingHosts"),
        (".config/BraveSoftware/Brave-Browser", "NativeMessagingHosts"),
        (".config/microsoft-edge", "NativeMessagingHosts"),
        (".config/vivaldi", "NativeMessagingHosts"),
        (".config/opera", "NativeMessagingHosts"),
        (".config/google-chrome-beta", "NativeMessagingHosts"),
    ];
    const FIREFOX_DIRS: [(&str, &str); 3] =
        [(".mozilla", "native-messaging-hosts"), (".waterfox", "native-messaging-hosts"), (".librewolf", "native-messaging-hosts")];

    pub fn register(exe: &Path) -> io::Result<()> {
        let Some(home) = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf()) else { return Ok(()) };
        let mut hosts: Vec<(PathBuf, bool)> = Vec::new();
        for (dirs, firefox) in [(&CHROMIUM_DIRS[..], false), (&FIREFOX_DIRS[..], true)] {
            for (browser, dir) in dirs {
                let browser = home.join(browser);
                if browser.is_dir() {
                    hosts.push((browser.join(dir), firefox));
                }
            }
        }
        // Any other browser of either family, found by its profile folder (as IDM finds any
        // browser): no list to keep up to date.
        for (dir, firefox) in discovered(&home) {
            if !hosts.iter().any(|(h, _)| *h == dir) {
                hosts.push((dir, firefox));
            }
        }
        let (chromium, firefox) = (manifest(exe, false), manifest(exe, true));
        for (dir, is_firefox) in hosts {
            let _ = write_if_changed(&dir.join(format!("{HOST}.json")), if is_firefox { &firefox } else { &chromium });
        }
        Ok(())
    }

    /// Where the browsers of the home folder look for connectors: a Chromium-based browser keeps
    /// its profiles in `~/.config/<name>` (or one level deeper, `BraveSoftware/Brave-Browser`)
    /// with a `Local State` file, and reads `NativeMessagingHosts` there; a Firefox-based one
    /// keeps a `profiles.ini` in `~/.<name>` (or one level deeper, `.mozilla/firefox`) and reads
    /// `~/.<name>/native-messaging-hosts`.
    fn discovered(home: &Path) -> Vec<(PathBuf, bool)> {
        let subdirs = |dir: &Path| -> Vec<PathBuf> {
            std::fs::read_dir(dir).map(|e| e.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect()).unwrap_or_default()
        };
        let mut found = Vec::new();
        for dir in subdirs(&home.join(".config")) {
            for profile in std::iter::once(dir.clone()).chain(subdirs(&dir)) {
                // Electron applications (VS Code, Discord…) keep a `Local State` too, not a `Default` profile.
                if profile.join("Local State").is_file() && profile.join("Default").is_dir() {
                    found.push((profile.join("NativeMessagingHosts"), false));
                }
            }
        }
        for dir in subdirs(home).into_iter().filter(|d| d.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'))) {
            if std::iter::once(dir.clone()).chain(subdirs(&dir)).any(|d| d.join("profiles.ini").is_file()) {
                found.push((dir.join("native-messaging-hosts"), true));
            }
        }
        found
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    #[test]
    fn manifests_name_only_the_rdm_extension() {
        let chrome: Value = serde_json::from_slice(&manifest(Path::new("/opt/rdm"), false)).unwrap();
        assert_eq!(chrome["allowed_origins"][0], CHROME_ORIGIN);
        assert!(chrome.get("allowed_extensions").is_none());
        let firefox: Value = serde_json::from_slice(&manifest(Path::new("/opt/rdm"), true)).unwrap();
        assert_eq!(firefox["allowed_extensions"][0], FIREFOX_ID);
        assert_eq!(firefox["name"], HOST);
        assert_eq!(CHROME_ORIGIN.trim_end_matches('/'), crate::bridge::EXTENSION_ORIGIN);
    }
}
