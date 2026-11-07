//! Installing a checked package: the Windows installation assistant, the Linux binary or package
//! manager; the update's leftovers.

use super::*;

/// The command-line flag of the installation assistant: `rdm --apply-update <msi> <pid> <exe>
/// <sha256>` (the package's SHA-256, checked again just before installing it).
pub const HELPER_FLAG: &str = "--apply-update";
/// What the assistant's folder is named after, in the temporary folder.
const HELPER_PREFIX: &str = "rdm-updater-";

/// Where the Windows installer puts RDM (per user).
pub fn installed_exe() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join(r"Programs\RDM\rdm.exe"))
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Windows: starts the installation assistant for `msi` (see the module docs). The caller quits
/// right after.
pub fn start_installation(msi: &Verified) -> Result<(), UpdateError> {
    let target = installed_exe().ok_or(UpdateError::NotHere)?;
    // In a folder of its own: a program looks for its libraries beside itself first, and the
    // temporary folder holds whatever other programs leave there.
    let dir = std::env::temp_dir().join(format!("{HELPER_PREFIX}{}", std::process::id()));
    let helper = dir.join("rdm-updater.exe");
    let start = || {
        std::fs::create_dir_all(&dir)?;
        std::fs::copy(startup_exe().map_or_else(std::env::current_exe, |p| Ok(p.to_path_buf()))?, &helper)?;
        let mut command = std::process::Command::new(&helper);
        command.arg(HELPER_FLAG).arg(&msi.path).arg(std::process::id().to_string()).arg(&target).arg(&msi.sha256);
        assistant::spawn_outliving(&mut command)
    };
    start().map_err(UpdateError::CannotStart)
}

/// Linux: installs the downloaded package (binary replaced in place, or the package manager behind
/// the system's password prompt). RDM restarts afterwards.
pub async fn install_linux(package: Verified) -> Result<(), UpdateError> {
    tokio::task::spawn_blocking(move || linux::install(method(), &package)).await.map_err(|_| UpdateError::InstallationInterrupted)?
}

/// Whether the file at `path` is still the package that was verified (its SHA-256 `expected`).
#[cfg_attr(target_os = "macos", allow(dead_code, reason = "RDM does not update itself on macOS"))]
fn unchanged(path: &Path, expected: &str) -> bool {
    crate::manager::checksum::digest(path, domain::HashAlgo::Sha256).is_ok_and(|digest| digest.eq_ignore_ascii_case(expected))
}

/// After quitting for an update: starts the (new) RDM from where it was installed.
pub fn relaunch() {
    if let Some(exe) = startup_exe() {
        let _ = std::process::Command::new(exe).spawn();
    }
}

/// In the assistant: `true` when `args` asked for it (it then did its job).
pub fn run_assistant(args: &[String]) -> bool {
    let [flag, msi, pid, exe, sha256] = args else { return false };
    if flag != HELPER_FLAG {
        return false;
    }
    assistant::apply(Path::new(msi), pid.parse().unwrap_or(0), Path::new(exe), sha256);
    true
}

/// Where update packages are downloaded: the user's own temporary folder on Windows; on Linux a
/// private folder (`~/.cache/rdm`, 0700) rather than the shared `/tmp`, where another account
/// could prepare a file or a link under the expected name.
pub(super) fn work_dir() -> std::io::Result<PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let dir = directories::BaseDirs::new().ok_or(std::io::ErrorKind::NotFound)?.cache_dir().join("rdm");
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        Ok(dir)
    }
    #[cfg(not(unix))]
    {
        Ok(std::env::temp_dir())
    }
}

/// At start: leftovers of a previous update (package, assistant copy, old logs). Files still in
/// use (an assistant finishing its job) are left for next time.
pub fn clean_leftovers() {
    if let Ok(dir) = work_dir() {
        clean(&dir);
    }
}

fn clean(dir: &Path) {
    let Ok(dir) = std::fs::read_dir(dir) else { return };
    let week = Duration::from_secs(7 * 24 * 3600);
    for entry in dir.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let old = || entry.metadata().and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().is_ok_and(|age| age > week));
        let update = name.starts_with("rdm-update-");
        let helper = name.starts_with(HELPER_PREFIX);
        // Beside its folder: where an older RDM left it.
        let leftover = helper && name.ends_with(".exe")
            || update && [".msi", ".tmp", ".tar.gz", ".deb", ".rpm"].iter().any(|e| name.ends_with(e))
            || update && name.ends_with(".log") && old();
        if leftover {
            let _ = std::fs::remove_file(entry.path());
        } else if (update || helper) && entry.file_type().is_ok_and(|t| t.is_dir()) {
            let _ = std::fs::remove_dir_all(entry.path()); // an unpacked tarball (Linux), the assistant's folder
        }
    }
}

#[cfg(target_os = "linux")]
mod linux;

#[cfg(not(target_os = "linux"))]
mod linux {
    use super::{Method, UpdateError, Verified};

    pub fn install(_: Method, _: &Verified) -> Result<(), UpdateError> {
        Err(UpdateError::NotHere)
    }
}

#[cfg(windows)]
mod assistant;

#[cfg(not(windows))]
mod assistant {
    use std::{path::Path, process::Command};

    pub fn spawn_outliving(_: &mut Command) -> std::io::Result<()> {
        Err(std::io::ErrorKind::Unsupported.into())
    }

    pub fn apply(_: &Path, _: u32, _: &Path, _: &str) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_assistant_runs_only_with_its_five_arguments() {
        assert!(!run_assistant(&["--minimized".into()]));
        assert!(!run_assistant(&["--other".into(), "a".into(), "1".into(), "b".into(), "c".into()]));
        assert!(!run_assistant(&[HELPER_FLAG.into(), "a.msi".into(), "1".into(), "rdm.exe".into()]), "the SHA-256 is required");
    }

    #[test]
    fn only_an_updates_own_leftovers_are_cleaned() {
        let dir = std::env::temp_dir().join(format!("rdm-leftovers-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("rdm-updater-12")).unwrap();
        std::fs::create_dir_all(dir.join("rdm-update-34")).unwrap();
        std::fs::create_dir_all(dir.join("another-folder")).unwrap();
        std::fs::write(dir.join("rdm-updater-12").join("rdm-updater.exe"), b"").unwrap();
        for file in ["rdm-updater-56.exe", "rdm-update-0.3.14.msi", "rdm-update-0.3.14.log", "someone-elses.msi"] {
            std::fs::write(dir.join(file), b"").unwrap();
        }

        clean(&dir);

        let mut left: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, ["another-folder", "rdm-update-0.3.14.log", "someone-elses.msi"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The package is checked again at the last moment against the SHA-256 its signature covers.
    #[test]
    fn a_package_changed_after_its_check_is_noticed() {
        let path = std::env::temp_dir().join(format!("rdm-unchanged-{}.bin", std::process::id()));
        std::fs::write(&path, b"").unwrap();
        const EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert!(unchanged(&path, EMPTY) && unchanged(&path, &EMPTY.to_ascii_uppercase()));
        std::fs::write(&path, b"swapped").unwrap();
        assert!(!unchanged(&path, EMPTY));
        let _ = std::fs::remove_file(&path);
        assert!(!unchanged(&path, EMPTY), "gone");
    }

    /// What `pkexec` runs as root: the copy it installs is the verified file, or nothing is.
    #[cfg(target_os = "linux")]
    #[test]
    fn root_installs_only_the_verified_copy() {
        let path = std::env::temp_dir().join(format!("rdm_{}_amd64.deb", std::process::id()));
        std::fs::write(&path, b"").unwrap();
        const EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let run = |sha: &str, tool: &[&str]| {
            std::process::Command::new("sh").args(["-c", linux::AS_ROOT, "sh"]).arg(&path).arg(sha).args(tool).status().unwrap().code()
        };
        // The tool gets a copy of the same name, in a fresh folder: not the user's file ($1 here).
        let original = path.to_str().unwrap();
        let check = r#"test -f "$2" && test "$2" != "$1" && test "$(basename "$2")" = "$(basename "$1")""#;
        assert_eq!(run(EMPTY, &["sh", "-c", check, "sh", original]), Some(0));
        assert_eq!(run(&"0".repeat(64), &["true"]), Some(3), "swapped: never installed");
        let _ = std::fs::remove_file(&path);
    }
}
