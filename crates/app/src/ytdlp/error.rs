//! Why installing or running the module failed, in English: shown as is in the extension.

use std::path::PathBuf;

use super::setup::YTDLP_ASSET;

#[derive(Debug, thiserror::Error)]
pub enum ModuleError {
    #[error("no user data folder")]
    NoDataFolder,
    #[error("installation took too long")]
    SetupTooLong,
    #[error("cannot create the folder: {0}")]
    CreateFolder(#[source] std::io::Error),
    #[error("{0}")]
    Client(#[source] reqwest::Error),
    #[error("{0}")]
    Task(#[from] tokio::task::JoinError),
    #[error("cannot write: {0}")]
    Write(#[from] std::io::Error),
    #[error("cannot write (is it running?): {0}")]
    Replace(#[source] std::io::Error),
    #[error("GitHub ({repo}): {source}")]
    GitHub { repo: &'static str, source: reqwest::Error },
    #[error("GitHub ({repo}): {source}")]
    GitHubAnswer { repo: &'static str, source: serde_json::Error },
    #[error("{0} not found in yt-dlp's release")]
    MissingAsset(String),
    #[error("{0}: unexpected address")]
    UnexpectedAddress(String),
    #[error("yt-dlp: the checksum list is not signed by yt-dlp's key: refused")]
    Unsigned,
    #[error("{}: not in yt-dlp's signed checksums", YTDLP_ASSET)]
    NotInChecksums,
    #[error("download failed: {0}")]
    Download(#[source] reqwest::Error),
    #[error("file abnormally large")]
    TooLarge,
    #[error("corrupted download (SHA-256 mismatch): {0}")]
    Corrupted(String),
    #[error("bad archive: {0}")]
    BadArchive(#[from] zip::result::ZipError),
    #[error("{name}: {source}")]
    MissingEntry { name: String, source: zip::result::ZipError },
    #[error("unsafe path in archive")]
    UnsafePath,
    #[error("cannot unpack {}: {stderr}", .dest.display())]
    Unpack { dest: PathBuf, stderr: String },
    #[error("bgutil setup failed: {0}")]
    BgutilSetup(String),
    #[error("invalid video")]
    InvalidVideo,
    #[error("the YouTube module is busy: try again in a moment")]
    Busy,
    /// yt-dlp's own error line.
    #[error("{0}")]
    Refused(String),
    #[error("unexpected yt-dlp output: {0}")]
    UnexpectedOutput(#[source] serde_json::Error),
    #[error("cannot start the YouTube module: {0}")]
    CannotStart(#[source] std::io::Error),
    #[error("{0}")]
    Wait(#[source] std::io::Error),
    #[error("the YouTube module took too long")]
    TooLong,
}
