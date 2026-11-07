//! Installing the module: yt-dlp's latest program, checked against its signed checksum list; Deno,
//! bgutil and `canvas` at the versions pinned here, each archive checked against its SHA-256.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use serde::Deserialize;

use super::{
    EXE, ModuleError, READY, SETUP_TIMEOUT, SIGNING_KEY, paths,
    process::{reach_by, run, tail},
};

/// bgutil is pinned: its server code runs on this computer.
const BGUTIL_TAG: &str = "2.0.0";
const BGUTIL_PLUGIN: &str = "bgutil-ytdlp-pot-provider.zip";
const BGUTIL_PLUGIN_SHA256: &str = "bce874dfa25896c2798e0f4f8147b7b22e785479eb1e459ab232bf2506c95016";
const BGUTIL_SOURCE_SHA256: &str = "e95324ee24b1b0f1b4ad43d336343afe7cf1914acdf65d9cc1977f51d7b137c2";
const BGUTIL_SOURCE_URL: &str = "https://codeload.github.com/Brainicism/bgutil-ytdlp-pot-provider/zip/refs/tags/2.0.0";

#[cfg(windows)]
pub(super) const YTDLP_ASSET: &str = "yt-dlp.exe";
#[cfg(target_os = "macos")]
pub(super) const YTDLP_ASSET: &str = "yt-dlp_macos";
#[cfg(all(unix, not(target_os = "macos")))]
pub(super) const YTDLP_ASSET: &str = "yt-dlp_linux";
/// yt-dlp's checksum list, and its detached signature.
const YTDLP_SUMS: &str = "SHA2-256SUMS";

/// Deno is pinned: it runs bgutil's code, and needs no update to follow YouTube.
const DENO_VERSION: &str = "2.9.7";
#[cfg(windows)]
const DENO_ASSET: (&str, &str) = ("deno-x86_64-pc-windows-msvc.zip", "a0c3101b4158d1dfb7d6a78a7bf0f3de80c96bb423c152beec8beb22786f2238");
// shortcut: Apple Silicon only (what RDM ships for macOS), add the x86_64 archives if it ever builds for Intel Macs.
#[cfg(target_os = "macos")]
const DENO_ASSET: (&str, &str) = ("deno-aarch64-apple-darwin.zip", "5cd46d6268f6f78f5d88bdc7159d20bd44cdaa4b3303474839f87ec6fe7ae25c");
#[cfg(all(unix, not(target_os = "macos")))]
const DENO_ASSET: (&str, &str) = ("deno-x86_64-unknown-linux-gnu.zip", "c6527f24f4b16031d3ae4fa9f658d5f11534c8d84ce7dc8502420280919c3490");

/// The native drawing library bgutil uses (through its `canvas` package), as its own release
/// publishes it; unpacked over the package in place of its install script.
const CANVAS_VERSION: &str = "3.2.3";
#[cfg(windows)]
const CANVAS_ASSET: (&str, &str) = ("canvas-v3.2.3-napi-v7-win32-x64.tar.gz", "ba953cc8c38303ab94cc83461c7561506a5a3af37d6c678de4a47914d2d0bb48");
#[cfg(target_os = "macos")]
const CANVAS_ASSET: (&str, &str) = ("canvas-v3.2.3-napi-v7-darwin-arm64.tar.gz", "38c296c9d81c05598db849fb8103543d6649fec7246c35197d9be8199c75116f");
#[cfg(all(unix, not(target_os = "macos")))]
const CANVAS_ASSET: (&str, &str) = ("canvas-v3.2.3-napi-v7-linux-x64.tar.gz", "886d1cc270d4caad1d1698ee97532e79c60e992e16fe5f37f2d3bb058f4292b0");

const MAX_FILE: u64 = 400 << 20;

#[derive(Deserialize)]
struct Release {
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

/// Installs the module into `dir` (the user's data folder; a scratch folder in tests).
pub(super) async fn setup(dir: &Path, route: &engine::Route) -> Result<(), ModuleError> {
    let p = paths(dir);
    let _ = tokio::fs::remove_file(dir.join(READY)).await;
    tokio::fs::create_dir_all(&p.plugins).await.map_err(ModuleError::CreateFolder)?;
    let client = engine::client_with(&engine::ClientOptions { route: route.clone(), ..engine::ClientOptions::default() }).map_err(ModuleError::Client)?;

    let ytdlp = signed_ytdlp(&client).await?;
    let path = p.ytdlp.clone();
    tokio::task::spawn_blocking(move || write_exe(&path, |out| std::io::Write::write_all(out, &ytdlp))).await??;

    let (name, sha256) = DENO_ASSET;
    let deno = download(&client, &format!("https://github.com/denoland/deno/releases/download/v{DENO_VERSION}/{name}"), Some(sha256)).await?;
    let path = p.deno.clone();
    tokio::task::spawn_blocking(move || unzip_exe(&deno, &format!("deno{EXE}"), &path)).await??;

    let plugin_url = format!("https://github.com/Brainicism/bgutil-ytdlp-pot-provider/releases/download/{BGUTIL_TAG}/{BGUTIL_PLUGIN}");
    let plugin = download(&client, &plugin_url, Some(BGUTIL_PLUGIN_SHA256)).await?;
    tokio::fs::write(p.plugins.join(BGUTIL_PLUGIN), &plugin).await?;

    let source = download(&client, BGUTIL_SOURCE_URL, Some(BGUTIL_SOURCE_SHA256)).await?;
    let bgutil = dir.join("bgutil");
    let _ = tokio::fs::remove_dir_all(&bgutil).await;
    let target = bgutil.clone();
    tokio::task::spawn_blocking(move || unzip_stripped(&source, &target)).await??;

    // The token generator's own libraries (npm), each checked against bgutil's lock file
    // (`--frozen`); no install script runs.
    let mut command = tokio::process::Command::new(&p.deno);
    command.args(["install", "--frozen"]).current_dir(&p.server);
    reach_by(&mut command, route);
    let out = run(command, SETUP_TIMEOUT).await?;
    if !out.status.success() {
        return Err(ModuleError::BgutilSetup(tail(&out.stderr)));
    }
    // What `canvas`'s install script would have downloaded: its library, from its own release.
    let (name, sha256) = CANVAS_ASSET;
    let canvas_url = format!("https://github.com/Automattic/node-canvas/releases/download/v{CANVAS_VERSION}/{name}");
    let library = download(&client, &canvas_url, Some(sha256)).await?;
    unpack_tar_gz(dir, &library, &p.server.join("node_modules").join("canvas")).await?;
    let ready = format!("bgutil {BGUTIL_TAG}\ndeno {DENO_VERSION}\ncanvas {CANVAS_VERSION}\n");
    Ok(tokio::fs::write(dir.join(READY), ready).await?)
}

/// yt-dlp's latest program, once its release's checksum list carries yt-dlp's signature
/// ([`SIGNING_KEY`]) and the program matches its line there.
async fn signed_ytdlp(client: &reqwest::Client) -> Result<Vec<u8>, ModuleError> {
    let release = latest_release(client, "yt-dlp/yt-dlp").await?;
    let url_of = |name: &str| {
        let asset = release.assets.iter().find(|a| a.name == name).ok_or_else(|| ModuleError::MissingAsset(name.to_owned()))?;
        if asset.browser_download_url.starts_with("https://github.com/") {
            Ok(asset.browser_download_url.clone())
        } else {
            Err(ModuleError::UnexpectedAddress(name.to_owned()))
        }
    };
    let sums = download(client, &url_of(YTDLP_SUMS)?, None).await?;
    let signature = download(client, &url_of(&format!("{YTDLP_SUMS}.sig"))?, None).await?;
    if !crate::openpgp::verify(&SIGNING_KEY, &sums, &signature) {
        return Err(ModuleError::Unsigned);
    }
    let sha256 = sum_of(&String::from_utf8_lossy(&sums), YTDLP_ASSET).ok_or(ModuleError::NotInChecksums)?;
    download(client, &url_of(YTDLP_ASSET)?, Some(&sha256)).await
}

/// The SHA-256 a `sha256sum` list (`<hex>  <name>` lines) gives for `name`.
fn sum_of(list: &str, name: &str) -> Option<String> {
    list.lines().find_map(|line| {
        let (hash, file) = line.split_once(char::is_whitespace)?;
        (file.trim_start().trim_start_matches('*') == name && hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| hash.to_ascii_lowercase())
    })
}

/// The latest release of `repo`, as GitHub's API lists it.
async fn latest_release(client: &reqwest::Client, repo: &'static str) -> Result<Release, ModuleError> {
    let res = client
        .get(format!("https://api.github.com/repos/{repo}/releases/latest"))
        .header("accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|source| ModuleError::GitHub { repo, source })?;
    let body = res.bytes().await.map_err(|source| ModuleError::GitHub { repo, source })?;
    serde_json::from_slice(&body).map_err(|source| ModuleError::GitHubAnswer { repo, source })
}

/// Unpacks a (checked) `.tar.gz` archive into `dest` with the system's `tar` (Windows 10 and
/// later ship one), through a file in `work`.
async fn unpack_tar_gz(work: &Path, archive: &[u8], dest: &Path) -> Result<(), ModuleError> {
    let file = work.join("unpack.tar.gz");
    tokio::fs::write(&file, archive).await?;
    #[cfg(windows)]
    let tar = std::env::var_os("SystemRoot").map_or_else(|| "tar.exe".into(), |root| Path::new(&root).join(r"System32\tar.exe"));
    #[cfg(not(windows))]
    let tar = PathBuf::from("tar");
    let mut command = tokio::process::Command::new(tar);
    command.arg("-xzf").arg(&file).arg("-C").arg(dest);
    let out = run(command, Duration::from_secs(120)).await;
    let _ = tokio::fs::remove_file(&file).await;
    match out {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => Err(ModuleError::Unpack { dest: dest.to_path_buf(), stderr: tail(&out.stderr) }),
        Err(e) => Err(e),
    }
}

async fn download(client: &reqwest::Client, url: &str, sha256: Option<&str>) -> Result<Vec<u8>, ModuleError> {
    use sha2::{Digest, Sha256};
    let res = client
        .get(url)
        .timeout(Duration::from_secs(600))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(ModuleError::Download)?;
    if res.content_length().is_some_and(|n| n > MAX_FILE) {
        return Err(ModuleError::TooLarge);
    }
    let bytes = res.bytes().await.map_err(ModuleError::Download)?;
    if bytes.len() as u64 > MAX_FILE {
        return Err(ModuleError::TooLarge);
    }
    let digest = crate::hex::encode(&Sha256::digest(&bytes));
    if sha256.is_some_and(|expected| !expected.eq_ignore_ascii_case(&digest)) {
        return Err(ModuleError::Corrupted(url.to_owned()));
    }
    Ok(bytes.to_vec())
}

/// Writes the program at `path` through a temporary file (`fill` writes its bytes), then puts it
/// in place: a half-written program is never left under its name.
fn write_exe(path: &Path, fill: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>) -> Result<(), ModuleError> {
    let tmp = crate::settings::with_suffix(path, ".tmp");
    let mut out = std::fs::File::create(&tmp)?;
    fill(&mut out)?;
    drop(out);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&tmp, path).map_err(ModuleError::Replace)
}

fn archive(data: &[u8]) -> Result<zip::ZipArchive<std::io::Cursor<&[u8]>>, ModuleError> {
    Ok(zip::ZipArchive::new(std::io::Cursor::new(data))?)
}

/// The one file named `name` of a zip archive, written as the program `path`: unpacked straight to
/// disk (Deno's is over 100 MB), never whole in memory.
fn unzip_exe(data: &[u8], name: &str, path: &Path) -> Result<(), ModuleError> {
    let mut zip = archive(data)?;
    let mut entry = zip.by_name(name).map_err(|source| ModuleError::MissingEntry { name: name.to_owned(), source })?;
    write_exe(path, |out| std::io::copy(&mut entry, out).map(drop))
}

/// A GitHub source archive into `dest`, without its top folder (`<repo>-<tag>/`). Paths that
/// would leave `dest` (`..`, absolute) are refused.
fn unzip_stripped(data: &[u8], dest: &Path) -> Result<(), ModuleError> {
    let mut zip = archive(data)?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let Some(inner) = entry.enclosed_name() else { return Err(ModuleError::UnsafePath) };
        let inner: PathBuf = inner.components().skip(1).collect();
        if inner.as_os_str().is_empty() {
            continue;
        }
        let path = dest.join(inner);
        if entry.is_dir() {
            std::fs::create_dir_all(&path)?;
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = std::fs::File::create(&path)?;
        std::io::copy(&mut entry, &mut out)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_is_unpacked_into_place() {
        let dir = std::env::temp_dir().join(format!("rdm-unzip-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("deno");
        let data = crate::extension::zip([("deno", b"\x7fELF program".as_slice()), ("README.md", b"docs".as_slice())].into_iter());
        unzip_exe(&data, "deno", &path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"\x7fELF program");
        assert!(!crate::settings::with_suffix(&path, ".tmp").exists(), "nothing left beside it");
        assert!(matches!(unzip_exe(&data, "missing", &path), Err(ModuleError::MissingEntry { .. })));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The program's line in yt-dlp's signed list (the real one of 2026.08.19).
    #[test]
    fn reads_the_signed_checksum_list() {
        let list = include_str!("../../testdata/yt-dlp-2026.08.19-SHA2-256SUMS");
        assert_eq!(sum_of(list, "yt-dlp.exe").as_deref(), Some("66674953fe251b89f4d08c5f0e35e0728679bd67ab3d7d05c0562af101dd3e7a"));
        assert_eq!(sum_of(list, "yt-dlp_linux").as_deref(), Some("58162f9bfdc27458ea47bfcb311cf47028f17d8154a8bf7d689861d46399230a"));
        assert_eq!(sum_of(list, "yt-dlp_macos").as_deref(), Some("0f192b7ec147ab6288885d6351d9ab67367640029b4377576ef46dd79cf7b202"));
        assert_eq!(sum_of(list, "yt-dlp"), Some(sum_of(list, "yt-dlp").unwrap()), "exact names only");
        assert!(sum_of(list, "yt-dlp_linux.zip").is_some_and(|s| Some(s) != sum_of(list, "yt-dlp_linux")));
        assert_eq!(sum_of(list, "missing.exe"), None);
        assert_eq!(sum_of("nothex  yt-dlp.exe\n", "yt-dlp.exe"), None);
    }
}
