//! Native messaging: the browsers start `rdm` itself as the extension's connector ("native
//! host") and talk to it over stdin/stdout. Compared with the extension calling the loopback
//! bridge directly:
//! - the browser vouches for the caller — only the RDM extension (its ID, listed in the host's
//!   manifest) can start the connector — so Firefox and its derivatives (whose extension origin is
//!   random) need no approval in RDM's window: the connector pairs them;
//! - RDM does not have to be running: an explicit request (a link sent from the page, the toolbar
//!   button) starts it.
//!
//! The connector only relays a fixed set of requests to the bridge on 127.0.0.1, as a local
//! program (see `bridge`), and answers each with its status and body. The extension falls back to
//! the bridge itself when the connector is missing (a Flatpak or Snap browser cannot start it).
//!
//! Framing (both ways): a 32-bit length in native byte order, then that much UTF-8 JSON.

mod register;

use std::{
    io::{self, BufReader, Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpStream},
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::settings::BRIDGE_PORT;
pub use register::register;

/// The connector's name, as the extension asks for it (`runtime.connectNative`).
pub const HOST: &str = "rdm.bridge";
/// The RDM extension, the only one allowed to start the connector.
const CHROME_ORIGIN: &str = "chrome-extension://cgailhenfaoohkakpdacohcnmppepjjl/";
const CHROME_STORE_ORIGIN: &str = "chrome-extension://adoaoaomelojpakojcmkaclkhopkeljp/";
pub(crate) const FIREFOX_ID: &str = "rdm@rdm-download-manager";

/// Browsers cap what a host may send at 1 MiB; requests from the extension are small.
const MAX_REPLY: usize = 1 << 20;
const MAX_REQUEST: u32 = 256 * 1024;
const MAX_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a starting RDM gets to open its bridge.
const START_WAIT: Duration = Duration::from_secs(20);
/// Requests handled at once; more wait for their turn (a burst of links, a slow probe).
const MAX_INFLIGHT: usize = 8;
/// A running RDM's port takes a connection at once. Where nothing listens, Linux and macOS refuse at
/// once, but Windows tries again for 2 s first: not worth waiting for (see `connect`).
const CONNECT_WAIT: Duration = Duration::from_secs(1);
/// A new RDM's port answers a moment before its token is written: refused once, then asked again.
const TOKEN_WAIT: Duration = Duration::from_millis(500);

/// Whether the browser started this process as the connector: Chrome and its derivatives pass
/// the caller's origin (plus `--parent-window=…` on Windows), Firefox the manifest's path and the
/// extension's ID.
pub fn is_host(args: &[String]) -> bool {
    match args {
        [origin, ..] if origin.starts_with("chrome-extension://") => true,
        [manifest, id, ..] => id == FIREFOX_ID || manifest.ends_with(&format!("{HOST}.json")),
        _ => false,
    }
}

#[derive(Deserialize)]
struct Request {
    id: u64,
    #[serde(default)]
    method: Method,
    path: String,
    #[serde(default)]
    body: Option<Value>,
    /// Milliseconds.
    #[serde(default)]
    timeout: Option<u64>,
    /// Which browser this is (`x-rdm-browser`).
    #[serde(default)]
    browser: Option<String>,
}

#[derive(Deserialize, Default, Clone, Copy, PartialEq, Eq)]
enum Method {
    #[default]
    #[serde(rename = "GET")]
    Get,
    #[serde(rename = "POST")]
    Post,
}

#[derive(Serialize)]
struct Reply {
    id: u64,
    /// The bridge's HTTP status; 0 = RDM is not running, 1 = no answer in time, 2 = refused here.
    status: u16,
    body: String,
}

const NOT_RUNNING: u16 = 0;
const TIMED_OUT: u16 = 1;
const REFUSED: u16 = 2;

/// What the extension may ask through the connector, and whether it starts RDM when absent.
fn route(method: Method, path: &str) -> Option<bool> {
    match (method, path) {
        (Method::Get | Method::Post, "/ping") | (Method::Get, "/config") => Some(false),
        (Method::Post, "/probe" | "/check" | "/pair" | "/installed" | "/uninstalled" | "/youtube/state" | "/youtube/install" | "/youtube/extract") => Some(false),
        // Explicit actions of the user: RDM starts if needed.
        (Method::Post, "/add" | "/show" | "/record/start") => Some(true),
        _ => None,
    }
}

/// The connector: answers the browser until it closes the connection.
pub fn run_host() {
    let stdout = Arc::new(Mutex::new(io::stdout()));
    let slots = Arc::new((Mutex::new(0usize), std::sync::Condvar::new()));
    let mut input = BufReader::new(io::stdin());
    while let Some(message) = read_message(&mut input) {
        let Ok(request) = serde_json::from_slice::<Request>(&message) else { continue };
        // Bounded concurrency: each request runs on its own thread, at most `MAX_INFLIGHT` at once.
        {
            let (count, freed) = &*slots;
            let mut n = count.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            while *n >= MAX_INFLIGHT {
                n = freed.wait(n).unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            *n += 1;
        }
        let (stdout, slots) = (stdout.clone(), slots.clone());
        std::thread::spawn(move || {
            let id = request.id;
            let reply = handle(request);
            if let Ok(json) = serde_json::to_vec(&reply).map(|json| fit(id, json)) {
                let mut out = stdout.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let _ = write_message(&mut *out, &json);
            }
            let (count, freed) = &*slots;
            *count.lock().unwrap_or_else(std::sync::PoisonError::into_inner) -= 1;
            freed.notify_one();
        });
    }
}

fn read_message(input: &mut impl Read) -> Option<Vec<u8>> {
    let mut len = [0u8; 4];
    input.read_exact(&mut len).ok()?;
    let len = u32::from_ne_bytes(len);
    if len > MAX_REQUEST {
        return None; // not the extension: stop
    }
    let mut message = vec![0u8; len as usize];
    input.read_exact(&mut message).ok()?;
    Some(message)
}

/// A reply the browser accepts: it ends the whole connection on a message over 1 MB (JSON escaping
/// can make a body under `MAX_REPLY` exceed it). Too large: "refused here", and the extension
/// asks the bridge itself.
fn fit(id: u64, json: Vec<u8>) -> Vec<u8> {
    const BROWSER_LIMIT: usize = 1024 * 1024;
    if json.len() <= BROWSER_LIMIT {
        return json;
    }
    serde_json::to_vec(&Reply { id, status: REFUSED, body: String::new() }).unwrap_or_default()
}

fn write_message(out: &mut impl Write, json: &[u8]) -> io::Result<()> {
    out.write_all(&(json.len() as u32).to_ne_bytes())?;
    out.write_all(json)?;
    out.flush()
}

fn handle(req: Request) -> Reply {
    let refused = |id| Reply { id, status: REFUSED, body: String::new() };
    let Some(starts_rdm) = route(req.method, &req.path) else { return refused(req.id) };
    let browser = req.browser.filter(|b| (1..=16).contains(&b.len()) && b.bytes().all(|c| c.is_ascii_lowercase()));
    let body = match (&req.body, req.method) {
        (Some(v), Method::Post) => match serde_json::to_vec(v) {
            Ok(b) => Some(b),
            Err(_) => return refused(req.id),
        },
        _ => None,
    };
    let timeout = req.timeout.map_or(Duration::from_secs(5), Duration::from_millis).clamp(Duration::from_millis(500), MAX_TIMEOUT);
    if starts_rdm {
        // Started by the browser the user is in, the connector may hand RDM the foreground:
        // without it Windows keeps RDM's window (a download to confirm) behind the browser.
        crate::window::allow_foreground_handoff();
    }
    let call = || bridge_call(req.method, &req.path, body.as_deref(), browser.as_deref(), timeout);
    let result = match call() {
        Err(e) if e.kind() == io::ErrorKind::ConnectionRefused && starts_rdm && start_rdm(&req.path) => match call() {
            Ok((403, _)) => {
                std::thread::sleep(TOKEN_WAIT);
                call()
            }
            other => other,
        },
        other => other,
    };
    match result {
        Ok((status, body)) => Reply { id: req.id, status, body: String::from_utf8_lossy(&body).into_owned() },
        Err(e) if matches!(e.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock) => Reply { id: req.id, status: TIMED_OUT, body: String::new() },
        Err(_) => Reply { id: req.id, status: NOT_RUNNING, body: String::new() },
    }
}

fn bridge_addr() -> SocketAddr {
    SocketAddr::from((Ipv4Addr::LOCALHOST, BRIDGE_PORT))
}

/// A connection to a port of this computer. Waiting for it longer than `CONNECT_WAIT` means nothing
/// listens: said as a refusal, which is what starts RDM (a timeout on Windows, where it takes 2 s).
fn connect(addr: &SocketAddr) -> io::Result<TcpStream> {
    TcpStream::connect_timeout(addr, CONNECT_WAIT).map_err(|e| if e.kind() == io::ErrorKind::TimedOut { io::ErrorKind::ConnectionRefused.into() } else { e })
}

/// One HTTP/1.1 request to the bridge (`Connection: close`): status and body.
fn bridge_call(method: Method, path: &str, body: Option<&[u8]>, browser: Option<&str>, timeout: Duration) -> io::Result<(u16, Vec<u8>)> {
    // The page's cookies go only to the user's own RDM: never to another account's program
    // holding the port (it would pose as RDM). Reported as "RDM is not running".
    if !crate::local::bridge_is_ours() {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    let mut stream = connect(&bridge_addr())?;
    stream.set_nodelay(true)?;
    stream.set_write_timeout(Some(timeout))?;
    stream.set_read_timeout(Some(timeout))?;
    let verb = if method == Method::Post { "POST" } else { "GET" };
    let mut head = format!("{verb} {path} HTTP/1.1\r\nHost: 127.0.0.1:{BRIDGE_PORT}\r\nConnection: close\r\n");
    if let Some(browser) = browser {
        head.push_str(&format!("x-rdm-browser: {browser}\r\n"));
    }
    // The connector is the user's: it proves it with the token RDM wrote in the user's files.
    if let Some(token) = crate::local::read_token() {
        head.push_str(&format!("{}: {token}\r\n", crate::local::TOKEN_HEADER));
    }
    if let Some(body) = body {
        head.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n", body.len()));
    } else if method == Method::Post {
        head.push_str("Content-Length: 0\r\n");
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    if let Some(body) = body {
        stream.write_all(body)?;
    }
    let deadline = Instant::now() + timeout;
    let mut response = Vec::new();
    let mut buf = [0u8; 16 * 1024];
    loop {
        if Instant::now() > deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                response.extend_from_slice(&buf[..n]);
                if response.len() > MAX_REPLY {
                    return Err(io::ErrorKind::InvalidData.into());
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    parse_response(&response).ok_or_else(|| io::ErrorKind::InvalidData.into())
}

/// Status and body of a complete HTTP/1.1 response (plain or chunked body).
fn parse_response(response: &[u8]) -> Option<(u16, Vec<u8>)> {
    let split = response.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&response[..split]).ok()?;
    let body = &response[split + 4..];
    let mut lines = head.split("\r\n");
    let status: u16 = lines.next()?.split(' ').nth(1)?.parse().ok()?;
    let chunked = lines.any(|l| {
        l.split_once(':').is_some_and(|(k, v)| k.trim().eq_ignore_ascii_case("transfer-encoding") && v.to_ascii_lowercase().contains("chunked"))
    });
    Some((status, if chunked { dechunk(body)? } else { body.to_vec() }))
}

fn dechunk(mut body: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let line_end = body.windows(2).position(|w| w == b"\r\n")?;
        let size_text = std::str::from_utf8(&body[..line_end]).ok()?;
        let size = usize::from_str_radix(size_text.split(';').next()?.trim(), 16).ok()?;
        body = &body[line_end + 2..];
        if size == 0 {
            return Some(out);
        }
        out.extend_from_slice(body.get(..size)?);
        body = body.get(size + 2..)?;
    }
}

/// Starts RDM (the executable this connector runs from) and waits for its bridge. Another request
/// may have started it meanwhile: only one start at a time.
fn start_rdm(path: &str) -> bool {
    static STARTING: Mutex<()> = Mutex::new(());
    let _one = STARTING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let running = || TcpStream::connect_timeout(&bridge_addr(), Duration::from_millis(300)).is_ok();
    if running() {
        return true;
    }
    let Ok(exe) = std::env::current_exe() else { return false };
    // A link sent from the page: RDM starts in the notification area and takes it from there;
    // the toolbar button asks for the window.
    let minimized = path != "/show";
    if spawn_detached(&exe, minimized).is_err() {
        return false;
    }
    let start = Instant::now();
    while start.elapsed() < START_WAIT {
        std::thread::sleep(Duration::from_millis(200));
        if running() {
            return true;
        }
    }
    false
}

/// Outside the browser's job (Firefox ends its connectors' jobs with it), without the connector's
/// pipes (RDM would keep them open, and the browser would never see the connector end).
#[cfg(windows)]
fn spawn_detached(exe: &Path, minimized: bool) -> io::Result<()> {
    use std::os::windows::process::CommandExt;

    use windows_sys::Win32::{
        Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation},
        System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE},
    };
    // SAFETY: plain Win32 calls on this process's own standard handles.
    unsafe {
        for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            let handle = GetStdHandle(which);
            if !handle.is_null() {
                SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0);
            }
        }
    }
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    let mut command = std::process::Command::new(exe);
    if minimized {
        command.arg(crate::autostart::MINIMIZED_FLAG);
    }
    command.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    match command.creation_flags(CREATE_BREAKAWAY_FROM_JOB).spawn() {
        Ok(_) => Ok(()),
        Err(_) => command.creation_flags(0).spawn().map(drop),
    }
}

/// In its own session: closing the browser (which ends the connector's process group) must not
/// take RDM with it.
#[cfg(not(windows))]
fn spawn_detached(exe: &Path, minimized: bool) -> io::Result<()> {
    use std::os::unix::process::CommandExt;

    // macOS: an application in its bundle starts through the system. As the browser's child, the
    // system would ask the user's permissions (the Downloads folder…) in the browser's name.
    #[cfg(target_os = "macos")]
    if let Some(bundle) = exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")) {
        let mut open = std::process::Command::new("/usr/bin/open");
        open.arg("-a").arg(bundle);
        if minimized {
            open.args(["--args", crate::autostart::MINIMIZED_FLAG]);
        }
        return open.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().and_then(|s| {
            if s.success() { Ok(()) } else { Err(io::Error::other("open failed")) }
        });
    }

    let mut command = std::process::Command::new(exe);
    if minimized {
        command.arg(crate::autostart::MINIMIZED_FLAG);
    }
    command.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    // SAFETY: `setsid` is async-signal-safe and touches nothing of the parent.
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = command.spawn()?;
    // Reaped when it ends (the connector usually ends first; then init adopts it).
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_how_browsers_start_the_connector() {
        let args = |a: &[&str]| a.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert!(is_host(&args(&["chrome-extension://cgailhenfaoohkakpdacohcnmppepjjl/", "--parent-window=0"])));
        assert!(is_host(&args(&["C:\\x\\rdm.bridge.json", "rdm@rdm-download-manager"])));
        assert!(!is_host(&args(&["--minimized"])));
        assert!(!is_host(&args(&["https://example.com/file.zip"])));
        assert!(!is_host(&args(&[])));
    }

    /// Regression: on Windows a refused connection took longer than the wait, read as "RDM does not
    /// answer" (nothing to start) instead of "RDM is not running": a link from the page was lost.
    #[test]
    fn a_port_nothing_listens_on_is_refused_without_waiting_long() {
        let free = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).and_then(|l| l.local_addr()).unwrap();

        let started = Instant::now();
        let error = connect(&free).expect_err("nothing listens there");

        assert_eq!(error.kind(), io::ErrorKind::ConnectionRefused);
        assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
    }

    #[test]
    fn only_known_requests_are_relayed() {
        assert_eq!(route(Method::Post, "/add"), Some(true));
        assert_eq!(route(Method::Get, "/config"), Some(false));
        assert_eq!(route(Method::Post, "/quit"), None, "the extension cannot close RDM");
        assert_eq!(route(Method::Post, "/record/abc/append"), None);
        assert_eq!(route(Method::Get, "/add"), None);
    }

    #[test]
    fn framing_round_trips() {
        let mut buf = Vec::new();
        write_message(&mut buf, br#"{"id":1}"#).unwrap();
        assert_eq!(read_message(&mut buf.as_slice()).unwrap(), br#"{"id":1}"#);
        let mut huge = (MAX_REQUEST + 1).to_ne_bytes().to_vec();
        huge.extend_from_slice(b"xx");
        assert!(read_message(&mut huge.as_slice()).is_none(), "oversized: the connector stops");
    }

    #[test]
    fn replies_stay_within_what_browsers_accept() {
        let small = serde_json::to_vec(&Reply { id: 7, status: 200, body: "rdm".into() }).unwrap();
        assert_eq!(fit(7, small.clone()), small);
        // Quotes are escaped: a body under MAX_REPLY grows past the browsers' 1 MB.
        let big = serde_json::to_vec(&Reply { id: 7, status: 200, body: "\"".repeat(MAX_REPLY - 100) }).unwrap();
        let reply: Value = serde_json::from_slice(&fit(7, big)).unwrap();
        assert_eq!((reply["id"].as_u64(), reply["status"].as_u64()), (Some(7), Some(u64::from(REFUSED))));
    }

    #[test]
    fn parses_plain_and_chunked_responses() {
        let plain = b"HTTP/1.1 202 Accepted\r\ncontent-length: 2\r\n\r\nok";
        assert_eq!(parse_response(plain), Some((202, b"ok".to_vec())));
        let chunked = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nrdm \r\n3\r\n0.3\r\n0\r\n\r\n";
        assert_eq!(parse_response(chunked), Some((200, b"rdm 0.3".to_vec())));
        assert_eq!(parse_response(b"garbage"), None);
    }
}
