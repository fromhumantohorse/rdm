//! Linux: the binary replaced in place, or the package manager behind the system's password prompt.

use std::{os::unix::fs::PermissionsExt, process::Command};

use super::{Method, UpdateError, Verified, startup_exe, unchanged, which};

pub fn install(method: Method, package: &Verified) -> Result<(), UpdateError> {
    match method {
        Method::Binary => replace_binary(package),
        Method::Deb => {
            let tool = if which("apt-get").is_some() { vec!["apt-get", "install", "-y", "--allow-downgrades"] } else { vec!["dpkg", "-i"] };
            elevated(&tool, package)
        }
        Method::Rpm => {
            let tool = if which("dnf").is_some() {
                vec!["dnf", "install", "-y"]
            } else if which("zypper").is_some() {
                vec!["zypper", "--non-interactive", "install", "--allow-unsigned-rpm"]
            } else {
                vec!["rpm", "-U", "--replacepkgs"]
            };
            elevated(&tool, package)
        }
        Method::Msi | Method::Manual => Err(UpdateError::NotHere),
    }
}

/// Run as root (`sh -c` with the package, its verified SHA-256, then the package manager's
/// command): the package is copied where only root can write, checked to be the very file RDM
/// verified, and that copy is installed. Another program of the account cannot swap the
/// package between RDM's checks and the installation as root. Exit code 3: it was swapped.
pub(super) const AS_ROOT: &str = r#"set -eu
dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT
copy="$dir/$(basename "$1")"
cp -- "$1" "$copy"
chmod 755 "$dir"
printf '%s  %s\n' "$2" "$copy" | sha256sum -c --status - || exit 3
shift 2
"$@" "$copy"
"#;

/// The package manager, as root after the system's password prompt (polkit).
fn elevated(tool: &[&str], package: &Verified) -> Result<(), UpdateError> {
    let status = Command::new("pkexec")
        .args(["/bin/sh", "-c", AS_ROOT, "sh"])
        .arg(&package.path)
        .arg(&package.sha256)
        .args(tool)
        .status()
        .map_err(UpdateError::CannotStart)?;
    match status.code() {
        Some(0) => Ok(()),
        Some(3) => Err(UpdateError::ChangedAfterVerification),
        Some(126 | 127) => Err(UpdateError::PasswordRefused),
        Some(c) => Err(UpdateError::PackageManagerFailed(c)),
        None => Err(UpdateError::InstallationInterrupted),
    }
}

/// Unpacks the tarball and swaps the binary in place (a rename: the running process keeps its
/// file until it exits, and a crash half-way leaves the old binary intact).
fn replace_binary(package: &Verified) -> Result<(), UpdateError> {
    // Everything here runs as the user: checked again all the same, just before use.
    if !unchanged(&package.path, &package.sha256) {
        return Err(UpdateError::ChangedAfterVerification);
    }
    let archive = package.path.as_path();
    let exe = startup_exe().ok_or(UpdateError::UnknownLocation)?;
    let work = super::work_dir()?.join(format!("rdm-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work)?;
    let result = (|| {
        let ok = Command::new("tar").arg("-xzf").arg(archive).arg("-C").arg(&work).status().is_ok_and(|s| s.success());
        let new = work.join("rdm-linux-x64").join("rdm");
        let elf = std::fs::read(&new).is_ok_and(|b| b.starts_with(b"\x7fELF"));
        if !ok || !elf {
            return Err(UpdateError::UnreadableArchive);
        }
        let staged = exe.with_file_name(".rdm-update");
        std::fs::copy(&new, &staged)?;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
        std::fs::rename(&staged, exe)?;
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&work);
    result
}
