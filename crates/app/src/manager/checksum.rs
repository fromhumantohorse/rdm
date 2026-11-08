//! Checking a finished file against the checksum the user expects for it (see `domain::Checksum`).

use std::{io::Read, path::Path, sync::Arc};

use domain::{Checksum, DownloadId, HashAlgo, Status};
use md5::Md5;
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha512};

use super::{Entry, Manager, Verify};

/// Hex digest of the file at `path` (read in 1 MiB blocks: any size, little memory).
pub fn digest(path: &Path, algo: HashAlgo) -> std::io::Result<String> {
    fn run<D: Digest>(path: &Path) -> std::io::Result<String> {
        let mut file = std::fs::File::open(path)?;
        let mut hasher = D::new();
        let mut buf = vec![0u8; 1 << 20];
        loop {
            match file.read(&mut buf)? {
                0 => break,
                n => hasher.update(&buf[..n]),
            }
        }
        Ok(crate::hex::encode(&hasher.finalize()))
    }
    match algo {
        HashAlgo::Md5 => run::<Md5>(path),
        HashAlgo::Sha1 => run::<Sha1>(path),
        HashAlgo::Sha256 => run::<Sha256>(path),
        HashAlgo::Sha512 => run::<Sha512>(path),
    }
}

impl Manager {
    /// Sets (or clears) the checksum `id` must match; a finished file is checked at once.
    /// `false` when `input` is not a checksum.
    pub fn set_checksum(self: &Arc<Self>, id: DownloadId, input: &str) -> bool {
        let checksum = if input.trim().is_empty() {
            None
        } else {
            match Checksum::parse(input) {
                Some(c) => Some(c),
                None => return false,
            }
        };
        let done = self.update(id, |e| {
            e.download.checksum = checksum;
            e.verify = Verify::None;
            *e.download.status() == Status::Completed
        });
        if done == Some(true) {
            self.verify(id);
        }
        true
    }

    /// Checks a finished download against its expected checksum, off the UI thread.
    pub(super) fn verify(self: &Arc<Self>, id: DownloadId) {
        let job = self.update_quiet(id, |e| {
            let expected = e.download.checksum.clone()?;
            e.verify = Verify::Running;
            Some((e.download.target.clone(), expected, e.name.clone()))
        });
        let Some(Some((path, expected, name))) = job else { return };
        let this = self.clone();
        self.rt.spawn_blocking(move || {
            let digest = digest(&path, expected.algo());
            let mismatch = this.update(id, |e| finish_verification(e, &path, &expected, digest));
            if mismatch == Some(true) {
                crate::notify::checksum_mismatch(&name);
            }
        });
    }
}

/// A hash can take minutes. A cleared or replaced expectation must not be overwritten by the
/// previous check's result (nor generate a notification about an expectation no longer in use).
fn finish_verification(entry: &mut Entry, path: &Path, expected: &Checksum, digest: std::io::Result<String>) -> bool {
    if entry.download.target != path || entry.download.checksum.as_ref() != Some(expected) || *entry.download.status() != Status::Completed {
        return false;
    }
    if expected.algo() == HashAlgo::Sha256 && let Ok(hash) = &digest {
        entry.sha256 = Some(hash.clone());
    }
    entry.verify = match digest {
        Ok(actual) if expected.matches(&actual) => Verify::Ok,
        Ok(actual) => Verify::Mismatch(actual),
        Err(e) => Verify::Failed(e.to_string()),
    };
    matches!(entry.verify, Verify::Mismatch(_))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_late_checksum_result_cannot_replace_the_current_verdict() {
        let mut download = domain::Download::new("https://example.com/a.zip".parse().unwrap(), std::env::temp_dir().join("a.zip"), 1);
        download.start().unwrap();
        download.complete().unwrap();
        let mut entry = Entry::new(download, Vec::new(), 0, 0);
        let path = entry.download.target.clone();
        let old = Checksum::parse(&"11".repeat(32)).unwrap();
        let current = Checksum::parse(&"22".repeat(32)).unwrap();
        entry.download.checksum = Some(current.clone());
        entry.verify = Verify::Running;
        assert!(!finish_verification(&mut entry, &path, &old, Ok("11".repeat(32))));
        assert_eq!(entry.verify, Verify::Running, "an old success must not mark the new checksum valid");
        assert!(entry.sha256.is_none());
        assert!(!finish_verification(&mut entry, &path, &current, Ok("22".repeat(32))));
        assert_eq!(entry.verify, Verify::Ok);
        entry.download.checksum = None;
        entry.verify = Verify::None;
        assert!(!finish_verification(&mut entry, &path, &current, Ok("33".repeat(32))));
        assert_eq!(entry.verify, Verify::None, "a cleared checksum stays cleared, without a mismatch notification");
    }

    #[test]
    fn digests_of_known_content() {
        let path = std::env::temp_dir().join(format!("rdm-digest-{}", std::process::id()));
        std::fs::write(&path, b"abc").unwrap();
        assert_eq!(digest(&path, HashAlgo::Md5).unwrap(), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(digest(&path, HashAlgo::Sha1).unwrap(), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(digest(&path, HashAlgo::Sha256).unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert!(digest(&path, HashAlgo::Sha512).unwrap().starts_with("ddaf35a193617aba"));
        let _ = std::fs::remove_file(path);
    }
}
