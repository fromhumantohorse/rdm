use serde::{Deserialize, Serialize};

/// IDM-style categories: each gets its own folder and sidebar filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Category {
    Video,
    Music,
    Archive,
    Program,
    Document,
    Model,
    Other,
}

/// Revision of the default capture list, raised by each update that adds extensions to it.
pub const CAPTURED_REVISION: u8 = 1;

/// Single source of truth: extension → category, with the revision of the defaults that added it.
/// Every listed extension is captured by default.
const KNOWN: &[(Category, u8, &[&str])] = &[
    (
        Category::Video,
        0,
        &[
            "3g2", "3gp", "asf", "avi", "divx", "f4v", "flv", "m2ts", "m4v", "mkv", "mov", "mp4", "mpe", "mpeg",
            "mpg", "mts", "mxf", "ogm", "ogv", "qt", "rm", "rmvb", "ts", "vob", "webm", "wmv", "wtv",
        ],
    ),
    (
        Category::Video,
        1,
        &[
            "amv", "dv", "evo", "f4p", "h264", "h265", "hevc", "ivf", "m2t", "m2v", "mjpeg", "mk3d", "mod", "mp4v",
            "nsv", "tod", "trp",
        ],
    ),
    (
        Category::Music,
        0,
        &[
            "aac", "ac3", "aif", "aiff", "alac", "amr", "ape", "dts", "flac", "m4a", "m4b", "mid", "midi", "mka",
            "mp3", "mpa", "oga", "ogg", "opus", "ra", "wav", "weba", "wma", "wv",
        ],
    ),
    (
        Category::Music,
        1,
        &[
            "aa", "aax", "aifc", "au", "caf", "dff", "dsf", "f4a", "f4b", "kar", "m4r", "mogg", "mp2", "mpc", "rmi",
            "spx", "tak", "tta", "w64",
        ],
    ),
    (
        Category::Archive,
        0,
        &[
            "7z", "ace", "arj", "bz2", "cab", "cpio", "gz", "gzip", "lha", "lz", "lzh", "lzma", "rar", "sit",
            "sitx", "tar", "tbz", "tbz2", "tgz", "txz", "tzst", "xz", "z", "zip", "zipx", "zst",
        ],
    ),
    (
        Category::Archive,
        1,
        &[
            "alz", "arc", "br", "bz", "egg", "lz4", "lzo", "par2", "pea", "rz", "s7z", "shar", "sz", "taz", "tlz",
            "tz", "xar", "zoo", "zpaq", "zstd",
        ],
    ),
    (
        Category::Program,
        0,
        &[
            "aab", "apk", "apks", "appimage", "appx", "appxbundle", "bin", "deb", "dmg", "exe", "img", "iso",
            "jar", "msi", "msix", "msixbundle", "msu", "ova", "pkg", "plj", "qcow2", "rpm", "sea", "vhd", "vhdx",
            "vmdk", "xapk",
        ],
    ),
    (
        Category::Program,
        1,
        &[
            "apkm", "esd", "flatpak", "ipa", "ipsw", "isz", "mdf", "mds", "msp", "nrg", "qcow", "run", "snap", "swm",
            "vdi", "wim", "xip",
        ],
    ),
    (
        Category::Document,
        0,
        &[
            "azw", "azw3", "cbr", "cbz", "djvu", "doc", "docx", "epub", "mobi", "odp", "ods", "odt", "pdf", "pps",
            "ppsx", "ppt", "pptx", "rtf", "tif", "tiff", "xls", "xlsx",
        ],
    ),
    (
        Category::Document,
        1,
        &[
            "cb7", "cbt", "chm", "djv", "docm", "dot", "dotm", "dotx", "fb2", "ibooks", "kfx", "lit", "numbers",
            "odg", "ott", "oxps", "pages", "pot", "potx", "ppsm", "pptm", "prc", "pub", "vsd", "vsdx", "wpd", "wps",
            "xlsb", "xlsm", "xlt", "xltx", "xps",
        ],
    ),
    (
        Category::Model,
        1,
        &[
            "caffemodel", "ckpt", "ggml", "gguf", "h5", "keras", "llamafile", "mlmodel", "nemo", "onnx", "ort", "pt",
            "pth", "safetensors", "tflite",
        ],
    ),
];

impl Category {
    pub const ALL: [Self; 7] = [Self::Video, Self::Music, Self::Archive, Self::Program, Self::Document, Self::Model, Self::Other];

    pub fn of(name: &str) -> Self {
        let ext = extension(name);
        if is_split_archive(&ext) {
            return Self::Archive;
        }
        KNOWN
            .iter()
            .find(|(_, _, exts)| exts.contains(&ext.as_str()))
            .map_or(Self::Other, |(c, _, _)| *c)
    }

    /// Display (and default folder) name, in English or French.
    pub const fn label(self, english: bool) -> &'static str {
        match (self, english) {
            (Self::Video, false) => "Vidéos",
            (Self::Video, true) => "Videos",
            (Self::Music, false) => "Musique",
            (Self::Music, true) => "Music",
            (Self::Archive, false) => "Compressés",
            (Self::Archive, true) => "Archives",
            (Self::Program, false) => "Programmes",
            (Self::Program, true) => "Programs",
            (Self::Document, _) => "Documents",
            (Self::Model, false) => "Modèles IA",
            (Self::Model, true) => "AI models",
            (Self::Other, false) => "Autres",
            (Self::Other, true) => "Other",
        }
    }
}

/// Default capture list (space-separated, user-editable in the settings).
pub fn default_captured() -> String {
    let mut all: Vec<&str> = KNOWN.iter().flat_map(|(_, _, exts)| exts.iter().copied()).collect();
    all.sort_unstable();
    all.join(" ")
}

/// Extensions the defaults gained after `revision`: a list saved then receives them once.
pub fn captured_since(revision: u8) -> impl Iterator<Item = &'static str> {
    KNOWN.iter().filter(move |(_, added, _)| *added > revision).flat_map(|(_, _, exts)| exts.iter().copied())
}

/// `true` if the file name / URL ends with an extension from `list` (separated by spaces, line
/// breaks, commas or semicolons: the settings edit it as free text), or is a split archive part
/// (`.r00`, `.r01`…, `.z01`, `.z02`…, `.001`, `.002`…).
pub fn is_capturable(list: &str, name: &str) -> bool {
    let ext = extension(name);
    !ext.is_empty()
        && (is_split_archive(&ext)
            || list.split(|c: char| c.is_whitespace() || matches!(c, ',' | ';')).any(|e| e.trim_start_matches('.').eq_ignore_ascii_case(&ext)))
}

/// Lower-case extension of a file name, or of a URL's path. Only URLs lose their `?query` and
/// `#fragment`: in a file name, `#` and `?` are ordinary characters ("Clip #42.mp4").
fn extension(name: &str) -> String {
    let path = if name.contains("://") { name.split(['?', '#']).next().unwrap_or_default() } else { name };
    let file = path.rsplit(['/', '\\']).next().unwrap_or_default();
    file.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default()
}

fn is_split_archive(ext: &str) -> bool {
    match ext.as_bytes() {
        [b'r' | b'z', digits @ ..] => !digits.is_empty() && digits.len() <= 3 && digits.iter().all(u8::is_ascii_digit),
        digits => digits.len() == 3 && digits.iter().all(u8::is_ascii_digit),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_known_and_split_extensions() {
        let list = default_captured();
        assert!(is_capturable(&list, "https://x.io/a/Video.MP4?token=1"));
        assert!(is_capturable(&list, "archive.r01"));
        assert!(is_capturable(&list, "archive.r1"));
        assert!(is_capturable(&list, "backup.7z.001"));
        assert!(is_capturable(&list, "film.webm"));
        assert!(!is_capturable(&list, "page.html"));
        assert!(!is_capturable(&list, "noext"));
        assert!(!is_capturable(&list, "https://x.io/dir.zip/page"));
        assert!(!is_capturable(&list, "archive.rar5x"));
        assert!(is_capturable(&list, ".zip"), "a name that is only an extension (same answer as the extension)");
    }

    #[test]
    fn user_list_is_respected() {
        assert!(is_capturable(".iso, .ZIP", "a.zip"));
        assert!(!is_capturable("iso", "a.zip"));
        assert!(is_capturable("iso\nzip\tmkv", "a.zip"), "one per line, as typed in the settings");
        assert!(!is_capturable("", "a.zip"));
    }

    #[test]
    fn files_are_sorted_by_their_extension() {
        assert_eq!(Category::of("Film (1080p).MP4"), Category::Video);
        assert_eq!(Category::of("song.m4a"), Category::Music);
        assert_eq!(Category::of("part.r07"), Category::Archive);
        assert_eq!(Category::of("data.7z.002"), Category::Archive);
        assert_eq!(Category::of("setup.exe"), Category::Program);
        assert_eq!(Category::of("book.epub"), Category::Document);
        assert_eq!(Category::of("README"), Category::Other);
    }

    #[test]
    fn ai_models_have_their_own_category() {
        assert_eq!(Category::of("https://huggingface.co/x/resolve/main/llama-3-8b.Q4_K_M.gguf?download=true"), Category::Model);
        assert_eq!(Category::of("model-00001-of-00002.safetensors"), Category::Model);
        assert_eq!((Category::Model.label(false), Category::Model.label(true)), ("Modèles IA", "AI models"));
    }

    #[test]
    fn parts_of_a_split_zip_are_always_captured() {
        assert!(is_capturable("iso", "backup.z01"));
        assert_eq!(Category::of("backup.z12"), Category::Archive);
        assert_eq!(Category::of("old.z"), Category::Archive);
    }

    #[test]
    fn a_list_saved_before_a_revision_receives_only_what_it_added() {
        let added: Vec<&str> = captured_since(0).collect();

        assert!(added.contains(&"gguf") && added.contains(&"flatpak"));
        assert!(!added.contains(&"mp4"), "known before");
        assert_eq!(captured_since(CAPTURED_REVISION).count(), 0, "nothing newer than the latest revision");
    }

    #[test]
    fn the_latest_revision_is_the_highest_listed() {
        assert_eq!(KNOWN.iter().map(|(_, revision, _)| *revision).max(), Some(CAPTURED_REVISION));
    }

    /// Only `r` followed by digits is a part of a split archive: `.rtf` and `.rm` are not.
    #[test]
    fn extensions_starting_with_r_keep_their_own_category() {
        assert_eq!(Category::of("letter.rtf"), Category::Document);
        assert_eq!(Category::of("clip.rm"), Category::Video);
    }

    #[test]
    fn categories_are_named_in_french_and_in_english() {
        assert_eq!((Category::Video.label(false), Category::Video.label(true)), ("Vidéos", "Videos"));
        assert_eq!((Category::Archive.label(false), Category::Archive.label(true)), ("Compressés", "Archives"));
    }

    /// Regression: a `#` in a title was taken for a URL fragment → "Other" instead of "Videos".
    #[test]
    fn hash_and_question_mark_in_file_names() {
        assert_eq!(Category::of("Chine–USA _ terres rares #octogone93 (1080p).mp4"), Category::Video);
        assert_eq!(Category::of(r"C:\dl\Vidéos\Why? #1.mkv"), Category::Video);
        assert!(is_capturable(&default_captured(), "Clip #42.zip"));
        assert!(is_capturable(&default_captured(), "https://x.io/a.zip#section"));
        assert!(!is_capturable(&default_captured(), "https://x.io/page#a.zip"));
    }

    #[test]
    fn every_extension_belongs_to_one_category() {
        let list = default_captured();
        let all: Vec<&str> = list.split(' ').collect();
        let mut dedup = all.clone();
        dedup.dedup();
        assert_eq!(all.len(), dedup.len(), "duplicate extension in KNOWN");
        assert!(all.iter().all(|e| Category::of(&format!("x.{e}")) != Category::Other));
    }
}
