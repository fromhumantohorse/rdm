//! Reading a video's links with yt-dlp (`-J`): RDM downloads the streams itself, like any other link.

use std::{path::Path, time::Duration};

use serde::Serialize;
use serde_json::Value;

use super::{
    ModuleError, paths,
    process::{reach_by, run, tail},
    tools_dir,
};

const EXTRACT_TIMEOUT: Duration = Duration::from_secs(25);
/// Extractions at once, and how long one waits for its turn.
const MAX_EXTRACTIONS: usize = 2;
const WAIT_TURN: Duration = Duration::from_secs(3);

/// A direct link, in the shape `extension/youtube.js` gives the menu.
#[derive(Debug, Serialize, PartialEq)]
pub struct Format {
    itag: String,
    url: String,
    container: String,
    pub(super) video: bool,
    pub(super) audio: bool,
    height: u64,
    fps: f64,
    label: String,
    size: u64,
    bitrate: u64,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Formats {
    pub(super) title: String,
    /// The User-Agent the links are bound to.
    pub(super) ua: String,
    pub(super) formats: Vec<Format>,
}

/// Whether `id` is a YouTube video ID (11 characters of `[A-Za-z0-9_-]`).
pub fn valid_id(id: &str) -> bool {
    id.len() == 11 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The links of video `id` (checked by [`valid_id`]), as yt-dlp resolves them reaching YouTube by
/// `route`: the links are bound to the address that reads them, so the download must take the
/// same route.
pub async fn extract(id: &str, route: &engine::Route) -> Result<Formats, ModuleError> {
    extract_in(&tools_dir().ok_or(ModuleError::NoDataFolder)?, id, route).await
}

/// [`extract`] with the module installed in `dir`.
pub(super) async fn extract_in(dir: &Path, id: &str, route: &engine::Route) -> Result<Formats, ModuleError> {
    if !valid_id(id) {
        return Err(ModuleError::InvalidVideo);
    }
    // yt-dlp and Deno take hundreds of megabytes each: a menu opened again and again must not
    // start them by the dozen.
    static RUNNING: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(MAX_EXTRACTIONS);
    let _turn = match tokio::time::timeout(WAIT_TURN, RUNNING.acquire()).await {
        Ok(Ok(permit)) => permit,
        _ => return Err(ModuleError::Busy),
    };
    let p = paths(dir);
    let mut jsrt = p.deno.into_os_string();
    jsrt = [std::ffi::OsStr::new("deno:"), &jsrt].into_iter().collect();
    let mut server_home = p.server.into_os_string();
    server_home = [std::ffi::OsStr::new("youtubepot-bgutilscript:server_home="), &server_home].into_iter().collect();
    let mut command = tokio::process::Command::new(&p.ytdlp);
    command
        .args(["-J", "--no-playlist", "--no-warnings", "--ignore-config", "--no-js-runtimes", "--js-runtimes"])
        .arg(jsrt)
        .args(["--no-plugin-dirs", "--plugin-dirs"])
        .arg(&p.plugins)
        .arg("--extractor-args")
        .arg(server_home);
    if *route == engine::Route::Direct {
        // Not the system's proxy either (yt-dlp reads Windows' settings when no variable is set).
        command.args(["--proxy", ""]);
    }
    command.arg("--").arg(format!("https://www.youtube.com/watch?v={id}"));
    reach_by(&mut command, route);
    let out = run(command, EXTRACT_TIMEOUT).await?;
    if !out.status.success() {
        return Err(ModuleError::Refused(tail(&out.stderr)));
    }
    let info: Value = serde_json::from_slice(&out.stdout).map_err(ModuleError::UnexpectedOutput)?;
    Ok(formats(&info))
}

/// The direct (`https`) links of yt-dlp's `-J` output, all bound to one User-Agent; no DRM
/// streams, no dynamic-range-compressed audio duplicates.
fn formats(info: &Value) -> Formats {
    let str_of = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
    let ua_of = |f: &Value| f.pointer("/http_headers/User-Agent").and_then(Value::as_str).unwrap_or("").to_owned();
    let all = info.get("formats").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    let direct: Vec<&Value> = all
        .iter()
        .filter(|f| {
            str_of(f, "protocol") == "https"
                && str_of(f, "url").starts_with("https://")
                && !str_of(f, "format_id").ends_with("-drc")
                && f.get("has_drm").and_then(Value::as_bool) != Some(true)
        })
        .collect();
    let ua = direct.first().map(|f| ua_of(f)).unwrap_or_default();
    let formats = direct
        .into_iter()
        .filter(|f| ua_of(f) == ua)
        .map(|f| {
            let codec = |k: &str| f.get(k).and_then(Value::as_str).is_some_and(|c| c != "none");
            let height = f.get("height").and_then(Value::as_u64).unwrap_or(0);
            let ext = str_of(f, "ext");
            Format {
                itag: str_of(f, "format_id"),
                url: str_of(f, "url"),
                container: if ext == "m4a" { "mp4".into() } else { ext },
                video: codec("vcodec"),
                audio: codec("acodec"),
                height,
                fps: f.get("fps").and_then(Value::as_f64).unwrap_or(0.0),
                label: if height > 0 { format!("{height}p") } else { String::new() },
                size: f.get("filesize").or_else(|| f.get("filesize_approx")).and_then(Value::as_f64).unwrap_or(0.0) as u64,
                bitrate: (f.get("tbr").and_then(Value::as_f64).unwrap_or(0.0) * 1000.0) as u64,
            }
        })
        .collect();
    Formats { title: str_of(info, "title"), ua, formats }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_eleven_character_video_ids_are_accepted() {
        assert!(valid_id("jNQXAC9IVRw"));
        assert!(valid_id("a-b_c123456"));
        assert!(!valid_id("jNQXAC9IVR"));
        assert!(!valid_id("jNQXAC9IVR&"));
        assert!(!valid_id("../../etc/x"));
    }

    #[test]
    fn keeps_direct_links_only() {
        let ua = "Mozilla/5.0 X";
        let info = serde_json::json!({
            "title": "Me at the zoo",
            "formats": [
                { "format_id": "233", "protocol": "m3u8_native", "url": "https://manifest.googlevideo.com/x", "ext": "mp4" },
                { "format_id": "140-drc", "protocol": "https", "url": "https://r.googlevideo.com/a", "ext": "m4a", "http_headers": { "User-Agent": ua } },
                { "format_id": "140", "protocol": "https", "url": "https://r.googlevideo.com/b", "ext": "m4a", "vcodec": "none",
                  "acodec": "mp4a.40.2", "filesize": 309288, "tbr": 129.5, "http_headers": { "User-Agent": ua } },
                { "format_id": "134", "protocol": "https", "url": "https://r.googlevideo.com/c", "ext": "mp4", "vcodec": "avc1",
                  "acodec": "none", "height": 360, "fps": 30, "filesize_approx": 1000, "http_headers": { "User-Agent": ua } },
                { "format_id": "999", "protocol": "https", "url": "https://r.googlevideo.com/d", "ext": "mp4", "has_drm": true, "http_headers": { "User-Agent": ua } },
                { "format_id": "18", "protocol": "https", "url": "https://r.googlevideo.com/e", "ext": "mp4", "http_headers": { "User-Agent": "other" } },
            ]
        });
        let f = formats(&info);
        assert_eq!(f.title, "Me at the zoo");
        assert_eq!(f.ua, ua);
        assert_eq!(f.formats.iter().map(|f| f.itag.as_str()).collect::<Vec<_>>(), ["140", "134"]);
        let audio = &f.formats[0];
        assert!(audio.audio && !audio.video && audio.container == "mp4" && audio.size == 309288 && audio.bitrate == 129500);
        let video = &f.formats[1];
        assert!(video.video && !video.audio && video.height == 360 && video.label == "360p" && video.size == 1000);
    }
}
