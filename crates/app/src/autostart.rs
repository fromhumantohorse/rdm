//! Launch at login (minimized): `HKCU\…\Run` on Windows, a LaunchAgent on macOS, XDG autostart entry
//! on Linux.

pub const MINIMIZED_FLAG: &str = "--minimized";

pub fn set(enabled: bool) -> std::io::Result<()> {
    imp::set(enabled, &std::env::current_exe()?)
}

#[cfg(windows)]
mod imp {
    use std::{io, path::Path};

    use winreg::{RegKey, enums::HKEY_CURRENT_USER};

    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

    pub fn set(enabled: bool, exe: &Path) -> io::Result<()> {
        let (run, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(RUN)?;
        if enabled {
            run.set_value("RDM", &format!("\"{}\" {}", exe.display(), super::MINIMIZED_FLAG))
        } else {
            match run.delete_value("RDM") {
                Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            }
        }
    }
}

/// The per-user agent launchd reads at the next login: no `launchctl`, nothing to do at once.
#[cfg(target_os = "macos")]
mod imp {
    use std::{fs, io, path::Path};

    pub fn set(enabled: bool, exe: &Path) -> io::Result<()> {
        let dir = directories::BaseDirs::new().ok_or(io::ErrorKind::NotFound)?.home_dir().join("Library/LaunchAgents");
        let agent = dir.join("org.rdm.rdm.plist");
        if !enabled {
            return match fs::remove_file(agent) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            };
        }
        fs::create_dir_all(&dir)?;
        fs::write(agent, super::launch_agent(&exe.to_string_lossy()))
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use std::{fs, io, path::Path};

    pub fn set(enabled: bool, exe: &Path) -> io::Result<()> {
        let dir = directories::BaseDirs::new().ok_or(io::ErrorKind::NotFound)?.config_dir().join("autostart");
        let entry = dir.join("rdm.desktop");
        if !enabled {
            return match fs::remove_file(entry) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            };
        }
        fs::create_dir_all(&dir)?;
        fs::write(
            entry,
            format!(
                "[Desktop Entry]\nType=Application\nName=RDM\nExec=\"{}\" {}\nIcon=rdm\nX-GNOME-Autostart-enabled=true\n",
                super::desktop_quote(&exe.to_string_lossy()),
                super::MINIMIZED_FLAG
            ),
        )
    }
}

/// A LaunchAgent starting `exe` minimized at each login, in the user's graphical session.
#[cfg_attr(not(target_os = "macos"), allow(dead_code, reason = "macOS only, tested everywhere"))]
fn launch_agent(exe: &str) -> String {
    let exe = exe.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n<dict>\n\
         \t<key>Label</key>\n\t<string>org.rdm.rdm</string>\n\
         \t<key>ProgramArguments</key>\n\t<array>\n\t\t<string>{exe}</string>\n\t\t<string>{MINIMIZED_FLAG}</string>\n\t</array>\n\
         \t<key>RunAtLoad</key>\n\t<true/>\n\
         \t<key>LimitLoadToSessionType</key>\n\t<string>Aqua</string>\n\
         \t<key>ProcessType</key>\n\t<string>Interactive</string>\n\
         </dict>\n</plist>\n"
    )
}

/// Escapes a path for a quoted `Exec=` argument (Desktop Entry spec): `"` `` ` `` `$` `\` get a
/// backslash (itself doubled by the string-value escaping), and field codes' `%` is doubled.
#[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code))]
fn desktop_quote(path: &str) -> String {
    path.chars().fold(String::with_capacity(path.len()), |mut out, c| {
        match c {
            '"' | '`' | '$' | '\\' => {
                out.push_str("\\\\");
                out.push(c);
            }
            '%' => out.push_str("%%"),
            c => out.push(c),
        }
        out
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn desktop_exec_escaping() {
        assert_eq!(super::desktop_quote("/opt/r d m/rdm"), "/opt/r d m/rdm");
        assert_eq!(super::desktop_quote("/a\"$b%/rdm"), "/a\\\\\"\\\\$b%%/rdm");
    }

    #[test]
    fn a_launch_agent_starts_the_program_minimized_whatever_its_path_holds() {
        let agent = super::launch_agent("/Applications/R&D <m>.app/Contents/MacOS/rdm");

        assert!(agent.contains("<string>/Applications/R&amp;D &lt;m&gt;.app/Contents/MacOS/rdm</string>"), "{agent}");
        assert!(agent.contains("<string>--minimized</string>") && agent.contains("<key>RunAtLoad</key>"));
    }
}
