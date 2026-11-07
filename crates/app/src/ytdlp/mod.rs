//! YouTube module: yt-dlp, with Deno (YouTube's JavaScript challenges) and bgutil (the anti-bot
//! "proof of origin" token, generated locally by YouTube's own BotGuard code) — for the videos
//! whose links YouTube refuses to hand to the extension's clients.
//!
//! Installed on request only, in the user's own data folder (no administrator rights), from the
//! projects' GitHub releases, and nothing runs that was not checked first:
//! - yt-dlp, its latest release (YouTube changes often; brought up to date when YouTube refuses
//!   it): its checksum list must carry the signature of yt-dlp's own key (written below), and the
//!   program must match it — a GitHub account taken over cannot slip in another program;
//! - Deno, bgutil and the drawing library bgutil needs (`canvas`, a native library): exact
//!   versions, each archive checked against the SHA-256 written in `setup`. No package's own
//!   install script runs (`canvas`'s used to download its library unchecked).
//!
//! yt-dlp only reads the link list (`-J`): RDM downloads the streams itself, like any other link.

mod error;
mod extract;
mod process;
mod setup;

use std::{
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use serde::Serialize;

pub use error::ModuleError;
pub use extract::{Formats, extract, valid_id};

/// yt-dlp's release signing key (RSA 4096, fingerprint AC0C BBE6 848D 6A87 3464 AF4E 57CF 6593
/// 3B5A 7581): its repository's `public.key`, unchanged since 2023, the same on keys.openpgp.org.
pub(crate) const SIGNING_KEY: crate::openpgp::RsaKey = crate::openpgp::RsaKey { n: &YTDLP_MODULUS, e: &[1, 0, 1] };
const YTDLP_MODULUS: [u8; 512] = crate::hex::decode(concat!(
    "f4ac5f738c63c0b74b6196de42d5e6f371c0155fb35bd6ff9ea908e4f961197531135428375803399e002b947766fdf66acd480efa9018ac1eb418a7a0edf7d4",
    "0dff92d1714a11975e44b61e01aaaa2a14a26d56a50e3bce04ecedbb75bad3b1cb113400a80ef04cfb88b765aa9a92fad0d21ccabdbb2f1aa681d37798dbd9da",
    "22cef9b3f49ca47f0be3e013d1f8cadcb00dad76cccc7a3e66429d538e3aee97644166754c71750f5f143a9df4b701298d8b0ce712628d6c698d185a26561edc",
    "d5a19bb590ece39d2e458eaff204724b2260c7ee93711e6eecfcf9dababa66405b4316bba4b7eda58439e9294986aff23fdc81ea1ff5fef673e84b8ba2a90e47",
    "a5b941f012bee167e96a2120ef92e7bee38f7d920f298c976bfd00ed0a2333080eac3f3ca334fe54290a92427e8a35d9ee01cd8e670169474a2bb7c320439823",
    "adcbc9ce905a41904e28057df4945d61c308ef5a5cd84fdde8ea15680632d094b2b6e4709916b9663bab9011cbfd391db48cdcae0353d7aab67c66895d5643c9",
    "d7b88f49809411be1e4f4e49f2e4bb71ab3726aaf58ef538c705baecd7804f10ef5f717c4a2ff08505bcd70d2bef02002b76aaa53289dc43fca271395f2faab3",
    "27c10a162a96d7840c4dd027e92bae653a1d5c24e2227c284fb5a38ee14f28a76f7c7d463f86e7a9dcd39f32f10920c812d7f27cfb085fef4623b26493fcd98d",
));

const EXE: &str = std::env::consts::EXE_SUFFIX;
const SETUP_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const REFRESH_EVERY: Duration = Duration::from_secs(24 * 3600);
/// Written last: the module is complete.
const READY: &str = "ready";

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase", tag = "state", content = "error")]
pub enum State {
    Missing,
    Installing,
    Ready,
    /// The last installation failed (why, in English: shown as is in the extension).
    Failed(String),
}

static INSTALLING: Mutex<Option<State>> = Mutex::new(None);

fn tools_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("org", "rdm", "rdm").map(|d| d.data_local_dir().join("youtube"))
}

struct Paths {
    ytdlp: PathBuf,
    deno: PathBuf,
    plugins: PathBuf,
    server: PathBuf,
}

fn paths(dir: &Path) -> Paths {
    Paths {
        ytdlp: dir.join(format!("yt-dlp{EXE}")),
        deno: dir.join(format!("deno{EXE}")),
        plugins: dir.join("plugins"),
        server: dir.join("bgutil").join("server"),
    }
}

pub fn state() -> State {
    let current = INSTALLING.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
    match current {
        Some(state) => state,
        None if tools_dir().is_some_and(|d| d.join(READY).is_file()) => State::Ready,
        None => State::Missing,
    }
}

/// Starts the installation in the background (nothing when one runs or the module is there),
/// reaching GitHub by `route`.
pub fn install(rt: &tokio::runtime::Handle, route: engine::Route) {
    {
        let mut current = INSTALLING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if matches!(*current, Some(State::Installing)) {
            return;
        }
        if current.is_none() && tools_dir().is_some_and(|d| d.join(READY).is_file()) {
            return;
        }
        *current = Some(State::Installing);
    }
    rt.spawn(async move {
        let setup = async { setup::setup(&tools_dir().ok_or(ModuleError::NoDataFolder)?, &route).await };
        let result = match tokio::time::timeout(SETUP_TIMEOUT, setup).await {
            Ok(result) => result,
            Err(_) => Err(ModuleError::SetupTooLong),
        };
        let mut current = INSTALLING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        *current = match result {
            Ok(()) => None, // `ready` is on disk
            Err(e) => Some(State::Failed(e.to_string())),
        };
    });
}

/// YouTube refused the module's yt-dlp, which may be too old by now (YouTube changes often, and the
/// module is installed once): its latest program, checked like at installation, for the next try.
pub fn refresh(rt: &tokio::runtime::Handle, route: engine::Route) {
    static RUNNING: AtomicBool = AtomicBool::new(false);
    let Some(path) = tools_dir().map(|dir| paths(&dir).ytdlp) else { return };
    if !refresh_due(&path) || RUNNING.swap(true, Ordering::AcqRel) {
        return;
    }
    rt.spawn(async move {
        // A failure (no network, the program in use) is tried again at the next refusal.
        let _ = tokio::time::timeout(SETUP_TIMEOUT, setup::refresh(&path, &route)).await;
        RUNNING.store(false, Ordering::Release);
    });
}

/// Not more than once a day: YouTube refuses for other reasons too.
fn refresh_due(program: &Path) -> bool {
    !std::fs::metadata(program).and_then(|m| m.modified()).is_ok_and(|at| at.elapsed().is_ok_and(|age| age < REFRESH_EVERY))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yt_dlp_is_refreshed_once_a_day_at_most() {
        let dir = std::env::temp_dir().join(format!("rdm-refresh-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let program = dir.join("yt-dlp");
        assert!(refresh_due(&program), "missing");

        let file = std::fs::File::create(&program).unwrap();
        assert!(!refresh_due(&program), "just written");

        file.set_modified(std::time::SystemTime::now() - REFRESH_EVERY - Duration::from_secs(60)).unwrap();
        drop(file);
        assert!(refresh_due(&program), "a day old");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// The real thing: installs the module (every check included) in a scratch folder and reads a
/// video's links (network, minutes): `cargo test -p rdm ytdlp::live -- --ignored --nocapture`.
#[cfg(test)]
mod live {
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "network: downloads the YouTube module"]
    async fn install_and_extract() {
        let dir = std::env::temp_dir().join(format!("rdm-youtube-{}", std::process::id()));
        super::setup::setup(&dir, &engine::Route::System).await.expect("installation");
        assert!(dir.join(super::READY).is_file());
        // bgutil's native drawing library loads (unpacked by RDM, not by its install script).
        let p = super::paths(&dir);
        let mut deno = tokio::process::Command::new(&p.deno);
        let script = "import { createCanvas } from 'canvas'; createCanvas(4, 4).getContext('2d'); console.log('canvas ok')";
        deno.args(["eval", script]).current_dir(&p.server);
        let out = super::process::run(deno, std::time::Duration::from_secs(120)).await.expect("deno runs");
        assert!(String::from_utf8_lossy(&out.stdout).contains("canvas ok"), "{}", String::from_utf8_lossy(&out.stderr));
        let f = super::extract::extract_in(&dir, "jNQXAC9IVRw", &engine::Route::System).await.expect("extraction");
        println!("{} — {} links, UA {}", f.title, f.formats.len(), f.ua);
        assert!(f.formats.iter().any(|x| x.audio && !x.video) && f.formats.iter().any(|x| x.video));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
