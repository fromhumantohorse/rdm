//! The file being downloaded, opened once for every connection: each writes at its own offset (no
//! seek, no reopening). Opening and closing a file for each piece costs system calls — and on
//! Windows an antivirus scan of the whole file at each close after writing.

use std::{path::Path, sync::Arc};

use tokio::fs::{self, File, OpenOptions};

pub(super) struct Disk(std::fs::File);

impl Disk {
    /// A new file of `size` bytes (sparse where the system allows it).
    pub(super) async fn create(path: &Path, size: u64) -> std::io::Result<Arc<Self>> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).await?;
        }
        let file = File::create(path).await?.into_std().await;
        sparse(&file);
        let disk = Arc::new(Self(file));
        let d = disk.clone();
        tokio::task::spawn_blocking(move || d.0.set_len(size)).await.map_err(std::io::Error::other)??;
        Ok(disk)
    }

    /// The file of a download being resumed.
    pub(super) async fn open(path: &Path) -> std::io::Result<Arc<Self>> {
        let file = OpenOptions::new().write(true).open(path).await?.into_std().await;
        Ok(Arc::new(Self(file)))
    }

    pub(super) async fn write_at(self: &Arc<Self>, buf: Vec<u8>, offset: u64) -> std::io::Result<Vec<u8>> {
        let disk = self.clone();
        tokio::task::spawn_blocking(move || disk.write_all_at(&buf, offset).map(|()| buf)).await.map_err(std::io::Error::other)?
    }

    /// What was written so far is on disk once this returns: through this handle, no other opened
    /// and closed around the connections (on Windows, an antivirus scan of the file at each close).
    pub(super) async fn sync(self: &Arc<Self>) -> std::io::Result<()> {
        let disk = self.clone();
        tokio::task::spawn_blocking(move || disk.0.sync_data()).await.map_err(std::io::Error::other)?
    }

    pub(super) async fn truncate(self: &Arc<Self>) -> std::io::Result<()> {
        let disk = self.clone();
        tokio::task::spawn_blocking(move || disk.0.set_len(0)).await.map_err(std::io::Error::other)?
    }

    fn write_all_at(&self, buf: &[u8], offset: u64) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::FileExt;
            self.0.write_all_at(buf, offset)
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::FileExt;
            let (mut buf, mut offset) = (buf, offset);
            while !buf.is_empty() {
                match self.0.seek_write(buf, offset) {
                    Ok(0) => return Err(std::io::ErrorKind::WriteZero.into()),
                    Ok(n) => {
                        buf = &buf[n..];
                        offset += n as u64;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(e) => return Err(e),
                }
            }
            Ok(())
        }
    }
}

/// Windows: a sparse file. On a plain NTFS file, writing far past the data written so far first
/// makes the file system fill the gap with zeros — every segment but the first would wait for
/// gigabytes of zeros before its first byte lands, and the disk would write the file twice.
/// (Linux file systems create sparse files by themselves.) Best effort: FAT/exFAT refuse it.
fn sparse(file: &std::fs::File) {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;

        use windows_sys::Win32::System::{IO::DeviceIoControl, Ioctl::FSCTL_SET_SPARSE};
        let mut returned = 0u32;
        // SAFETY: a valid handle we own for the duration of the call; no input or output buffer.
        unsafe {
            DeviceIoControl(
                file.as_raw_handle() as _,
                FSCTL_SET_SPARSE,
                std::ptr::null(),
                0,
                std::ptr::null_mut(),
                0,
                &mut returned,
                std::ptr::null_mut(),
            );
        }
    }
    #[cfg(not(windows))]
    let _ = file;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn every_connection_writes_at_its_own_offset() {
        let dir = std::env::temp_dir().join(format!("rdm-disk-{}", std::process::id()));
        let path = dir.join("f.bin");
        let disk = Disk::create(&path, 8).await.unwrap();
        let (a, b) = tokio::join!(disk.write_at(b"5678".to_vec(), 4), disk.write_at(b"1234".to_vec(), 0));
        assert!(a.unwrap().capacity() >= 4 && b.is_ok(), "the buffers come back for reuse");
        drop(disk);
        assert_eq!(std::fs::read(&path).unwrap(), b"12345678");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
