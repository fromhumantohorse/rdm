//! The resume state next to an unfinished download (`.rdm`): how far each piece went, and which
//! version of the file on the server they belong to.

use std::path::Path;

use domain::Segment;
use serde::{Deserialize, Serialize};
use tokio::fs::{self, OpenOptions};

/// What a resume state file holds: the file's version on the server when the download began, and
/// how far each piece went.
#[derive(Serialize, Deserialize)]
struct Saved {
    #[serde(default)]
    version: Option<String>,
    segments: Vec<Segment>,
}

/// The pieces for the resume state and the progress bar, in file order, finished neighbours joined:
/// a long download split many times keeps a short list (a smaller state file, less to draw).
pub(super) fn compact(mut segs: Vec<Segment>) -> Vec<Segment> {
    segs.sort_unstable_by_key(|s| s.start);
    let mut out: Vec<Segment> = Vec::with_capacity(segs.len());
    for s in segs {
        match out.last_mut() {
            Some(last) if last.is_done() && s.is_done() && last.end.checked_add(1) == Some(s.start) => {
                last.end = s.end;
                last.pos = s.end.saturating_add(1);
            }
            _ => out.push(s),
        }
    }
    out
}

/// Syncs the data through a handle of its own (the connections keep writing through theirs),
/// then records how far each piece went.
pub(super) async fn persist(target: &Path, state: &Path, version: Option<&str>, segs: &[Segment]) -> std::io::Result<()> {
    OpenOptions::new().write(true).open(target).await?.sync_data().await?;
    save_state(state, version, segs).await
}

/// The pieces to resume, or `None` to start over: no usable state, or the file changed on the
/// server since (another size, another version) — its new bytes would not continue the old ones.
pub(super) async fn load_state(state: &Path, target: &Path, size: u64, version: Option<&str>) -> Option<Vec<Segment>> {
    fs::metadata(target).await.ok().filter(|m| m.len() == size)?;
    let saved = parse_state(&fs::read(state).await.ok()?)?;
    let changed = matches!((saved.version.as_deref(), version), (Some(then), Some(now)) if then != now);
    if changed {
        return None;
    }
    covers_exactly(saved.segments, size)
}

/// A state file, of this version of RDM or an older one (the bare list of pieces).
fn parse_state(bytes: &[u8]) -> Option<Saved> {
    serde_json::from_slice::<Saved>(bytes)
        .or_else(|_| serde_json::from_slice::<Vec<Segment>>(bytes).map(|segments| Saved { version: None, segments }))
        .ok()
}

/// Rejects a tampered or stale state file: segments must tile `[0, size)` with no gap or overlap.
fn covers_exactly(mut segs: Vec<Segment>, size: u64) -> Option<Vec<Segment>> {
    segs.sort_unstable_by_key(|s| s.start);
    let mut next = 0u64;
    for s in &segs {
        if s.start != next || s.end < s.start || s.pos < s.start {
            return None;
        }
        next = s.end.checked_add(1)?;
    }
    (next == size).then_some(segs)
}

async fn save_state(state: &Path, version: Option<&str>, segs: &[Segment]) -> std::io::Result<()> {
    let saved = Saved { version: version.map(str::to_owned), segments: segs.to_vec() };
    crate::write_atomic(state, &serde_json::to_vec(&saved)?).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_must_tile_the_file() {
        let ok = vec![Segment::new(50, 99), Segment::new(0, 49)];
        assert!(covers_exactly(ok, 100).is_some());
        assert!(covers_exactly(vec![Segment::new(0, 49), Segment::new(60, 99)], 100).is_none());
        assert!(covers_exactly(vec![Segment::new(0, 49)], 100).is_none());
        assert!(covers_exactly(vec![Segment::new(0, u64::MAX)], 100).is_none());
    }

    #[test]
    fn finished_neighbours_are_joined() {
        let done = |start, end| Segment { start, pos: end + 1, end };
        let live = Segment { start: 20, pos: 25, end: 29 };
        // Out of order (pieces split later come last), one overshooting its end.
        let segs = vec![done(30, 39), Segment { pos: 22, ..done(10, 19) }, done(0, 9), live];
        let joined = compact(segs);
        assert_eq!(joined, [done(0, 19), live, done(30, 39)]);
        assert!(covers_exactly(joined, 40).is_some(), "still tiles the file");
    }

    #[tokio::test]
    async fn resumes_only_the_same_version_of_the_file() {
        let dir = std::env::temp_dir().join(format!("rdm-resume-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (target, state) = (dir.join("f.bin"), dir.join("f.bin.rdm"));
        std::fs::write(&target, vec![0u8; 100]).unwrap();
        let segs = [Segment::new(0, 49), Segment::new(50, 99)];
        let v1 = Some("date:Wed, 21 Oct 2025 07:28:00 GMT");

        save_state(&state, v1, &segs).await.unwrap();
        assert!(load_state(&state, &target, 100, v1).await.is_some(), "same version: resumed");
        assert!(load_state(&state, &target, 100, None).await.is_some(), "the server no longer says: resumed");
        assert!(load_state(&state, &target, 100, Some("date:Thu, 22 Oct 2025 07:28:00 GMT")).await.is_none(), "changed: start over");
        assert!(load_state(&state, &target, 200, v1).await.is_none(), "another size: start over");
        // A state written by an older RDM (the bare list of pieces).
        std::fs::write(&state, serde_json::to_vec(&segs).unwrap()).unwrap();
        assert!(load_state(&state, &target, 100, v1).await.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
