//! Handing things to the desktop: a file to its default application, a folder to the file manager,
//! a page to the browser. Off the UI thread; a failure is said in the window.

use std::{path::PathBuf, sync::Arc};

use crate::{manager::Manager, trf};

/// Opens a finished file with its default application.
///
/// On Linux and macOS the opener (`xdg-open`, `open`) is waited for, so it is reaped instead of
/// lingering as a zombie process until RDM exits (the `opener` crate never waits for it).
pub(super) fn open_file(path: PathBuf, manager: &Arc<Manager>) {
    let manager = manager.clone();
    std::thread::spawn(move || {
        if let Err(e) = open_with_default_app(&path) {
            manager.notice(true, &trf!("Impossible d'ouvrir le fichier : {e}", "Cannot open the file: {e}", e = e));
        }
    });
}

#[cfg(windows)]
fn open_with_default_app(path: &std::path::Path) -> std::io::Result<()> {
    opener::open(path).map_err(std::io::Error::other)
}

#[cfg(not(windows))]
fn open_with_default_app(path: &std::path::Path) -> std::io::Result<()> {
    use std::process::{Command, Stdio};
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    match Command::new(opener).arg(path).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
        Ok(mut child) => match child.wait()? {
            status if status.success() => Ok(()),
            status => Err(std::io::Error::other(format!("{opener}: {status}"))),
        },
        Err(_) => opener::open(path).map_err(std::io::Error::other),
    }
}

/// Shows a file in the file manager (Linux: a D-Bus call).
pub(super) fn reveal_file(path: PathBuf, manager: &Arc<Manager>) {
    let manager = manager.clone();
    std::thread::spawn(move || {
        if let Err(e) = opener::reveal(&path) {
            manager.notice(true, &trf!("Impossible d'afficher le dossier : {e}", "Cannot show the folder: {e}", e = e));
        }
    });
}

/// Opens a web page (VirusTotal key, full report) in the default browser.
pub(super) fn open_link(url: String) {
    std::thread::spawn(move || {
        let _ = opener::open_browser(url);
    });
}
