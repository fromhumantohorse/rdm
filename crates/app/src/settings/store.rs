//! RDM's own files, in the settings folder: private to the user, written atomically, read back
//! leniently (a damaged one is kept aside, not lost).

use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::{Mutex, PoisonError},
    time::Duration,
};

use directories::ProjectDirs;
use serde::Serialize;

pub use engine::with_suffix;

/// A file another program holds a moment (an antivirus scanning it at start) is read again.
const READ_ATTEMPTS: u32 = 4;
const READ_RETRY: Duration = Duration::from_millis(150);

/// `RDM_CONFIG_DIR` overrides the location (portable mode, e.g. on a USB stick).
pub fn config_file(name: &str) -> PathBuf {
    std::env::var_os("RDM_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| ProjectDirs::from("org", "rdm", "rdm").map(|d| d.config_dir().to_path_buf()))
        .unwrap_or_default()
        .join(name)
}

/// A JSON file of the settings folder; the default when there is none. A file this version cannot
/// read (damaged) is kept aside as `<name>.bad` rather than overwritten by the next save.
pub fn load_json<T: serde::de::DeserializeOwned + Default>(name: &str) -> T {
    load_json_at(&config_file(name))
}

fn load_json_at<T: serde::de::DeserializeOwned + Default>(path: &Path) -> T {
    let Some(bytes) = read_config(path) else { return T::default() };
    serde_json::from_slice(&bytes).unwrap_or_else(|_| {
        keep_aside(path);
        T::default()
    })
}

/// A file of the settings folder; `None` when there is none. One another program holds a moment
/// is read again; one that stays unreadable is kept aside, like a damaged one.
pub fn read_config(path: &Path) -> Option<Vec<u8>> {
    let mut attempt = 1;
    loop {
        match fs::read(path) {
            Ok(bytes) => return Some(bytes),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
            Err(_) if attempt < READ_ATTEMPTS => {
                attempt += 1;
                std::thread::sleep(READ_RETRY);
            }
            Err(_) => {
                keep_aside(path);
                return None;
            }
        }
    }
}

/// A copy of a file this RDM cannot use, `<name>.bad`, for the user: the next save replaces the
/// file itself. Best effort: a file that cannot be read cannot always be copied either.
pub fn keep_aside(path: &Path) {
    let _ = fs::copy(path, with_suffix(path, ".bad"));
}

/// Atomic write (temp file + rename), serialized: concurrent saves from the UI and download
/// threads can neither interleave in the temp file nor leave a truncated config after a crash.
pub fn save_json(name: &str, value: &impl Serialize) -> io::Result<()> {
    static WRITE: Mutex<()> = Mutex::new(());
    let bytes = serde_json::to_vec(value)?;
    let path = config_file(name);
    let _guard = WRITE.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(dir) = path.parent() {
        create_private_dir(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    // Synced before the rename: a power cut right after must not leave an empty list behind.
    let written = fs::File::create(&tmp)
        .and_then(|mut f| {
            io::Write::write_all(&mut f, &bytes)?;
            f.sync_all()
        })
        .and_then(|()| fs::rename(&tmp, &path));
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

/// RDM's settings folder, private to the user on Linux and macOS (0700, like `~/.ssh`): the download
/// list holds links with their access tokens. Windows: the profile folder is private already.
pub fn create_private_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        // Made private too when an older RDM created it readable by everyone.
        if fs::metadata(dir).is_ok_and(|m| m.permissions().mode() & 0o077 != 0) {
            let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
        }
        Ok(())
    }
    #[cfg(not(unix))]
    fs::create_dir_all(dir)
}

/// Writes a file only the user can read: created 0600 on Linux — never readable by others, not
/// even for an instant; Windows: the profile folder is private to the account already.
pub fn write_private(path: &Path, data: &[u8]) -> io::Result<()> {
    use std::io::Write;
    #[cfg(unix)]
    let mut f = {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let f = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
        // A leftover file of that name keeps its mode when reopened: made private in any case.
        f.set_permissions(fs::Permissions::from_mode(0o600))?;
        f
    };
    #[cfg(not(unix))]
    let mut f = fs::File::create(path)?;
    f.write_all(data)?;
    f.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn the_settings_folder_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("rdm-private-{}", std::process::id()));
        let nested = dir.join("rdm");
        create_private_dir(&nested).unwrap();
        assert_eq!(fs::metadata(&nested).unwrap().permissions().mode() & 0o777, 0o700);
        fs::set_permissions(&nested, fs::Permissions::from_mode(0o755)).unwrap();
        create_private_dir(&nested).unwrap();
        assert_eq!(fs::metadata(&nested).unwrap().permissions().mode() & 0o777, 0o700, "an older, readable one is fixed");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_damaged_file_is_kept_aside() {
        let dir = std::env::temp_dir().join(format!("rdm-load-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        assert_eq!(load_json_at::<Vec<u32>>(&path), Vec::<u32>::new(), "none yet: the default");
        fs::write(&path, b"[1, 2").unwrap();
        assert_eq!(load_json_at::<Vec<u32>>(&path), Vec::<u32>::new());
        assert_eq!(fs::read(with_suffix(&path, ".bad")).unwrap(), b"[1, 2", "kept for the user");
        fs::write(&path, b"[1, 2]").unwrap();
        assert_eq!(load_json_at::<Vec<u32>>(&path), [1, 2]);
        let _ = fs::remove_dir_all(&dir);
    }
}
