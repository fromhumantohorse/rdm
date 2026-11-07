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
    ("safari", "safari"),
    // macOS names them in full (`Google Chrome.app/Contents/MacOS/Google Chrome`).
    ("google chrome", "chrome"),
    ("microsoft edge", "edge"),
    ("brave browser", "brave"),
    ("mullvad browser", "mullvad"),
    ("zen browser", "zen"),
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

/// The keys of the browsers the user has open now.
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
    use std::collections::HashSet;

    use windows_sys::{
        Win32::{
            Foundation::{CloseHandle, HWND, INVALID_HANDLE_VALUE, LPARAM},
            System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS},
            UI::WindowsAndMessaging::{EnumWindows, GetWindowThreadProcessId, IsWindowVisible},
        },
        core::BOOL,
    };

    /// The processes with a window on this session's screen. A browser without one was not opened
    /// by the user: Edge's "startup boost" or Chrome's background apps, there from the login on
    /// (and no other session's browser either).
    fn with_a_window() -> HashSet<u32> {
        unsafe extern "system" fn add(window: HWND, found: LPARAM) -> BOOL {
            // SAFETY: `found` points to the set below, alive for the whole enumeration; the rest are
            // plain queries on a window handle the system just gave.
            unsafe {
                if IsWindowVisible(window) != 0 {
                    let mut pid = 0u32;
                    GetWindowThreadProcessId(window, &mut pid);
                    (*(found as *mut HashSet<u32>)).insert(pid);
                }
            }
            1
        }
        let mut found = HashSet::new();
        // SAFETY: the callback above; `found` outlives the call, which returns once all were seen.
        unsafe { EnumWindows(Some(add), (&raw mut found) as LPARAM) };
        found
    }

    /// The executable file name of every process with a window (`chrome.exe`…).
    pub fn executables() -> Vec<String> {
        let windowed = with_a_window();
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
                if windowed.contains(&entry.th32ProcessID) {
                    let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
                    names.push(String::from_utf16_lossy(&entry.szExeFile[..len]));
                }
                more = Process32NextW(snapshot, &mut entry) != 0;
            }
            CloseHandle(snapshot);
        }
        names
    }
}

#[cfg(target_os = "macos")]
mod imp {
    /// The program of every process this account can see.
    pub fn executables() -> Vec<String> {
        // SAFETY: a null buffer only asks how many processes there are.
        let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
        let mut pids = vec![0 as libc::c_int; usize::try_from(count).unwrap_or(0) + 32]; // some may start meanwhile
        let Ok(bytes) = libc::c_int::try_from(pids.len() * size_of::<libc::c_int>()) else { return Vec::new() };
        // SAFETY: `pids` holds `bytes` writable bytes.
        let listed = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
        pids.truncate(usize::try_from(listed).unwrap_or(0));
        let path_of = |pid| {
            let mut path = vec![0u8; 4096]; // PROC_PIDPATHINFO_MAXSIZE
            // SAFETY: `path` holds that many writable bytes.
            let len = unsafe { libc::proc_pidpath(pid, path.as_mut_ptr().cast(), 4096) };
            path.truncate(usize::try_from(len).ok()?);
            (!path.is_empty()).then(|| String::from_utf8_lossy(&path).into_owned())
        };
        pids.into_iter().filter_map(path_of).collect()
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
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
            ("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", "chrome"),
            ("/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge", "edge"),
            ("/Applications/Brave Browser.app/Contents/MacOS/Brave Browser", "brave"),
            ("/Applications/Firefox.app/Contents/MacOS/firefox", "firefox"),
            ("/Applications/Safari.app/Contents/MacOS/Safari", "safari"),
        ] {
            assert_eq!(key_of_exe(exe), Some(key), "{exe}");
        }
        for exe in [
            "/usr/bin/zenity",
            "/usr/bin/bash",
            "/home/firefoxfan/bin/tool",
            "/usr/bin/operator",
            // A browser's helpers are not the browser starting.
            "/Applications/Google Chrome.app/Contents/Frameworks/Google Chrome Framework.framework/Helpers/Google Chrome Helper (Renderer).app/Contents/MacOS/Google Chrome Helper (Renderer)",
        ] {
            assert_eq!(key_of_exe(exe), None, "{exe}");
        }
    }
}
