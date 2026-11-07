//! Downloading an update package from GitHub and checking it: size, SHA-256, format and the
//! Ed25519 signature of the release workflow.

use super::*;

/// Public half of the key the release workflow signs packages with (Ed25519, raw 32 bytes).
const PUBLIC_KEY: [u8; 32] = crate::hex::decode("a972e3ab906c230c0e1c96b750e0c7d2abeae96c5979f9c48085853506b9b135");

/// A package downloaded and checked, and the SHA-256 its signature covers: the installation
/// checks the file against it again, at the last moment (see `install`).
#[derive(Debug, Clone)]
pub struct Verified {
    pub path: PathBuf,
    pub sha256: String,
}

/// Downloads `package` of `version` and checks it (size, SHA-256, format, signature);
/// `progress` gets the fraction done.
pub async fn download(client: &reqwest::Client, version: &str, package: &Package, progress: impl Fn(f32)) -> Result<Verified, UpdateError> {
    if !trusted(&package.url) {
        return Err(UpdateError::UnexpectedAddress);
    }
    let sig_url = package.signature.as_deref().filter(|u| trusted(u)).ok_or(UpdateError::Unsigned)?;
    let signature = fetch_small(client, sig_url).await?;
    let safe: String = version.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-')).collect();
    let extension = package.name.rsplit_once('.').map_or("bin", |(_, e)| e);
    let extension = if package.name.ends_with(".tar.gz") { "tar.gz" } else { extension };
    let path = work_dir()?.join(format!("rdm-update-{safe}.{extension}"));
    let mut attempt = 0;
    loop {
        let result = fetch(client, version, package, &path, &signature, &progress).await;
        match result {
            Ok(sha256) => return Ok(Verified { path, sha256 }),
            Err(e) => {
                let _ = tokio::fs::remove_file(crate::settings::with_suffix(&path, ".tmp")).await;
                attempt += 1;
                if !e.is_retryable() || attempt >= DOWNLOAD_ATTEMPTS {
                    return Err(e);
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(2 << attempt)).await;
    }
}

/// A small file (a signature), with retries.
async fn fetch_small(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, UpdateError> {
    for attempt in 0..DOWNLOAD_ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_secs(2 << attempt)).await;
        }
        let res = client.get(url).timeout(Duration::from_secs(30)).send().await.and_then(reqwest::Response::error_for_status);
        if let Ok(res) = res
            && let Ok(bytes) = crate::http::read_limited(res, 1024).await
        {
            return Ok(bytes);
        }
    }
    Err(UpdateError::SignatureUnavailable)
}

/// The package at `path`, checked; its SHA-256.
async fn fetch(client: &reqwest::Client, version: &str, package: &Package, path: &Path, signature: &[u8], progress: &impl Fn(f32)) -> Result<String, UpdateError> {
    use futures_util::StreamExt;
    use sha2::{Digest, Sha256};
    use tokio::io::AsyncWriteExt;

    let res = tokio::time::timeout(STALL, client.get(&package.url).send())
        .await
        .map_err(|_| UpdateError::Unreachable)?
        .map_err(|_| UpdateError::Unreachable)?;
    if !res.status().is_success() {
        return Err(UpdateError::DownloadRefused(res.status().as_u16()));
    }
    let total = if package.size > 0 { package.size } else { res.content_length().unwrap_or(0) };
    if total > MAX_PACKAGE {
        return Err(UpdateError::TooLarge);
    }
    let tmp = crate::settings::with_suffix(path, ".tmp");
    let mut file = tokio::fs::File::create(&tmp).await?;
    // Streamed to disk: only the first bytes (the format) and the running hash stay in memory.
    let (mut stream, mut head, mut received, mut hash) = (res.bytes_stream(), Vec::with_capacity(16), 0u64, Sha256::new());
    loop {
        let chunk = match tokio::time::timeout(STALL, stream.next()).await {
            Err(_) => return Err(UpdateError::Stalled),
            Ok(None) => break,
            Ok(Some(Err(_))) => return Err(UpdateError::Interrupted),
            Ok(Some(Ok(chunk))) => chunk,
        };
        received += chunk.len() as u64;
        if received > MAX_PACKAGE {
            return Err(UpdateError::TooLarge);
        }
        if head.len() < 16 {
            head.extend_from_slice(&chunk[..chunk.len().min(16 - head.len())]);
        }
        hash.update(&chunk);
        file.write_all(&chunk).await?;
        if total > 0 {
            progress((received as f32 / total as f32).min(1.0));
        }
    }
    file.sync_all().await?;
    drop(file);

    if total > 0 && received != total {
        return Err(UpdateError::Incomplete);
    }
    let digest = crate::hex::encode(&hash.finalize());
    if package.sha256.as_ref().is_some_and(|expected| *expected != digest) {
        return Err(UpdateError::Corrupted);
    }
    if !format_matches(&package.name, &head) {
        return Err(UpdateError::NotThePackage);
    }
    if !signature_valid(signed_statement(version, &package.name, &digest).as_bytes(), signature) {
        return Err(UpdateError::BadSignature);
    }
    tokio::fs::rename(&tmp, path).await?;
    Ok(digest)
}

/// The package's own format, from its first bytes.
fn format_matches(name: &str, data: &[u8]) -> bool {
    let name = name.to_ascii_lowercase();
    let magic: &[u8] = if name.ends_with(".msi") {
        &[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1] // OLE compound file
    } else if name.ends_with(".tar.gz") {
        &[0x1F, 0x8B]
    } else if name.ends_with(".deb") {
        b"!<arch>\n"
    } else if name.ends_with(".rpm") {
        &[0xED, 0xAB, 0xEE, 0xDB]
    } else {
        return false;
    };
    data.starts_with(magic)
}

/// What the release workflow signs for each package (`.github/workflows/release.yml`): the version
/// it belongs to, its name and its SHA-256. Binding the version defeats replays: an older signed
/// package (with a since-fixed flaw) cannot be served again as a newer release.
fn signed_statement(version: &str, name: &str, sha256: &str) -> String {
    format!("rdm-update\nversion={version}\nname={name}\nsha256={sha256}\n")
}

/// Ed25519 signature of `message` by the release key.
fn signature_valid(message: &[u8], signature: &[u8]) -> bool {
    use ed25519_dalek::{Signature, VerifyingKey};
    let (Ok(key), Ok(sig)) = (VerifyingKey::from_bytes(&PUBLIC_KEY), <[u8; 64]>::try_from(signature)) else { return false };
    key.verify_strict(message, &Signature::from_bytes(&sig)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The release workflow checks its signing key against `PUBLIC_KEY` in this very file, by its
    /// path: moved without the script, every release was refused (0.3.9's first tag).
    #[test]
    fn the_release_workflow_looks_for_the_key_here() {
        let script = include_str!("../../../../.github/sign-packages.sh");
        let here = file!().replace('\\', "/");
        assert!(script.contains(&format!("grep -q \"\\\"$public\\\"\" {here}")), "sign-packages.sh must read {here}");
    }

    #[test]
    fn signatures_are_checked_against_the_release_key() {
        // Made with the release key exactly as the release workflow does (`openssl pkeyutl -sign
        // -rawin` over the statement).
        const EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let hex = "58bc8644f48626024e184357d866862636c6c1e0a87bbfe45437383bae392645\
                   a728ed1a840668d9e9147528bead52304664bd6fd2199b86f8acb05717d23c04";
        let good: Vec<u8> = (0..64).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect();
        let statement = |version: &str, name: &str, sha: &str| signed_statement(version, name, sha).into_bytes();
        assert!(signature_valid(&statement("0.3.0", "RDM-0.3.0-x64-setup.msi", EMPTY), &good));
        assert!(!signature_valid(&statement("9.9.9", "RDM-0.3.0-x64-setup.msi", EMPTY), &good), "an old package replayed as newer");
        assert!(!signature_valid(&statement("0.3.0", "rdm_0.3.0-1_amd64.deb", EMPTY), &good), "another package");
        assert!(!signature_valid(&statement("0.3.0", "RDM-0.3.0-x64-setup.msi", &EMPTY.replace('e', "f")), &good), "other content");
        let mut forged = good.clone();
        forged[10] ^= 1;
        assert!(!signature_valid(&statement("0.3.0", "RDM-0.3.0-x64-setup.msi", EMPTY), &forged), "a forged signature");
        assert!(!signature_valid(&statement("0.3.0", "RDM-0.3.0-x64-setup.msi", EMPTY), &good[..12]), "a malformed one");
    }

    #[test]
    fn a_package_is_recognised_by_its_first_bytes() {
        assert!(format_matches("a.msi", &[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1, 0]));
        assert!(format_matches("a.tar.gz", &[0x1F, 0x8B, 8]));
        assert!(format_matches("a.deb", b"!<arch>\ndebian"));
        assert!(!format_matches("a.msi", b"<html>"));
        assert!(!format_matches("a.exe", b"MZ"));
    }

    /// The latest published release, end to end: every package downloads and passes the checks
    /// (GitHub's SHA-256, format, Ed25519 signature). `cargo test -p rdm -- --ignored published`
    #[tokio::test]
    #[ignore = "network: downloads the published release"]
    async fn published_release_packages_verify() {
        let client = reqwest::Client::builder().user_agent("rdm-test").build().unwrap();
        let release = latest(&client).await.unwrap().expect("a published release");
        let version = release.tag_name.trim_start_matches(['v', 'V']).to_owned();
        for method in [Method::Msi, Method::Binary, Method::Deb, Method::Rpm] {
            let package = package(&release.assets, method).unwrap_or_else(|| panic!("{method:?}: no signed package"));
            let file = download(&client, &version, &package, |_| {}).await.unwrap_or_else(|e| panic!("{method:?}: {e}"));
            println!("{method:?}: {} verified", package.name);
            let _ = std::fs::remove_file(file.path);
        }
        // Replayed as another version, the same packages are refused.
        let msi = package(&release.assets, Method::Msi).unwrap();
        assert!(download(&client, "99.0.0", &msi, |_| {}).await.is_err());
    }
}
