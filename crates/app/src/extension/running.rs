//! Which browsers are running, by key (see `Browser::key`): from the processes of this account.

use std::collections::HashSet;

/// Executable (or folder) names → browser key. Chromium before Chrome: Chromium's own builds
/// (Flatpak included) run an executable named `chrome` in a `chromium` folder.
const NAMES: &[(&str, &str)] = &[
    ("msedge", "edge"),
    ("brave", "brave"),
    ("vivaldi", "vivaldi"),
    ("opera", "opera"),
    ("chromium", "chromium"),
    ("chrome", "chrome"),
    ("waterfox", "waterfox"),
    ("librewolf", "librewolf"),
    ("floorp", "floorp"),
    ("mullvad", "mullvad"),
    ("zen", "zen"),
    ("firefox", "firefox"),
];

/// The browser an executable belongs to, from its last two path components: `firefox-bin`,
/// `/opt/google/chrome/chrome`, `/app/chromium/chrome` (Flatpak), `msedge.exe`. A name counts
/// whole or followed by `-`/`.` (`zen-bin`, not `zenity`).
pub(crate) fn key_of_exe(path: &str) -> Option<&'static str> {
    let path = path.to_ascii_lowercase();
    let tail: Vec<&str> = path.rsplit(['/', '\\']).take(2).collect();
    NAMES.iter().find_map(|&(name, key)| {
        tail.iter()
            .any(|part| part.strip_prefix(name).is_some_and(|rest| rest.is_empty() || rest.starts_with(['-', '.'])))
            .then_some(key)
    })
}

/// The keys of the browsers running now.
pub fn running() -> HashSet<&'static str> {
    imp::executables().iter().filter_map(|exe| key_of_exe(exe)).collect()
}

#[cfg(target_os = "linux")]
mod imp {
    /// `/proc/<pid>/exe` of every process this account can see (other accounts' are unreadable).
    pub fn executables() -> Vec<String> {
        let Ok(dir) = std::fs::read_dir("/proc") else { return Vec::new() };
        dir.flatten()
            .filter(|e| e.file_name().as_encoded_bytes().iter().all(u8::is_ascii_digit))
            .filter_map(|e| std::fs::read_link(e.path().join("exe")).ok())
            .map(|p| p.to_string_lossy().into_owned())
            .collect()
    }
}

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
        System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS},
    };

    /// The executable file name of every process (`chrome.exe`…).
    pub fn executables() -> Vec<String> {
        let mut names = Vec::new();
        // SAFETY: a snapshot handle, closed below; the entry's size is set as the API requires.
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return names;
            }
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
            let mut more = Process32FirstW(snapshot, &mut entry) != 0;
            while more {
                let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
                names.push(String::from_utf16_lossy(&entry.szExeFile[..len]));
                more = Process32NextW(snapshot, &mut entry) != 0;
            }
            CloseHandle(snapshot);
        }
        names
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
mod imp {
    pub fn executables() -> Vec<String> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::key_of_exe;

    #[test]
    fn browsers_are_known_by_their_executable() {
        for (exe, key) in [
            ("/usr/lib/firefox/firefox", "firefox"),
            ("/app/lib/firefox/firefox-bin", "firefox"),
            ("/opt/google/chrome/chrome", "chrome"),
            ("/app/extra/chrome", "chrome"),
            ("/app/chromium/chrome", "chromium"),
            ("/opt/brave.com/brave/brave", "brave"),
            ("/opt/microsoft/msedge/msedge", "edge"),
            ("/opt/vivaldi/vivaldi-bin", "vivaldi"),
            ("/opt/zen/zen-bin", "zen"),
            ("C:\\Program Files\\Mozilla Firefox\\firefox.exe", "firefox"),
            ("msedge.exe", "edge"),
            ("Brave.exe", "brave"),
        ] {
            assert_eq!(key_of_exe(exe), Some(key), "{exe}");
        }
        for exe in ["/usr/bin/zenity", "/usr/bin/bash", "/home/firefoxfan/bin/tool", "/usr/bin/operator"] {
            assert_eq!(key_of_exe(exe), None, "{exe}");
        }
    }
}
