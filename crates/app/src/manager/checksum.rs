//! Checking a finished file against the checksum the user expects for it (see `domain::Checksum`).

use std::{io::Read, path::Path, sync::Arc};

use domain::{Checksum, DownloadId, HashAlgo, Status};
use md5::Md5;
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha512};

use super::{Manager, Verify};

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
            // Worth keeping: VirusTotal looks files up by SHA-256.
            if expected.algo() == HashAlgo::Sha256
                && let Ok(hash) = &digest
            {
                let hash = hash.clone();
                this.update_quiet(id, |e| e.sha256 = Some(hash));
            }
            let state = match digest {
                Ok(actual) if expected.matches(&actual) => Verify::Ok,
                Ok(actual) => {
                    crate::notify::checksum_mismatch(&name);
                    Verify::Mismatch(actual)
                }
                Err(e) => Verify::Failed(e.to_string()),
            };
            this.update(id, |e| e.verify = state);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
