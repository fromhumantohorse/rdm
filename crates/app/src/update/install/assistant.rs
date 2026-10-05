//! Windows: the installation assistant, a copy of `rdm.exe` that waits for RDM to be gone, runs
//! `msiexec` without any window, then makes sure RDM runs again.

use std::{
    ffi::OsString,
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    os::windows::{ffi::OsStringExt, process::CommandExt},
    path::{Path, PathBuf},
    process::Command,
    thread::sleep,
    time::{Duration, Instant},
};

use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, WAIT_TIMEOUT},
    System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
        QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject,
    },
    UI::WindowsAndMessaging::{IDYES, MB_ICONWARNING, MB_SETFOREGROUND, MB_YESNO, MessageBoxW},
};

use crate::{settings::BRIDGE_PORT, tr, trf};

/// Outside a job that would end it together with RDM (when the job allows it).
const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
/// How long RDM gets to close cleanly (downloads saved) before the assistant ends it.
const CLOSE_GRACE: Duration = Duration::from_secs(30);
/// Another installation running (Windows Update…): the installer is retried for this long.
const BUSY_RETRIES: u32 = 30;

pub fn spawn_outliving(command: &mut Command) -> std::io::Result<()> {
    match command.creation_flags(CREATE_BREAKAWAY_FROM_JOB).spawn() {
        Ok(_) => Ok(()),
        Err(_) => command.creation_flags(0).spawn().map(drop),
    }
}

pub fn apply(msi: &Path, pid: u32, exe: &Path, sha256: &str) {
    wait_for_exit(pid, exe);
    // The single-instance lock (the bridge port) is the last thing RDM lets go.
    wait_until(Duration::from_secs(15), || TcpListener::bind((Ipv4Addr::LOCALHOST, BRIDGE_PORT)).is_ok());

    // Still the package RDM verified (its signature covers this SHA-256)? Never install another.
    if !super::unchanged(msi, sha256) {
        crate::notify::fatal(tr!(
            "La mise à jour de RDM a été modifiée après sa vérification : elle n'est pas installée. RDM redémarre dans sa version actuelle.",
            "RDM's update changed after it was verified: it is not installed. RDM restarts in its current version."
        ));
        let _ = Command::new(exe).spawn();
        return;
    }
    let log = msi.with_extension("log");
    // Silent: no window at all; RDM's own card said "installing", and RDM comes back.
    let code = msiexec(&["/i".as_ref(), msi.as_os_str(), "/qn".as_ref(), "/norestart".as_ref(), "/l*v".as_ref(), log.as_os_str()]);
    // 3010 / 1641: installed, a restart completes it.
    if !matches!(code, Some(0 | 3010 | 1641)) {
        let reason = match code {
            Some(1602) => tr!("installation annulée", "installation cancelled").to_owned(),
            Some(1603) => tr!("erreur de Windows Installer (1603)", "Windows Installer error (1603)").to_owned(),
            Some(1618) => tr!("une autre installation est en cours (1618)", "another installation is in progress (1618)").to_owned(),
            Some(c) => trf!("code {c}", "code {c}", c = c),
            None => tr!("Windows Installer n'a pas pu être lancé", "Windows Installer could not be started").to_owned(),
        };
        let log = log.display();
        let text = trf!(
            "La mise à jour de RDM n'a pas pu s'installer : {reason}.\n\nOuvrir l'installateur pour réessayer ? (Sinon, RDM redémarre dans sa version actuelle.)\n\nJournal : {log}",
            "RDM's update could not be installed: {reason}.\n\nOpen the installer to try again? (Otherwise RDM restarts in its current version.)\n\nLog: {log}",
            log = log,
            reason = reason
        );
        if ask(&text) {
            msiexec(&["/i".as_ref(), msi.as_os_str()]);
        }
    }
    // The installer starts RDM itself when it succeeds; if it did not (or it failed), here.
    if !wait_until(Duration::from_secs(12), rdm_running) {
        let _ = Command::new(exe).spawn();
    }
}

/// Waits for RDM (`pid`, running `exe`) to exit; ends it if it does not within `CLOSE_GRACE`.
/// A process of that number running anything else is left alone: RDM is gone already, and
/// Windows gave its number to another program.
fn wait_for_exit(pid: u32, exe: &Path) {
    // SAFETY: plain Win32 calls on a handle we own and close.
    unsafe {
        let access = PROCESS_SYNCHRONIZE | PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION;
        let process = OpenProcess(access, 0, pid);
        if process.is_null() {
            return; // already gone
        }
        let still_rdm = image_of(process).is_some_and(|image| crate::update::detect::same_file(&image, exe));
        if still_rdm && WaitForSingleObject(process, CLOSE_GRACE.as_millis() as u32) == WAIT_TIMEOUT {
            TerminateProcess(process, 1);
            WaitForSingleObject(process, 10_000);
        }
        CloseHandle(process);
    }
}

/// The executable `process` runs.
///
/// # Safety
/// `process` is a valid handle opened with `PROCESS_QUERY_LIMITED_INFORMATION`.
unsafe fn image_of(process: HANDLE) -> Option<PathBuf> {
    let mut buffer = vec![0u16; 32_768];
    let mut len = u32::try_from(buffer.len()).ok()?;
    // SAFETY: `buffer` holds `len` UTF-16 units; on success `len` is the length written.
    let ok = unsafe { QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, buffer.as_mut_ptr(), &mut len) };
    let written = buffer.get(..usize::try_from(len).ok()?)?;
    (ok != 0).then(|| PathBuf::from(OsString::from_wide(written)))
}

/// `msiexec` with `args`, waited for; retried while another installation is running. `None`
/// when it could not be started.
fn msiexec(args: &[&std::ffi::OsStr]) -> Option<i32> {
    let exe = std::env::var_os("SystemRoot").map_or_else(|| "msiexec.exe".into(), |root| Path::new(&root).join(r"System32\msiexec.exe"));
    for _ in 0..BUSY_RETRIES {
        let code = Command::new(&exe).args(args).status().ok()?.code();
        if code != Some(1618) {
            return code;
        }
        sleep(Duration::from_secs(10));
    }
    Some(1618)
}

fn rdm_running() -> bool {
    TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, BRIDGE_PORT)), Duration::from_millis(300)).is_ok()
}

fn wait_until(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    loop {
        if done() {
            return true;
        }
        if start.elapsed() >= limit {
            return false;
        }
        sleep(Duration::from_millis(250));
    }
}

fn ask(text: &str) -> bool {
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let (text, title) = (wide(text), wide(tr!("RDM — mise à jour", "RDM — update")));
    // SAFETY: NUL-terminated UTF-16 strings that outlive the call; no owner window.
    unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_YESNO | MB_ICONWARNING | MB_SETFOREGROUND) == IDYES }
}
