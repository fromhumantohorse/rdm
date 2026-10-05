//! How this copy of RDM was installed, hence which package updates it.

use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};

#[cfg(windows)]
use super::installed_exe;

/// How this copy of RDM was installed, hence which package updates it.
#[cfg_attr(not(target_os = "linux"), allow(dead_code, reason = "the Linux methods are detected on Linux only"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// Windows installer (per user, `%LOCALAPPDATA%\Programs\RDM`).
    Msi,
    /// Linux binary in a folder the user can write (`~/.local/bin`, the tarball).
    Binary,
    /// Linux, installed by a package manager.
    Deb,
    Rpm,
    /// A copy RDM cannot update itself (a build folder, a USB stick…): the page is opened.
    Manual,
}

static EXE: OnceLock<PathBuf> = OnceLock::new();

/// Remembers the executable RDM started from: after an in-place update the running file is gone
/// (Linux then reports `…/rdm (deleted)`), and the restart must use the path, not the process.
pub fn remember_exe() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = EXE.set(std::fs::canonicalize(&exe).unwrap_or(exe));
    }
}

pub(super) fn startup_exe() -> Option<&'static Path> {
    EXE.get().map(PathBuf::as_path)
}

pub fn method() -> Method {
    static METHOD: OnceLock<Method> = OnceLock::new();
    *METHOD.get_or_init(detect)
}

#[cfg(windows)]
fn detect() -> Method {
    let (Some(installed), Some(me)) = (installed_exe(), startup_exe()) else { return Method::Manual };
    if same_file(&installed, me) { Method::Msi } else { Method::Manual }
}

/// Whether two paths name the same existing file, whatever their case or form (Windows).
#[cfg(windows)]
pub(super) fn same_file(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| std::fs::canonicalize(p).map(|p| p.to_string_lossy().to_lowercase());
    matches!((norm(a), norm(b)), (Ok(a), Ok(b)) if a == b)
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn detect() -> Method {
    let Some(exe) = startup_exe() else { return Method::Manual };
    let succeeds = |program: &str, args: &[&std::ffi::OsStr]| {
        std::process::Command::new(program)
            .args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    };
    if exe.starts_with("/usr/") || exe.starts_with("/opt/") {
        let has_pkexec = which("pkexec").is_some();
        if has_pkexec && succeeds("dpkg-query", &["-S".as_ref(), exe.as_os_str()]) {
            return Method::Deb;
        }
        if has_pkexec && succeeds("rpm", &["-qf".as_ref(), exe.as_os_str()]) {
            return Method::Rpm;
        }
        return Method::Manual;
    }
    if writable(exe) { Method::Binary } else { Method::Manual }
}

#[cfg(not(any(windows, all(target_os = "linux", target_arch = "x86_64"))))]
fn detect() -> Method {
    Method::Manual
}

#[cfg(target_os = "linux")]
fn writable(exe: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let access = |p: &Path| {
        let Ok(c) = std::ffi::CString::new(p.as_os_str().as_bytes()) else { return false };
        // SAFETY: a valid NUL-terminated path; `access` only reads it.
        unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
    };
    access(exe) && exe.parent().is_some_and(access)
}

#[cfg(target_os = "linux")]
pub(super) fn which(program: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .chain([PathBuf::from("/usr/bin"), PathBuf::from("/usr/sbin"), PathBuf::from("/bin")])
        .map(|d| d.join(program))
        .find(|p| p.is_file())
}
