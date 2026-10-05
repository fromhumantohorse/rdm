//! Running the module's programs: hidden, bounded in time, ended with everything they started, and
//! reaching the Internet the way RDM's downloads do.

use std::{process::Stdio, time::Duration};

use super::ModuleError;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const PROXY_VARIABLES: [&str; 6] = ["ALL_PROXY", "HTTP_PROXY", "HTTPS_PROXY", "all_proxy", "http_proxy", "https_proxy"];

pub(super) async fn run(mut command: tokio::process::Command, limit: Duration) -> Result<std::process::Output, ModuleError> {
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    // Its own process group: the programs it starts (Deno, bgutil) end with it (see `end_tree`).
    #[cfg(unix)]
    command.process_group(0);
    let child = command.spawn().map_err(ModuleError::CannotStart)?;
    let pid = child.id();
    let output = child.wait_with_output();
    tokio::pin!(output);
    tokio::select! {
        result = &mut output => result.map_err(ModuleError::Wait),
        () = tokio::time::sleep(limit) => {
            // Ended while the child still is (then by `kill_on_drop`): ending only yt-dlp would
            // leave its Deno running, hundreds of megabytes each.
            if let Some(pid) = pid {
                end_tree(pid).await;
            }
            Err(ModuleError::TooLong)
        }
    }
}

/// Ends process `pid` and every process it started.
async fn end_tree(pid: u32) {
    #[cfg(windows)]
    {
        let taskkill = std::env::var_os("SystemRoot")
            .map_or_else(|| "taskkill.exe".into(), |root| std::path::Path::new(&root).join(r"System32\taskkill.exe"));
        let pid = pid.to_string();
        let mut kill = tokio::process::Command::new(taskkill);
        kill.args(["/T", "/F", "/PID", &pid]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).creation_flags(CREATE_NO_WINDOW);
        let _ = kill.status().await;
    }
    #[cfg(unix)]
    if let Ok(group) = i32::try_from(pid) {
        // SAFETY: a plain signal to the process group the child leads (`process_group(0)`).
        unsafe {
            libc::killpg(group, libc::SIGKILL);
        }
    }
}

/// The programs reach the Internet like RDM's downloads (`route`), never around a proxy the user
/// counts on: its address in their proxy variables, or none at all for a direct connection.
pub(super) fn reach_by(command: &mut tokio::process::Command, route: &engine::Route) {
    if *route == engine::Route::System {
        return;
    }
    for name in PROXY_VARIABLES.iter().chain(&["NO_PROXY", "no_proxy"]) {
        command.env_remove(name);
    }
    if let engine::Route::Proxy { url, user, password } = route {
        let proxy = with_login(url, user, password);
        for name in PROXY_VARIABLES {
            command.env(name, &proxy);
        }
    }
}

/// The proxy's address with its login (`user:password@`), the way programs read it from their
/// environment.
fn with_login(url: &str, user: &str, password: &str) -> String {
    let Ok(mut address) = url::Url::parse(url) else { return url.to_owned() };
    if user.is_empty() || address.set_username(user).and_then(|()| address.set_password(Some(password))).is_err() {
        return url.to_owned();
    }
    address.into()
}

/// The last meaningful line of an error output (yt-dlp: `ERROR: …`).
pub(super) fn tail(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let line = text.lines().rev().find(|l| l.starts_with("ERROR")).or_else(|| text.lines().rev().find(|l| !l.trim().is_empty())).unwrap_or("failed");
    line.chars().take(300).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A module that takes too long is ended with what it started (yt-dlp's Deno): nothing is left
    /// running behind it.
    #[tokio::test]
    async fn a_timeout_ends_the_whole_process_tree() {
        let file = std::env::temp_dir().join(format!("rdm-tree-{}.pid", std::process::id()));
        let _ = std::fs::remove_file(&file);
        #[cfg(windows)]
        let command = {
            let mut c = tokio::process::Command::new("powershell");
            let script = format!(
                "$p = Start-Process -FilePath ping -ArgumentList '-n','120','127.0.0.1' -PassThru -WindowStyle Hidden; \
                 Set-Content -Path '{}' -Value $p.Id; Start-Sleep 120",
                file.display()
            );
            c.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
            c
        };
        #[cfg(unix)]
        let command = {
            let mut c = tokio::process::Command::new("sh");
            c.args(["-c", &format!("sleep 120 & echo $! > '{}'; wait", file.display())]);
            c
        };
        assert!(matches!(run(command, Duration::from_secs(6)).await, Err(ModuleError::TooLong)));
        let pid: u32 = std::fs::read_to_string(&file).expect("the child started").trim().parse().unwrap();
        let _ = std::fs::remove_file(&file);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while alive(pid) {
            assert!(std::time::Instant::now() < deadline, "the grandchild {pid} still runs");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    #[cfg(windows)]
    fn alive(pid: u32) -> bool {
        use windows_sys::Win32::{
            Foundation::{CloseHandle, STILL_ACTIVE},
            System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
        };
        // SAFETY: plain queries on a handle closed right after.
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if process.is_null() {
                return false;
            }
            let mut code = 0u32;
            let running = GetExitCodeProcess(process, &mut code) != 0 && code == STILL_ACTIVE as u32;
            CloseHandle(process);
            running
        }
    }

    #[cfg(unix)]
    fn alive(pid: u32) -> bool {
        // A process killed but not reaped yet (a zombie) is gone for this purpose.
        let state = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        !state.is_empty() && !state.rsplit(')').next().unwrap_or("").trim_start().starts_with('Z')
    }

    #[test]
    fn error_lines() {
        assert_eq!(tail(b"[youtube] x\nERROR: [youtube] x: Sign in to confirm you're not a bot\n"), "ERROR: [youtube] x: Sign in to confirm you're not a bot");
        assert_eq!(tail(b"boom\n\n"), "boom");
        assert_eq!(tail(b""), "failed");
    }

    /// The variables `reach_by` sets (`Some`) or clears (`None`) for `route`, by upper-case name.
    fn proxy_environment(route: &engine::Route) -> std::collections::HashMap<String, Option<String>> {
        let mut command = tokio::process::Command::new("yt-dlp");
        reach_by(&mut command, route);
        let std = command.as_std();
        std.get_envs().map(|(name, value)| (name.to_string_lossy().to_ascii_uppercase(), value.map(|v| v.to_string_lossy().into_owned()))).collect()
    }

    #[test]
    fn the_programs_go_through_the_proxy_of_the_settings_with_its_login() {
        let route = engine::Route::Proxy { url: "http://proxy.lan:3128".into(), user: "me".into(), password: "p@ss word".into() };

        let env = proxy_environment(&route);

        for name in ["ALL_PROXY", "HTTP_PROXY", "HTTPS_PROXY"] {
            assert_eq!(env[name].as_deref(), Some("http://me:p%40ss%20word@proxy.lan:3128/"), "{name}");
        }
        assert_eq!(env["NO_PROXY"], None, "no exception to the proxy");
    }

    #[test]
    fn a_direct_route_clears_every_proxy_variable() {
        let env = proxy_environment(&engine::Route::Direct);

        assert_eq!(env.len(), 4);
        assert!(env.values().all(Option::is_none));
    }

    #[test]
    fn the_systems_proxy_is_left_to_the_programs() {
        assert!(proxy_environment(&engine::Route::System).is_empty());
    }

    #[test]
    fn a_proxy_without_a_login_keeps_its_address() {
        assert_eq!(with_login("socks5h://proxy:1080", "", ""), "socks5h://proxy:1080");
    }
}
