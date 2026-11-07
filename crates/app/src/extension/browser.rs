//! The browsers the extension runs in: known by name, listed by Windows, added by the user, or
//! reported by the extension itself; where each one is, and how to open it on a page.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

/// A browser the extension can run in: a well-known one, one Windows lists as installed, one the
/// user added, or one the extension reported from. Every browser belongs to one of two families
/// (`Flavour`): what works in Chrome works in any Chromium-based browser, what works in Firefox in
/// any of its derivatives, so none has to be listed here to be supported.
#[derive(Debug, Clone, Copy)]
pub struct Browser(&'static Info);

#[derive(Debug)]
struct Info {
    /// What the extension reports about itself (`x-rdm-browser`): see `key_of`.
    key: String,
    name: String,
    flavour: Flavour,
    /// Where it was found (Windows' list of browsers, or picked by the user).
    exe: Option<PathBuf>,
}

/// Chromium-based browsers share one build; Firefox and its derivatives have their own manifest;
/// Safari's extension comes inside RDM.app (see `packaging/safari`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Flavour {
    Chromium,
    Firefox,
    Safari,
}

/// Browsers known by name: the name shown and the family. Any other browser works the same,
/// named as Windows or the extension names it.
const KNOWN: &[(&str, &str, Flavour)] = &[
    ("firefox", "Firefox", Flavour::Firefox),
    ("waterfox", "Waterfox", Flavour::Firefox),
    ("librewolf", "LibreWolf", Flavour::Firefox),
    ("floorp", "Floorp", Flavour::Firefox),
    ("zen", "Zen Browser", Flavour::Firefox),
    ("mullvad", "Mullvad Browser", Flavour::Firefox),
    ("chrome", "Google Chrome", Flavour::Chromium),
    ("edge", "Microsoft Edge", Flavour::Chromium),
    ("brave", "Brave", Flavour::Chromium),
    ("opera", "Opera", Flavour::Chromium),
    ("vivaldi", "Vivaldi", Flavour::Chromium),
    ("chromium", "Chromium", Flavour::Chromium),
    ("safari", "Safari", Flavour::Safari),
];

/// Every browser met so far, one entry per key: `Browser` stays a cheap copy.
static SEEN: std::sync::Mutex<Vec<&'static Info>> = std::sync::Mutex::new(Vec::new());

/// Words of a browser's name that do not tell it apart: vendors, and generic words (`NOT_A_NAME`
/// in `extension/shared.js`).
const NOT_A_NAME: [&str; 7] = ["mozilla", "google", "microsoft", "browser", "stable", "web", "the"];

/// The key of a browser's name, as the extension computes it too (`browserKey` in
/// `extension/shared.js`, same tests): the first word that is not the vendor's or a generic one,
/// letters only, at most 16. "Google Chrome" is `chrome`, "Mullvad Browser" is `mullvad`.
fn key_of(name: &str) -> String {
    name.split(|c: char| !c.is_ascii_alphabetic())
        .map(str::to_ascii_lowercase)
        .find(|w| !w.is_empty() && !NOT_A_NAME.contains(&w.as_str()))
        .map(|w| w.chars().take(16).collect())
        .unwrap_or_default()
}

pub(crate) fn valid_key(key: &str) -> bool {
    (1..=16).contains(&key.len()) && key.bytes().all(|b| b.is_ascii_lowercase())
}

impl PartialEq for Browser {
    fn eq(&self, other: &Self) -> bool {
        self.0.key == other.0.key
    }
}

impl Eq for Browser {}

impl std::hash::Hash for Browser {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.key.hash(state);
    }
}

impl Browser {
    /// The browser of this key; learnt with `exe` (and the name Windows gives it) when found.
    fn with(key: &str, name: Option<&str>, flavour: Option<Flavour>, exe: Option<PathBuf>) -> Self {
        let mut seen = SEEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let old = seen.iter().position(|i| i.key == key);
        if let Some(i) = old
            && (exe.is_none() || seen[i].exe == exe)
        {
            return Self(seen[i]);
        }
        let old = old.map(|i| seen.remove(i));
        let known = KNOWN.iter().find(|(k, ..)| *k == key);
        // Leaked on purpose: one per browser (and per new executable), a handful in all.
        let info: &'static Info = Box::leak(Box::new(Info {
            key: key.to_owned(),
            // A well-known name first ("Google Chrome", not Windows' "Chrome").
            name: known
                .map(|k| k.1.to_owned())
                .or_else(|| name.map(str::to_owned))
                .or_else(|| old.map(|o| o.name.clone()))
                .unwrap_or_else(|| title(key)),
            flavour: known.map(|k| k.2).or(flavour).or(old.map(|o| o.flavour)).unwrap_or(Flavour::Chromium),
            exe: exe.or_else(|| old.and_then(|o| o.exe.clone())),
        }));
        seen.push(info);
        Self(info)
    }

    /// The browser the extension says it runs in (`x-rdm-browser`); `None` for a malformed key.
    /// One RDM does not know otherwise is taken as Chromium-based until found on this computer.
    pub fn from_key(key: &str) -> Option<Self> {
        valid_key(key).then(|| Self::with(key, None, None, None))
    }

    pub fn name(self) -> &'static str {
        &self.0.name
    }

    pub fn key(self) -> &'static str {
        &self.0.key
    }

    pub fn flavour(self) -> Flavour {
        self.0.flavour
    }

    /// Where extensions are managed (opened for the user, who confirms the installation there).
    /// Chromium-based browsers without a page of their own accept `chrome://extensions`.
    pub fn extensions_page(self) -> &'static str {
        match (self.flavour(), self.key()) {
            (Flavour::Firefox, _) => "about:debugging#/runtime/this-firefox",
            (_, "brave") => "brave://extensions/",
            (_, "opera") => "opera://extensions/",
            (_, "edge") => "edge://extensions/",
            (_, "vivaldi") => "vivaldi://extensions/",
            _ => "chrome://extensions/",
        }
    }

    /// The browser's executable (its application bundle on macOS), if it is installed.
    pub fn find(self) -> Option<PathBuf> {
        self.0.exe.clone().filter(|p| p.exists()).or_else(|| imp::find(self.key()))
    }

    /// The browser at `exe` (added by the user): its family from its files, named after the file.
    /// `None`: not a browser the extension can run in.
    pub fn at(exe: &Path) -> Option<Self> {
        let flavour = flavour_of(exe)?;
        let stem = exe.file_stem()?.to_string_lossy().into_owned();
        let key = key_of(&stem);
        valid_key(&key).then(|| Self::with(&key, Some(&title(&stem)), Some(flavour), Some(exe.to_path_buf())))
    }
}

/// "waterfox" gives "Waterfox".
fn title(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// A browser's family, from the files beside its executable: Firefox and every derivative ship
/// `xul.dll` (`libxul.so`); Chromium-based browsers `.pak` resources (in a version sub-folder on
/// Windows). `None`: not a browser the extension can run in.
#[cfg(not(target_os = "macos"))]
fn flavour_of(exe: &Path) -> Option<Flavour> {
    let dir = exe.parent()?;
    let names = |dir: &Path| -> Vec<String> {
        fs::read_dir(dir).map(|e| e.flatten().map(|e| e.file_name().to_string_lossy().to_ascii_lowercase()).collect()).unwrap_or_default()
    };
    let here = names(dir);
    if here.iter().any(|n| n == "xul.dll" || n == "libxul.so") {
        return Some(Flavour::Firefox);
    }
    let chromium = |list: &[String]| list.iter().any(|n| n.ends_with(".pak") || n == "chrome.dll" || n == "msedge.dll");
    let versions = fs::read_dir(dir).map(|e| e.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect::<Vec<_>>()).unwrap_or_default();
    (chromium(&here) || versions.iter().any(|v| chromium(&names(v)))).then_some(Flavour::Chromium)
}

/// macOS: from the application bundle: Firefox and every derivative hold `XUL`, Chromium-based
/// browsers a `<name> Framework.framework` (Electron applications too, under that name).
#[cfg(target_os = "macos")]
fn flavour_of(app: &Path) -> Option<Flavour> {
    if app.file_stem().is_some_and(|name| name == "Safari") {
        return Some(Flavour::Safari);
    }
    if app.join("Contents/MacOS/XUL").is_file() {
        return Some(Flavour::Firefox);
    }
    let frameworks = fs::read_dir(app.join("Contents/Frameworks")).ok()?;
    frameworks
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .any(|name| name.ends_with(" Framework.framework") && name != "Electron Framework.framework")
        .then_some(Flavour::Chromium)
}

/// The browsers of this computer: those Windows lists as installed (any browser, known here or
/// not), the well-known ones found where they usually are, and those the user added (`custom`).
/// One browser per key, the first found: two of the same key (Firefox and Firefox Developer
/// Edition are both `firefox`) would otherwise take each other's place at every call, each time a
/// new (leaked) `Info`.
pub fn installed(custom: &[PathBuf]) -> Vec<Browser> {
    let mut list: Vec<Browser> = Vec::new();
    let listed = |list: &[Browser], key: &str| list.iter().any(|b| b.key() == key);
    for (name, exe) in imp::registered() {
        let key = key_of(&name);
        if valid_key(&key)
            && !listed(&list, &key)
            && let Some(flavour) = flavour_of(&exe)
        {
            list.push(Browser::with(&key, Some(&name), Some(flavour), Some(exe)));
        }
    }
    // macOS: the list above holds them all, and each look would read the Applications folder again.
    for (key, ..) in KNOWN.iter().filter(|_| !cfg!(target_os = "macos")) {
        if !listed(&list, key) {
            let b = Browser::with(key, None, None, None);
            if b.find().is_some() {
                list.push(b);
            }
        }
    }
    for exe in custom {
        let key = exe.file_stem().map(|s| key_of(&s.to_string_lossy())).unwrap_or_default();
        if !listed(&list, &key)
            && let Some(b) = Browser::at(exe)
        {
            list.push(b);
        }
    }
    list
}

/// The system's default browser, if the extension can run in it.
pub fn default_browser() -> Option<Browser> {
    imp::default_browser()
}

/// The browser at `exe`: a well-known one by its name, wherever it is; any other by its files.
#[cfg_attr(all(unix, not(target_os = "macos")), allow(dead_code, reason = "Linux names browsers by their desktop entry"))]
fn browser_of_exe(exe: PathBuf) -> Option<Browser> {
    match super::running::key_of_exe(&exe.to_string_lossy()) {
        Some(key) => Some(Browser::with(key, None, None, Some(exe))),
        None => Browser::at(&exe),
    }
}

/// The program of a command as Windows' registry writes it: `"C:\…\browser.exe" --args "%1"`, or
/// unquoted, `C:\…\browser.exe -osint %1`.
#[cfg_attr(not(windows), allow(dead_code, reason = "Windows only, tested everywhere"))]
fn exe_of_command(command: &str) -> PathBuf {
    let command = command.trim();
    let exe = command.strip_prefix('"').and_then(|c| c.split('"').next()).unwrap_or_else(|| command.split(" -").next().and_then(|c| c.split(" %").next()).unwrap_or(command));
    PathBuf::from(exe.trim())
}

/// Where a well-known browser may be on Linux: its command, and its Flatpak's name.
#[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code, reason = "Linux only, tested everywhere"))]
const LINUX_NAMES: [(&str, &[&str]); 12] = [
    ("firefox", &["firefox", "firefox-esr", "org.mozilla.firefox"]),
    ("waterfox", &["waterfox", "waterfox-g", "waterfox-current", "net.waterfox.waterfox"]),
    ("librewolf", &["librewolf", "io.gitlab.librewolf-community"]),
    ("floorp", &["floorp", "one.ablaze.floorp"]),
    ("zen", &["zen", "zen-browser", "app.zen_browser.zen"]),
    ("mullvad", &["mullvad-browser", "net.mullvad.MullvadBrowser"]),
    ("chrome", &["google-chrome", "google-chrome-stable", "com.google.Chrome"]),
    ("brave", &["brave-browser", "brave", "com.brave.Browser"]),
    ("opera", &["opera", "com.opera.Opera"]),
    ("edge", &["microsoft-edge", "microsoft-edge-stable", "com.microsoft.Edge"]),
    ("vivaldi", &["vivaldi", "vivaldi-stable", "com.vivaldi.Vivaldi"]),
    ("chromium", &["chromium", "chromium-browser", "org.chromium.Chromium"]),
];

/// The browser of a desktop entry, as `xdg-settings` names the default one: `firefox.desktop`,
/// `firefox_firefox.desktop` (Snap), `google-chrome-beta.desktop`, `com.brave.Browser.desktop`.
#[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code, reason = "Linux only, tested everywhere"))]
fn key_of_desktop_entry(entry: &str) -> Option<&'static str> {
    let entry = entry.trim().trim_end_matches(".desktop").to_ascii_lowercase();
    let names = |name: &&str| entry.strip_prefix(name.to_ascii_lowercase().as_str()).is_some_and(|rest| rest.is_empty() || rest.starts_with(['-', '_']));
    LINUX_NAMES.iter().find(|(_, list)| list.iter().any(names)).map(|(key, _)| *key)
}

/// Opens Safari's settings on RDM's extension, where the user turns it on. `false` when they could
/// not be asked for (not macOS).
pub fn open_safari_settings() -> bool {
    imp::open_safari_settings()
}

/// Opens `target` (a page or a package) in the browser whose executable is `exe`.
pub fn launch(exe: &Path, target: &str) -> io::Result<()> {
    use std::process::{Command, Stdio};
    // macOS: a second copy of a browser already running would refuse its profile: the system hands
    // the page to the running one.
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut open = Command::new("/usr/bin/open");
        open.arg("-a").arg(exe);
        open
    };
    #[cfg(not(target_os = "macos"))]
    let mut command = Command::new(exe);
    let mut child = command.arg(target).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
    // Reaped off the UI thread: the browser may run for hours (Linux would keep a zombie otherwise).
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(windows)]
mod imp {
    use std::path::PathBuf;

    use winreg::{
        RegKey,
        enums::{HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE},
    };

    use super::Browser;

    /// Who opens https links for this user, then that program's command. `UserChoiceLatest`: where
    /// recent Windows 11 keep the choice, next to the older key.
    pub fn default_browser() -> Option<Browser> {
        let choice: String = [r"UserChoiceLatest\ProgId", "UserChoice"].into_iter().find_map(|key| {
            let path = format!(r"Software\Microsoft\Windows\Shell\Associations\UrlAssociations\https\{key}");
            RegKey::predef(HKEY_CURRENT_USER).open_subkey(path).ok()?.get_value("ProgId").ok()
        })?;
        let command: String = RegKey::predef(HKEY_CLASSES_ROOT).open_subkey(format!(r"{choice}\shell\open\command")).ok()?.get_value("").ok()?;
        super::browser_of_exe(super::exe_of_command(&command))
    }

    pub fn open_safari_settings() -> bool {
        false
    }

    /// Browsers Windows lists as installed (Default apps): name and executable. Every browser
    /// installer registers there, known to RDM or not.
    pub fn registered() -> Vec<(String, PathBuf)> {
        let mut list = Vec::new();
        for (hive, path) in [
            (HKEY_CURRENT_USER, r"SOFTWARE\Clients\StartMenuInternet"),
            (HKEY_LOCAL_MACHINE, r"SOFTWARE\Clients\StartMenuInternet"),
            (HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\Clients\StartMenuInternet"),
        ] {
            let Ok(clients) = RegKey::predef(hive).open_subkey(path) else { continue };
            for id in clients.enum_keys().flatten() {
                let Ok(client) = clients.open_subkey(&id) else { continue };
                let name: String = client.get_value("").unwrap_or_else(|_| id.clone());
                let Some(command) = client.open_subkey(r"shell\open\command").ok().and_then(|c| c.get_value::<String, _>("").ok()) else { continue };
                let exe = super::exe_of_command(&command);
                if exe.is_file() && !list.iter().any(|(_, e)| e == &exe) {
                    list.push((name, exe));
                }
            }
        }
        list
    }

    /// A well-known browser's executable, where its installer usually puts it.
    pub fn find(key: &str) -> Option<PathBuf> {
        let (exe, known): (&str, &[&str]) = match key {
            "firefox" => ("firefox.exe", &[r"Mozilla Firefox\firefox.exe"]),
            "waterfox" => (
                "waterfox.exe",
                &[r"Waterfox\waterfox.exe", r"Waterfox Current\waterfox.exe", r"Waterfox Classic\waterfox.exe", r"Programs\Waterfox\waterfox.exe"],
            ),
            "chrome" => ("chrome.exe", &[r"Google\Chrome\Application\chrome.exe"]),
            "brave" => ("brave.exe", &[r"BraveSoftware\Brave-Browser\Application\brave.exe"]),
            "opera" => ("opera.exe", &[r"Programs\Opera\opera.exe", r"Programs\Opera\launcher.exe", r"Opera\launcher.exe"]),
            "edge" => ("msedge.exe", &[r"Microsoft\Edge\Application\msedge.exe"]),
            "chromium" => ("chromium.exe", &[r"Chromium\Application\chrome.exe"]),
            "vivaldi" => ("vivaldi.exe", &[r"Vivaldi\Application\vivaldi.exe"]),
            "librewolf" => ("librewolf.exe", &[r"LibreWolf\librewolf.exe"]),
            _ => return None,
        };
        let registered = [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE].into_iter().find_map(|hive| {
            let key = RegKey::predef(hive).open_subkey(format!(r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\{exe}")).ok()?;
            let path: String = key.get_value("").ok()?;
            Some(PathBuf::from(path.trim_matches('"')))
        });
        registered.filter(|p| p.is_file()).or_else(|| {
            ["LOCALAPPDATA", "ProgramFiles", "ProgramFiles(x86)"]
                .into_iter()
                .filter_map(std::env::var_os)
                .flat_map(|root| known.iter().map(move |k| PathBuf::from(&root).join(k)))
                .find(|p| p.is_file())
        })
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::{ffi::CStr, path::PathBuf};

    use block2::Block;
    use objc2::{
        class, msg_send,
        rc::autoreleasepool,
        runtime::{AnyClass, AnyObject},
    };

    use super::Browser;

    /// RDM's extension, as Xcode names it (checked by `packaging/safari/build.sh`).
    const SAFARI_EXTENSION: &CStr = c"org.rdm.rdm.Extension";

    /// The application that opens https links.
    pub fn default_browser() -> Option<Browser> {
        // SAFETY: Foundation's and AppKit's documented calls, usable from any thread, on objects the
        // pool keeps alive; each result is checked before it is used.
        let app = autoreleasepool(|_| unsafe {
            let text: *mut AnyObject = msg_send![class!(NSString), stringWithUTF8String: c"https://example.com/".as_ptr()];
            let url: *mut AnyObject = msg_send![class!(NSURL), URLWithString: text];
            let workspace: *mut AnyObject = msg_send![class!(NSWorkspace), sharedWorkspace];
            let app: *mut AnyObject = msg_send![workspace, URLForApplicationToOpenURL: url];
            if app.is_null() {
                return None;
            }
            let path: *mut AnyObject = msg_send![app, path];
            if path.is_null() {
                return None;
            }
            let utf8: *const std::ffi::c_char = msg_send![path, UTF8String];
            (!utf8.is_null()).then(|| PathBuf::from(CStr::from_ptr(utf8).to_string_lossy().into_owned()))
        })?;
        super::browser_of_exe(app)
    }

    /// `SFSafariApplication` is loaded here, when asked for: most computers' RDM never needs it, and a
    /// program linked to the framework would fail to start without it.
    pub fn open_safari_settings() -> bool {
        const FRAMEWORK: &CStr = c"/System/Library/Frameworks/SafariServices.framework/SafariServices";
        // SAFETY: a system library path, NUL-terminated; loading it only registers its classes.
        unsafe { libc::dlopen(FRAMEWORK.as_ptr(), libc::RTLD_LAZY) };
        let Some(safari) = AnyClass::get(c"SFSafariApplication") else { return false };
        // SAFETY: the class's documented method, usable from any thread; a null completion handler
        // is allowed, and the identifier lives in the pool for the call.
        autoreleasepool(|_| unsafe {
            let id: *mut AnyObject = msg_send![class!(NSString), stringWithUTF8String: SAFARI_EXTENSION.as_ptr()];
            let no_handler: Option<&Block<dyn Fn(*mut AnyObject)>> = None;
            let _: () = msg_send![safari, showPreferencesForExtensionWithIdentifier: id, completionHandler: no_handler];
        });
        true
    }

    /// The applications of this computer that are browsers (whatever their name), with their bundle.
    pub fn registered() -> Vec<(String, PathBuf)> {
        let home = directories::BaseDirs::new().map(|d| d.home_dir().join("Applications"));
        std::iter::once(PathBuf::from("/Applications"))
            .chain(home)
            .filter_map(|dir| std::fs::read_dir(dir).ok())
            .flat_map(|apps| apps.flatten().map(|app| app.path()))
            .filter(|app| app.extension().is_some_and(|e| e == "app") && super::flavour_of(app).is_some())
            .filter_map(|app| Some((app.file_stem()?.to_string_lossy().into_owned(), app)))
            .collect()
    }

    pub fn find(key: &str) -> Option<PathBuf> {
        registered().into_iter().find(|(name, _)| super::key_of(name) == key).map(|(_, app)| app)
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use std::path::PathBuf;

    use super::Browser;

    /// Linux has no list of installed browsers: the well-known ones are looked for by name.
    pub fn registered() -> Vec<(String, PathBuf)> {
        Vec::new()
    }

    /// What the desktop's settings name, when `xdg-utils` is there.
    pub fn default_browser() -> Option<Browser> {
        let out = std::process::Command::new("xdg-settings")
            .args(["get", "default-web-browser"])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        let entry = String::from_utf8_lossy(&out.stdout);
        Browser::from_key(super::key_of_desktop_entry(&entry)?)
    }

    pub fn open_safari_settings() -> bool {
        false
    }

    pub fn find(key: &str) -> Option<PathBuf> {
        let names = super::LINUX_NAMES.iter().find(|(k, _)| *k == key)?.1;
        let home = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf());
        let dirs: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
            .unwrap_or_default()
            .into_iter()
            // Waterfox's own tarball unpacks to /opt/waterfox.
            .chain(["/snap/bin", "/var/lib/flatpak/exports/bin", "/opt/waterfox"].map(PathBuf::from))
            .chain(home.map(|h| h.join(".local/share/flatpak/exports/bin")))
            .collect();
        names.iter().flat_map(|n| dirs.iter().map(move |d| d.join(n))).find(|p| p.is_file())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Same cases as `browserKey` in `extension/test/shared.test.mjs`.
    #[test]
    fn a_browsers_key_is_the_first_telling_word_of_its_name() {
        for (name, key) in [
            ("Google Chrome", "chrome"),
            ("Microsoft Edge", "edge"),
            ("Mozilla Firefox", "firefox"),
            ("Firefox", "firefox"),
            ("Waterfox", "waterfox"),
            ("Mullvad Browser", "mullvad"),
            ("Zen Browser", "zen"),
            ("Opera Stable", "opera"),
            ("Brave", "brave"),
            ("Thorium", "thorium"),
            ("Supercalifragilisticexpialidocious", "supercalifragili"),
            ("", ""),
        ] {
            assert_eq!(key_of(name), key, "{name}");
        }
    }

    #[test]
    fn the_program_of_a_command_is_read_quoted_or_not() {
        for (command, exe) in [
            (r#""C:\Program Files\Google\Chrome\Application\chrome.exe" --single-argument %1"#, r"C:\Program Files\Google\Chrome\Application\chrome.exe"),
            (r#""C:\Program Files\Mozilla Firefox\firefox.exe" -osint -url "%1""#, r"C:\Program Files\Mozilla Firefox\firefox.exe"),
            (r"C:\Browsers\thorium.exe -url %1", r"C:\Browsers\thorium.exe"),
            (r"C:\Browsers\thorium.exe %1", r"C:\Browsers\thorium.exe"),
            (r"C:\Browsers\thorium.exe", r"C:\Browsers\thorium.exe"),
        ] {
            assert_eq!(exe_of_command(command), PathBuf::from(exe), "{command}");
        }
    }

    #[test]
    fn a_desktop_entry_names_its_browser() {
        for (entry, key) in [
            ("firefox.desktop\n", Some("firefox")),
            ("firefox_firefox.desktop", Some("firefox")),
            ("firefox-esr.desktop", Some("firefox")),
            ("org.mozilla.firefox.desktop", Some("firefox")),
            ("google-chrome.desktop", Some("chrome")),
            ("google-chrome-beta.desktop", Some("chrome")),
            ("com.brave.Browser.desktop", Some("brave")),
            ("brave-browser.desktop", Some("brave")),
            ("microsoft-edge.desktop", Some("edge")),
            ("vivaldi-stable.desktop", Some("vivaldi")),
            ("chromium-browser.desktop", Some("chromium")),
            ("zenity.desktop", None),
            ("epiphany.desktop", None),
            ("", None),
        ] {
            assert_eq!(key_of_desktop_entry(entry), key, "{entry}");
        }
    }

    #[test]
    fn safari_is_a_family_of_its_own() {
        assert_eq!(Browser::from_key("safari").unwrap().flavour(), Flavour::Safari);
    }

    #[test]
    fn the_extension_skips_the_same_generic_words() {
        let shared_js = include_str!("../../../../extension/shared.js");
        let start = shared_js.find("const NOT_A_NAME").unwrap();
        let list = &shared_js[start..start + shared_js[start..].find("]);").unwrap()];
        let mut words: Vec<&str> = list.split('"').skip(1).step_by(2).collect();
        let mut ours = NOT_A_NAME.to_vec();

        words.sort_unstable();
        ours.sort_unstable();

        assert_eq!(words, ours);
    }

    #[test]
    fn a_browser_is_known_by_its_key_whatever_it_is() {
        let firefox = Browser::from_key("firefox").unwrap();
        assert_eq!((firefox.name(), firefox.flavour()), ("Firefox", Flavour::Firefox));
        let thorium = Browser::from_key("thorium").unwrap();
        assert_eq!((thorium.name(), thorium.flavour(), thorium.extensions_page()), ("Thorium", Flavour::Chromium, "chrome://extensions/"));
        assert_eq!(Browser::from_key("thorium"), Some(thorium), "the same browser again");
        assert!(Browser::from_key("Net Scape").is_none() && Browser::from_key("").is_none());
    }
}
