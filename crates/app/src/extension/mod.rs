//! The browser extension, bundled in the executable. RDM writes it to a stable folder the browsers
//! load it from — Chrome, Brave, Opera, Edge, Chromium: "load unpacked"; Firefox: the signed
//! package of the GitHub release when there is one, a temporary add-on otherwise; Waterfox (a
//! Firefox derivative that can accept unsigned packages): the package itself — rewrites that copy
//! when a newer RDM brings a newer extension, and opens each browser where the user confirms.

mod browser;
mod running;

use std::{borrow::Cow, fs, io, path::PathBuf};

pub use browser::{Browser, Flavour, installed, launch};
pub(crate) use browser::valid_key;
pub use running::running;

include!(concat!(env!("OUT_DIR"), "/extension_files.rs"));

impl Flavour {
    const fn dir_name(self) -> &'static str {
        match self {
            Self::Chromium => "chromium",
            Self::Firefox => "firefox",
        }
    }
}

/// Parent of the unpacked copies. Visible on Linux: browsers packaged as Snap or Flatpak can only
/// read non-hidden folders of the home directory.
pub fn base() -> PathBuf {
    #[cfg(windows)]
    {
        std::env::var_os("LOCALAPPDATA").map_or_else(std::env::temp_dir, PathBuf::from).join("RDM").join("Extension")
    }
    #[cfg(not(windows))]
    {
        directories::BaseDirs::new()
            .map_or_else(std::env::temp_dir, |d| d.home_dir().to_path_buf())
            .join("RDM")
            .join("Extension")
    }
}

/// The folder a browser loads the unpacked extension from.
pub fn folder(flavour: Flavour) -> PathBuf {
    base().join(flavour.dir_name())
}

/// The extension's files for `flavour`: identical except the Firefox manifest.
pub fn files(flavour: Flavour) -> Vec<(&'static str, Cow<'static, [u8]>)> {
    FILES
        .iter()
        .map(|&(name, bytes)| match (name, flavour) {
            ("manifest.json", Flavour::Firefox) => (name, Cow::Owned(firefox_manifest(bytes))),
            _ => (name, Cow::Borrowed(bytes)),
        })
        .collect()
}

/// Same derivation as `packaging/firefox/build.sh`: Chrome wants an MV3 background service worker
/// (and flags `background.scripts`), Firefox only knows `background.scripts`; Chrome-only keys go;
/// `webRequestBlocking` (refused by Chrome MV3) lets Firefox take a download over before it starts.
fn firefox_manifest(chrome: &[u8]) -> Vec<u8> {
    let mut manifest: serde_json::Value = serde_json::from_slice(chrome).expect("extension/manifest.json is valid JSON");
    if let Some(m) = manifest.as_object_mut() {
        m.remove("key");
        m.remove("minimum_chrome_version");
        if let Some(bg) = m.get_mut("background").and_then(serde_json::Value::as_object_mut)
            && let Some(worker) = bg.remove("service_worker")
        {
            bg.insert("scripts".into(), serde_json::Value::Array(vec![worker]));
        }
        if let Some(permissions) = m.get_mut("permissions").and_then(serde_json::Value::as_array_mut) {
            permissions.push("webRequestBlocking".into());
        }
    }
    serde_json::to_vec_pretty(&manifest).expect("serializable")
}

/// The version in the bundled manifest (read once: the extension window shows it at every frame).
pub fn version() -> &'static str {
    static VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    VERSION.get_or_init(|| {
        FILES
            .iter()
            .find(|(name, _)| *name == "manifest.json")
            .and_then(|(_, bytes)| serde_json::from_slice::<serde_json::Value>(bytes).ok())
            .and_then(|m| m.get("version")?.as_str().map(str::to_owned))
            .unwrap_or_default()
    })
}

/// Writes (or brings up to date) the unpacked extension; returns its folder. Files already
/// identical are left alone.
pub fn write(flavour: Flavour) -> io::Result<PathBuf> {
    let dir = folder(flavour);
    for (name, bytes) in files(flavour) {
        let path = dir.join(name);
        if fs::read(&path).is_ok_and(|old| old == *bytes) {
            continue;
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = crate::settings::with_suffix(&path, ".tmp");
        fs::write(&tmp, &bytes)?;
        fs::rename(tmp, path)?;
    }
    Ok(dir)
}

/// At start: copies installed from an older RDM get this version's files (browsers load them
/// at their next start). Folders never installed are not created.
pub fn refresh_installed() {
    for flavour in [Flavour::Chromium, Flavour::Firefox] {
        if folder(flavour).join("manifest.json").exists() {
            let _ = write(flavour);
        }
    }
    if base().join(XPI).exists() {
        let _ = write_xpi();
    }
}

const XPI: &str = "rdm-firefox.xpi";

/// The Firefox package as one `.xpi` file (unsigned): permanent installation in Firefox Developer
/// Edition, Nightly, ESR (with `xpinstall.signatures.required` off) and derivatives such as
/// LibreWolf; release Firefox only installs signed packages.
pub fn write_xpi() -> io::Result<PathBuf> {
    let path = base().join(XPI);
    fs::create_dir_all(base())?;
    let files = files(Flavour::Firefox);
    let bytes = zip(files.iter().map(|(n, b)| (*n, b.as_ref())));
    if fs::read(&path).is_ok_and(|old| old == bytes) {
        return Ok(path);
    }
    let tmp = crate::settings::with_suffix(&path, ".tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(tmp, &path)?;
    Ok(path)
}

/// A minimal ZIP archive (stored, no compression): enough for an `.xpi`, no extra dependency.
pub(crate) fn zip<'a>(files: impl Iterator<Item = (&'a str, &'a [u8])>) -> Vec<u8> {
    let (mut out, mut central, mut count) = (Vec::new(), Vec::new(), 0u16);
    for (name, data) in files {
        let (crc, size, offset) = (crc32(data), data.len() as u32, out.len() as u32);
        let header = |sig: u32, central: bool| {
            let mut h = Vec::new();
            h.extend_from_slice(&sig.to_le_bytes());
            if central {
                h.extend_from_slice(&20u16.to_le_bytes()); // made by
            }
            // version needed, flags (UTF-8 names), method (stored), time, date (1980-01-01)
            for v in [20u16, 0x0800, 0, 0, 0x21] {
                h.extend_from_slice(&v.to_le_bytes());
            }
            for v in [crc, size, size] {
                h.extend_from_slice(&v.to_le_bytes());
            }
            h.extend_from_slice(&(name.len() as u16).to_le_bytes());
            h.extend_from_slice(&0u16.to_le_bytes()); // extra
            if central {
                for v in [0u16, 0, 0] {
                    h.extend_from_slice(&v.to_le_bytes()); // comment, disk, internal attributes
                }
                h.extend_from_slice(&0u32.to_le_bytes()); // external attributes
                h.extend_from_slice(&offset.to_le_bytes());
            }
            h.extend_from_slice(name.as_bytes());
            h
        };
        out.extend(header(0x0403_4b50, false));
        out.extend_from_slice(data);
        central.extend(header(0x0201_4b50, true));
        count += 1;
    }
    let (dir_offset, dir_size) = (out.len() as u32, central.len() as u32);
    out.extend(central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    for v in [0u16, 0, count, count] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&dir_size.to_le_bytes());
    out.extend_from_slice(&dir_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

fn crc32(data: &[u8]) -> u32 {
    !data.iter().fold(!0u32, |crc, &b| {
        (0..8).fold(crc ^ u32::from(b), |c, _| if c & 1 == 1 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundles_the_whole_extension() {
        for name in ["manifest.json", "background.js", "content.js", "shared.js", "relay.html", "icons/icon128.png"] {
            assert!(FILES.iter().any(|(n, _)| *n == name), "{name}");
        }
        assert!(!FILES.iter().any(|(n, _)| n.starts_with("test/")), "tests stay out");
        assert!(!version().is_empty());
    }

    #[test]
    fn firefox_manifest_is_derived_like_the_build_script() {
        let firefox = files(Flavour::Firefox);
        let manifest = &firefox.iter().find(|(n, _)| *n == "manifest.json").unwrap().1;
        let m: serde_json::Value = serde_json::from_slice(manifest).unwrap();
        assert_eq!(m["background"]["scripts"][0], "background.js");
        assert!(m["background"].get("service_worker").is_none());
        assert!(m.get("key").is_none() && m.get("minimum_chrome_version").is_none());
        assert!(m["browser_specific_settings"]["gecko"]["id"].is_string());
        assert!(m["permissions"].as_array().unwrap().iter().any(|p| p == "webRequestBlocking"));
        let chrome: serde_json::Value =
            serde_json::from_slice(&files(Flavour::Chromium).iter().find(|(n, _)| *n == "manifest.json").unwrap().1).unwrap();
        assert!(chrome["background"]["service_worker"].is_string() && chrome.get("key").is_some());
    }

    #[test]
    fn packages_are_zip_archives_with_their_checksums() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        let z = zip([("a.txt", b"hi".as_slice())].into_iter());
        assert_eq!(&z[..4], b"PK\x03\x04");
        assert_eq!(&z[z.len() - 22..z.len() - 18], b"PK\x05\x06");
        assert_eq!(u16::from_le_bytes([z[z.len() - 12], z[z.len() - 11]]), 1, "one entry");
    }

    fn bundled_manifest() -> serde_json::Value {
        let bytes = FILES.iter().find(|(name, _)| *name == "manifest.json").map(|(_, bytes)| *bytes).unwrap();
        serde_json::from_slice(bytes).unwrap()
    }

    fn base64_decode(text: &str) -> Vec<u8> {
        const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let (mut bits, mut pending, mut out) = (0u32, 0u32, Vec::new());
        for c in text.bytes().filter(|&c| c != b'=') {
            let value = u32::try_from(TABLE.iter().position(|&t| t == c).unwrap()).unwrap();
            bits = (bits << 6 | value) & 0xFFFF;
            pending += 6;
            if pending >= 8 {
                pending -= 8;
                out.push(u8::try_from(bits >> pending & 0xFF).unwrap());
            }
        }
        out
    }

    /// The ID RDM trusts (the bridge, the native connector) is the one Chrome derives from the
    /// manifest's `key`: the first 32 hexadecimal digits of its SHA-256, written with `a` to `p`.
    #[test]
    fn the_trusted_extension_id_is_the_manifests_key() {
        use sha2::{Digest, Sha256};
        let key = base64_decode(bundled_manifest()["key"].as_str().unwrap());

        let digest = crate::hex::encode(&Sha256::digest(&key));
        let id: String = digest.chars().take(32).map(|c| char::from(b'a' + u8::try_from(c.to_digit(16).unwrap()).unwrap())).collect();

        assert_eq!(format!("chrome-extension://{id}"), crate::bridge::EXTENSION_ORIGIN);
    }

    #[test]
    fn the_trusted_firefox_id_is_the_manifests() {
        assert_eq!(bundled_manifest()["browser_specific_settings"]["gecko"]["id"], crate::native::FIREFOX_ID);
    }
}
