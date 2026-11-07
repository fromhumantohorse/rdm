//! Where downloads go on disk: a free or chosen name, the temporary files around them, the
//! Windows mark of a file from the Internet.

use super::*;

/// Every temporary file an unfinished download may leave behind, plus the file itself ("").
fn cleanup_suffixes() -> impl Iterator<Item = &'static str> {
    ["", engine::STATE_SUFFIX, ".rdm.tmp", ".video.part.rdm.tmp", ".audio.part.rdm.tmp"]
        .into_iter()
        .chain(engine::PART_SUFFIXES)
}

/// What removing an unfinished download (or deleting its file) erases: its temporary files, and
/// the file itself — unless it existed before ("overwrite") and the transfer has not written over
/// it yet (no resume point next to it: not started, failed before its first byte, or a split
/// download whose parts are still apart). That file is the user's, not this download's.
pub(super) fn leftovers(target: &Path, delete_file: bool, replaces: bool) -> Vec<PathBuf> {
    let untouched = replaces && !with_suffix(target, engine::STATE_SUFFIX).exists();
    let keep_file = !delete_file && untouched;
    cleanup_suffixes().filter(|s| !(keep_file && s.is_empty())).map(|s| with_suffix(target, s)).collect()
}

/// Chunks of an interrupted recording (`<name>.rec<N>.<track>`), e.g. left by a previous session.
pub(super) async fn remove_recording_leftovers(target: &Path) {
    let (Some(dir), Some(name)) = (target.parent(), target.file_name()) else { return };
    let name = name.to_string_lossy();
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else { return };
    while let Ok(Some(entry)) = entries.next_entry().await {
        if is_recording_part(&name, &entry.file_name().to_string_lossy()) {
            let _ = tokio::fs::remove_file(entry.path()).await;
        }
    }
}

/// `<name>.rec<digits>.video` / `.audio` exactly (see `Recording::part`): never another file
/// that merely starts the same way.
fn is_recording_part(name: &str, file: &str) -> bool {
    file.strip_prefix(name)
        .and_then(|rest| rest.strip_prefix(".rec"))
        .and_then(|rest| rest.split_once('.'))
        .is_some_and(|(ms, track)| !ms.is_empty() && ms.bytes().all(|b| b.is_ascii_digit()) && matches!(track, "video" | "audio"))
}

/// Mark-of-the-Web, as browsers do: SmartScreen / Office Protected View then warn before running
/// downloaded executables or macros. Zone only — the source URL is not recorded (privacy). macOS's
/// counterpart is the quarantine attribute: Gatekeeper checks the file before it first runs.
pub(super) fn mark_from_internet(path: &Path) {
    if cfg!(windows) {
        let _ = fs::write(with_suffix(path, ":Zone.Identifier"), "[ZoneTransfer]\r\nZoneId=3\r\n");
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let Ok(path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return };
        // flags (downloaded, not approved yet);time;program;event, as browsers write it.
        let value = format!("0081;{:08x};RDM;{}", unix_now(), uuid::Uuid::new_v4().as_hyphenated().to_string().to_uppercase());
        // SAFETY: NUL-terminated names and a value of the given length, all outliving the call.
        unsafe { libc::setxattr(path.as_ptr(), c"com.apple.quarantine".as_ptr(), value.as_ptr().cast(), value.len(), 0, 0) };
    }
}

/// Whether two paths name one file: Windows' and macOS's file systems ignore case by default.
fn same_file(a: &Path, b: &Path) -> bool {
    if cfg!(any(windows, target_os = "macos")) { a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase() } else { a == b }
}

/// A target can itself be another transfer's sidecar (`video.mp4.rdm`, for example). Reserve
/// both sets of files, even before either transfer has created its first file on disk.
fn files_overlap(a: &Path, b: &Path) -> bool {
    cleanup_suffixes().any(|left| cleanup_suffixes().any(|right| same_file(&with_suffix(a, left), &with_suffix(b, right))))
}

/// What a download of the list writes, for choosing new names without holding the list's lock
/// while the disk is looked at (a slow or sleeping disk must not freeze the window).
#[derive(Clone)]
pub(super) struct Planned {
    pub(super) id: DownloadId,
    target: PathBuf,
    completed: bool,
}

/// A removed entry still owns its path until its asynchronous cleanup ends, even when it was
/// completed: choosing "overwrite" must not put a new download under the pending deletion.
pub(super) fn reserved(entry: &Entry) -> Planned {
    Planned { id: entry.download.id, target: entry.download.target.clone(), completed: false }
}

/// The files the list writes (or wrote).
pub(super) fn planned(entries: &[Entry]) -> Vec<Planned> {
    entries
        .iter()
        .map(|e| Planned { id: e.download.id, target: e.download.target.clone(), completed: *e.download.status() == Status::Completed })
        .collect()
}

/// Where a new download of `name` goes, following the settings when a file of that name exists
/// (`None`: skip it). Overwriting never follows a symbolic link (it could point anywhere), nor
/// takes a name another download of the list will write.
pub(super) fn target_for(settings: &Settings, name: &str, taken: &[Planned], except: Option<DownloadId>) -> Option<PathBuf> {
    let dir = settings.target_dir(name);
    let path = dir.join(name);
    match settings.existing {
        // Asked beforehand when possible (see `resolved`, `add_from_browser`); otherwise a new name.
        ExistingFile::Rename | ExistingFile::Ask => Some(unique_path(&dir, name, taken, except)),
        ExistingFile::Skip if path.is_file() => None,
        ExistingFile::Skip => Some(unique_path(&dir, name, taken, except)),
        ExistingFile::Overwrite => {
            let link = fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink());
            // Another download of the list still writing (or to write) that file keeps it: two
            // transfers into one file would corrupt both. A completed one gives it up.
            let planned = taken.iter().any(|e| {
                Some(e.id) != except && files_overlap(&e.target, &path) && (!e.completed || !same_file(&e.target, &path))
            });
            if link || planned || path.is_dir() {
                return Some(unique_path(&dir, name, taken, except));
            }
            // Leftovers of an earlier, unrelated download of that name must not be resumed.
            for suffix in cleanup_suffixes().filter(|s| !s.is_empty()) {
                let _ = fs::remove_file(with_suffix(&path, suffix));
            }
            Some(path)
        }
    }
}

/// A free path for `name` in `dir`: neither on disk (with or without leftovers of an unfinished
/// download) nor planned by another entry of the list (`except`: the entry being renamed).
pub(super) fn unique_path(dir: &Path, name: &str, taken: &[Planned], except: Option<DownloadId>) -> PathBuf {
    let (stem, ext) = name.rsplit_once('.').map_or((name, None), |(s, e)| (s, Some(e)));
    (0u32..)
        .map(|i| match (i, ext) {
            (0, _) => dir.join(name),
            (_, Some(ext)) => dir.join(format!("{stem} ({i}).{ext}")),
            (_, None) => dir.join(format!("{stem} ({i})")),
        })
        .find(|p| {
            !cleanup_suffixes().any(|suffix| fs::symlink_metadata(with_suffix(p, suffix)).is_ok())
                && !taken.iter().any(|e| files_overlap(&e.target, p) && Some(e.id) != except)
        })
        .expect("unbounded range always yields a free name")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_deletions_and_sidecars_keep_their_names_reserved() {
        let dir = std::env::temp_dir().join(format!("rdm-reserved-{}", uuid::Uuid::new_v4()));
        let settings = Settings { download_dir: dir.clone(), categorize: false, existing: ExistingFile::Overwrite, ..Settings::default() };
        let mut download = Download::new("https://example.com/video.mp4".parse().unwrap(), dir.join("video.mp4"), 1);
        download.start().unwrap();
        download.complete().unwrap();
        let entry = Entry::new(download, vec![], 0, 0);
        assert_eq!(target_for(&settings, "video.mp4", &planned(&[entry]), None), Some(dir.join("video.mp4")));

        let entry = Entry::new(Download::new("https://example.com/video.mp4".parse().unwrap(), dir.join("video.mp4"), 1), vec![], 0, 0);
        let taken = [reserved(&entry)];
        assert_eq!(target_for(&settings, "video.mp4", &taken, None), Some(dir.join("video (1).mp4")));
        assert_eq!(unique_path(&dir, "video.mp4.rdm", &taken, None), dir.join("video.mp4 (1).rdm"));
        assert_eq!(target_for(&settings, "video.mp4.rdm", &taken, None), Some(dir.join("video.mp4 (1).rdm")));
        let sidecar = [Planned { id: DownloadId::new(), target: dir.join("video.mp4.rdm"), completed: true }];
        assert_eq!(target_for(&settings, "video.mp4", &sidecar, None), Some(dir.join("video (1).mp4")));
    }

    #[test]
    fn orphaned_parts_are_not_reused_by_an_unrelated_download() {
        let dir = std::env::temp_dir().join(format!("rdm-orphan-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("video.mp4.audio.part"), b"old audio").unwrap();
        assert_eq!(unique_path(&dir, "video.mp4", &[], None), dir.join("video (1).mp4"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn dangling_symlinks_do_not_count_as_free_names() {
        let dir = std::env::temp_dir().join(format!("rdm-symlink-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        std::os::unix::fs::symlink(dir.join("missing"), dir.join("report.pdf")).unwrap();
        assert_eq!(unique_path(&dir, "report.pdf", &[], None), dir.join("report (1).pdf"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cleanup_covers_every_temporary_file() {
        let all: Vec<_> = cleanup_suffixes().collect();
        for s in [".rdm", ".video.part", ".video.part.ok", ".audio.part.rdm", ".mux.tmp"] {
            assert!(all.contains(&s), "{s}");
        }
    }

    #[test]
    fn only_recording_chunks_count_as_leftovers() {
        let target = PathBuf::from("dl").join("Clip.mp4");
        let recording = Recording { id: DownloadId::new(), target, parts: HashMap::new(), last_heard: Instant::now(), files: HashMap::new(), pending: Arc::default() };
        let part = recording.part(3, Track::Audio);
        assert!(is_recording_part("Clip.mp4", &part.file_name().unwrap().to_string_lossy()));
        assert!(is_recording_part("Clip.mp4", "Clip.mp4.rec12.video"));
        for other in ["Clip.mp4.recipe.txt", "Clip.mp4.rec.video", "Clip.mp4.rec1.video.bak", "Clip.mp4.rec1x.audio", "Clip.mp4"] {
            assert!(!is_recording_part("Clip.mp4", other), "{other}");
        }
    }

    /// Regression: two links differing by case in their names (`A.zip`, `a.zip`) were both planned
    /// under one file on Windows and macOS: two transfers corrupting each other.
    #[test]
    fn a_name_differing_by_case_is_taken_where_the_file_system_ignores_case() {
        let dir = std::env::temp_dir().join(format!("rdm-case-{}", std::process::id()));
        let planned = [Planned { id: DownloadId::new(), target: dir.join("Report.PDF"), completed: false }];

        let chosen = unique_path(&dir, "report.pdf", &planned, None);

        let expected = if cfg!(any(windows, target_os = "macos")) { "report (1).pdf" } else { "report.pdf" };
        assert_eq!(chosen, dir.join(expected));
    }

    /// Regression: with "overwrite", removing a download that failed before writing anything
    /// deleted the user's existing file of that name.
    #[test]
    fn removal_spares_a_file_the_download_has_not_written_over() {
        let dir = std::env::temp_dir().join(format!("rdm-leftovers-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("report.pdf");
        std::fs::write(&target, b"the user's file").unwrap();
        let erases_file = |delete_file, replaces| leftovers(&target, delete_file, replaces).contains(&target);

        assert!(!erases_file(false, true), "not written over yet: kept");
        assert!(erases_file(true, true), "\"delete the file\" deletes it");
        assert!(erases_file(false, false), "a partial file of this download's own");
        std::fs::write(with_suffix(&target, engine::STATE_SUFFIX), b"[]").unwrap();
        assert!(erases_file(false, true), "the transfer has written over it: a partial file now");
        assert!(leftovers(&target, false, true).contains(&with_suffix(&target, engine::STATE_SUFFIX)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
