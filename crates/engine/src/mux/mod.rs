//! Remuxes a video-only and an audio-only fragmented MP4 (DASH) into one MP4. No re-encoding:
//! boxes are copied, track ids renumbered and fragments interleaved by decode time.

mod boxes;
mod track;

use std::{
    fs::File,
    io::{BufWriter, Write},
    path::Path,
};

use boxes::{FourCc, atoms, boxed, full_box_field, locate, read_u32, version, write_u32};
use memmap2::Mmap;
use track::Track;

#[derive(Debug, thiserror::Error)]
pub enum MuxError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid mp4: {0}")]
    Invalid(#[from] Mp4Error),
}

/// What is wrong with an MP4 file that cannot be merged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Mp4Error {
    #[error("truncated box")]
    TruncatedBox,
    #[error("bad box header")]
    BadBoxHeader,
    #[error("box too large")]
    BoxTooLarge,
    #[error("missing box")]
    MissingBox,
    #[error("field out of bounds")]
    FieldOutOfBounds,
    #[error("missing moov")]
    MissingMoov,
    #[error("not a fragmented mp4")]
    NotFragmented,
    #[error("missing trak")]
    MissingTrak,
    #[error("missing mdhd")]
    MissingMdhd,
    #[error("bad timescale")]
    BadTimescale,
    #[error("missing trex")]
    MissingTrex,
    #[error("missing tfdt")]
    MissingTfdt,
    #[error("bad tfdt")]
    BadTfdt,
    #[error("no fragments")]
    NoFragments,
    #[error("missing mvhd")]
    MissingMvhd,
    #[error("truncated mvhd")]
    TruncatedMvhd,
    #[error("overlapping boxes")]
    OverlappingBoxes,
}

/// Builds the combined `ftyp` + `moov` header (video = track 1, audio = track 2), with real
/// durations: DASH/HLS init segments declare 0, which breaks the seek bar and duration display.
fn header(video: &Track<'_>, audio: &Track<'_>) -> Result<Vec<u8>, MuxError> {
    let mut mvhd = video.mvhd.to_vec();
    // Smallest valid mvhd (version 0): 8-byte header + 100-byte body ending with next_track_ID.
    if mvhd.len() < 108 {
        return Err(MuxError::Invalid(Mp4Error::TruncatedMvhd));
    }
    let next_id_at = mvhd.len() - 4;
    write_u32(&mut mvhd, next_id_at, 3)?;
    let body = locate(&mvhd, &[b"mvhd"]).ok_or(MuxError::Invalid(Mp4Error::MissingMvhd))?;
    let movie_ts = read_u32(&mvhd, full_box_field(&mvhd, &body, 8, 16)).filter(|&t| t > 0).unwrap_or(1000);
    let (v_dur, a_dur) = (video.duration_in(movie_ts), audio.duration_in(movie_ts));
    let total = v_dur.max(a_dur);
    write_duration(&mut mvhd, &[b"mvhd"], (12, 20), total)?;

    let mehd = boxed(b"mehd", &[&[1, 0, 0, 0], &total.to_be_bytes()]);
    let (vtrex, atrex) = (video.trex_with_id(1)?, audio.trex_with_id(2)?);
    let mvex = boxed(b"mvex", &[&mehd, &vtrex, &atrex]);
    let moov = boxed(b"moov", &[&mvhd, &video.trak_with(1, v_dur)?, &audio.trak_with(2, a_dur)?, &mvex]);
    Ok([video.ftyp.unwrap_or_default(), &moov].concat())
}

/// Writes a duration field whose width depends on the full-box version (u32 in v0, u64 in v1).
fn write_duration(buf: &mut [u8], path: &[&FourCc], (v0, v1): (usize, usize), value: u64) -> Result<(), MuxError> {
    let body = locate(buf, path).ok_or(MuxError::Invalid(Mp4Error::MissingBox))?;
    if version(buf, &body) == 1 {
        let at = body.start + 4 + v1;
        buf.get_mut(at..at + 8).ok_or(MuxError::Invalid(Mp4Error::FieldOutOfBounds))?.copy_from_slice(&value.to_be_bytes());
        Ok(())
    } else {
        write_u32(buf, body.start + 4 + v0, u32::try_from(value).unwrap_or(u32::MAX))
    }
}

/// Sum of the sample durations of a fragment (all `trun`s of its `traf`), in media ticks.
fn fragment_duration(frag: &[u8], trex_default: u32) -> Option<u64> {
    let traf = locate(frag, &[b"moof", b"traf"])?;
    let traf = frag.get(traf)?;
    let tfhd = locate(traf, &[b"tfhd"])?;
    let flags = read_u32(traf, tfhd.start)? & 0x00ff_ffff;
    let mut at = tfhd.start + 8; // version/flags + track_ID
    if flags & 0x01 != 0 {
        at += 8; // base_data_offset
    }
    if flags & 0x02 != 0 {
        at += 4; // sample_description_index
    }
    let default = if flags & 0x08 != 0 { read_u32(traf, at)? } else { trex_default };

    let mut total = 0u64;
    for trun in atoms(traf).map_while(Result::ok).filter(|a| &a.kind == b"trun") {
        let b = &trun.bytes[trun.head..];
        let flags = read_u32(b, 0)? & 0x00ff_ffff;
        let count = usize::try_from(read_u32(b, 4)?).ok()?;
        if flags & 0x100 == 0 {
            total = total.saturating_add(u64::try_from(count).ok()? * u64::from(default));
            continue;
        }
        let first: usize = 8 + if flags & 0x01 != 0 { 4 } else { 0 } + if flags & 0x04 != 0 { 4 } else { 0 };
        let stride = [0x100, 0x200, 0x400, 0x800].iter().filter(|&&f| flags & f != 0).count() * 4;
        if first.checked_add(count.checked_mul(stride)?)? > b.len() {
            return None;
        }
        for i in 0..count {
            total = total.saturating_add(u64::from(read_u32(b, first + i * stride)?));
        }
    }
    Some(total)
}

/// Writes a fragment renumbered (`mfhd` sequence, `tfhd` track id). The two fields are patched on
/// the way out: the fragment itself (megabytes of media) is never copied.
fn write_fragment(w: &mut impl Write, frag: &[u8], track: u32, seq: u32) -> Result<(), MuxError> {
    let field = |path: &[&FourCc]| {
        let at = locate(frag, path).ok_or(MuxError::Invalid(Mp4Error::MissingBox))?.start + 4;
        match at.checked_add(4) {
            Some(end) if end <= frag.len() => Ok(at),
            _ => Err(MuxError::Invalid(Mp4Error::FieldOutOfBounds)),
        }
    };
    let mut patches = [(field(&[b"moof", b"mfhd"])?, seq), (field(&[b"moof", b"traf", b"tfhd"])?, track)];
    patches.sort_unstable_by_key(|&(at, _)| at);
    if patches[0].0 + 4 > patches[1].0 {
        return Err(MuxError::Invalid(Mp4Error::OverlappingBoxes));
    }
    let mut pos = 0;
    for (at, value) in patches {
        w.write_all(&frag[pos..at])?;
        w.write_all(&value.to_be_bytes())?;
        pos = at + 4;
    }
    Ok(w.write_all(&frag[pos..])?)
}

fn merge_bytes(video: &[u8], audio: &[u8], out: &mut impl Write) -> Result<(), MuxError> {
    let (v, a) = (Track::parse(video)?, Track::parse(audio)?);
    out.write_all(&header(&v, &a)?)?;

    let (mut vi, mut ai) = (v.fragments.iter().peekable(), a.fragments.iter().peekable());
    for seq in 1u32.. {
        let (frag, track) = match (vi.peek(), ai.peek()) {
            (Some(x), Some(y)) if x.0 <= y.0 => (vi.next(), 1),
            (Some(_), None) => (vi.next(), 1),
            (_, Some(_)) => (ai.next(), 2),
            (None, None) => break,
        };
        if let Some((_, bytes)) = frag {
            write_fragment(out, bytes, track, seq)?;
        }
    }
    Ok(())
}

/// Written next to the output, then renamed over it: a failed or interrupted merge never leaves a
/// truncated file under the final name.
pub(crate) const TMP_SUFFIX: &str = ".mux.tmp";

pub fn merge(video: &Path, audio: &Path, out: &Path) -> Result<(), MuxError> {
    // SAFETY: the inputs are our own finished temporary files; nothing else writes to them.
    let (v, a) = unsafe { (Mmap::map(&File::open(video)?)?, Mmap::map(&File::open(audio)?)?) };
    let tmp = crate::with_suffix(out, TMP_SUFFIX);
    let written = File::create(&tmp).map_err(MuxError::from).and_then(|file| {
        let mut w = BufWriter::with_capacity(1 << 20, file);
        merge_bytes(&v, &a, &mut w)?;
        // On disk before it takes the final name: the parts are deleted next, and a power cut must
        // not leave an empty file in their place. Closed here, before the rename (Windows needs it).
        Ok(w.into_inner().map_err(std::io::IntoInnerError::into_error)?.sync_all()?)
    });
    match written {
        Ok(()) => Ok(std::fs::rename(&tmp, out)?),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full(kind: &FourCc, payload: &[u8]) -> Vec<u8> {
        boxed(kind, &[&[0, 0, 0, 0], payload])
    }

    fn track(id: u32, timescale: u32, times: &[u32]) -> Vec<u8> {
        let tkhd = full(b"tkhd", &[&[0; 8][..], &id.to_be_bytes(), &[0; 64]].concat());
        let mdhd = full(b"mdhd", &[&[0; 8][..], &timescale.to_be_bytes(), &[0; 8]].concat());
        let trak = boxed(b"trak", &[&tkhd, &boxed(b"mdia", &[&mdhd])]);
        let trex = full(b"trex", &[id.to_be_bytes(), [0; 4], [0; 4], [0; 4], [0; 4]].concat());
        let mvhd = full(b"mvhd", &[&[0; 92][..], &2u32.to_be_bytes()].concat());
        let moov = boxed(b"moov", &[&mvhd, &trak, &boxed(b"mvex", &[&trex])]);
        let mut file = [boxed(b"ftyp", &[b"dash"]), moov].concat();
        for (i, t) in times.iter().enumerate() {
            let mfhd = full(b"mfhd", &(i as u32 + 1).to_be_bytes());
            let tfhd = full(b"tfhd", &id.to_be_bytes());
            let tfdt = full(b"tfdt", &t.to_be_bytes());
            file.extend(boxed(b"moof", &[&mfhd, &boxed(b"traf", &[&tfhd, &tfdt])]));
            file.extend(boxed(b"mdat", &[&[id as u8; 3]]));
        }
        file
    }

    #[test]
    fn merges_and_interleaves_by_time() {
        let video = track(1, 1000, &[0, 2000, 4000]); // 0s, 2s, 4s
        let audio = track(1, 48000, &[0, 144_000]); // 0s, 3s
        let mut out = Vec::new();
        merge_bytes(&video, &audio, &mut out).unwrap();

        let top: Vec<FourCc> = atoms(&out).map(|a| a.unwrap().kind).collect();
        assert_eq!(&top[..2], [*b"ftyp", *b"moov"]);

        let moov = locate(&out, &[b"moov"]).unwrap();
        let traks: Vec<u32> = atoms(&out[moov.clone()])
            .map_while(Result::ok)
            .filter(|a| &a.kind == b"trak")
            .map(|a| read_u32(a.bytes, locate(a.bytes, &[b"trak", b"tkhd"]).unwrap().start + 12).unwrap())
            .collect();
        assert_eq!(traks, [1, 2]);

        // Fragment track order by time: v0(0s) a0(0s) v1(2s) a1(3s) v2(4s); sequence numbers 1..=5.
        let frags: Vec<(u32, u32)> = atoms(&out)
            .map_while(Result::ok)
            .filter(|a| &a.kind == b"moof")
            .map(|a| {
                let seq = read_u32(a.bytes, locate(a.bytes, &[b"moof", b"mfhd"]).unwrap().start + 4).unwrap();
                let id = read_u32(a.bytes, locate(a.bytes, &[b"moof", b"traf", b"tfhd"]).unwrap().start + 4).unwrap();
                (seq, id)
            })
            .collect();
        assert_eq!(frags, [(1, 1), (2, 2), (3, 1), (4, 2), (5, 1)]);
    }

    /// Hostile input must yield an error, never a panic: every truncation and a sweep of byte flips.
    #[test]
    fn survives_truncated_and_corrupted_input() {
        let video = track(1, 1000, &[0, 2000]);
        let audio = track(1, 48000, &[0]);
        for len in 0..video.len() {
            let _ = merge_bytes(&video[..len], &audio, &mut Vec::new());
        }
        for i in 0..video.len() {
            for flip in [0x00, 0xFF, 0x01, 0x80] {
                let mut bad = video.clone();
                bad[i] = flip;
                let _ = merge_bytes(&bad, &audio, &mut Vec::new());
            }
        }
    }

    /// What a browser recording looks like: out-of-order and repeated fragments, then a tail cut
    /// mid-box when the recording stopped. Everything complete must survive, once, in order.
    #[test]
    fn tolerates_recorded_streams() {
        let mut video = track(1, 1000, &[0, 4000, 2000, 2000]);
        video.extend_from_slice(&boxed(b"moof", &[&[0u8; 40]])[..20]); // truncated tail
        let audio = track(1, 48000, &[0]);
        let mut out = Vec::new();
        merge_bytes(&video, &audio, &mut out).unwrap();
        let video_times: Vec<u32> = atoms(&out)
            .map_while(Result::ok)
            .filter(|a| &a.kind == b"moof")
            .filter(|a| read_u32(a.bytes, locate(a.bytes, &[b"moof", b"traf", b"tfhd"]).unwrap().start + 4) == Some(1))
            .map(|a| read_u32(a.bytes, locate(a.bytes, &[b"moof", b"traf", b"tfdt"]).unwrap().start + 4).unwrap())
            .collect();
        assert_eq!(video_times, [0, 2000, 4000]);
    }

    #[test]
    fn output_appears_only_once_complete() {
        let dir = std::env::temp_dir().join(format!("rdm-mux-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (v, a, out) = (dir.join("v.part"), dir.join("a.part"), dir.join("out.mp4"));
        let tmp = crate::with_suffix(&out, TMP_SUFFIX);

        std::fs::write(&v, b"not an mp4").unwrap();
        std::fs::write(&a, track(1, 48000, &[0])).unwrap();
        assert!(merge(&v, &a, &out).is_err());
        assert!(!out.exists() && !tmp.exists(), "a failed merge leaves nothing behind");

        std::fs::write(&v, track(1, 1000, &[0, 2000])).unwrap();
        merge(&v, &a, &out).unwrap();
        assert!(out.exists() && !tmp.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_non_fragmented() {
        let plain = [boxed(b"ftyp", &[b"isom"]), boxed(b"moov", &[])].concat();

        let merged = merge_bytes(&plain, &plain, &mut Vec::new());

        assert!(matches!(merged, Err(MuxError::Invalid(Mp4Error::NotFragmented))), "{merged:?}");
    }

    #[test]
    fn rejects_a_file_without_any_movie_header() {
        let fragment_only = boxed(b"moof", &[&[0u8; 8]]);

        let merged = merge_bytes(&fragment_only, &fragment_only, &mut Vec::new());

        assert!(matches!(merged, Err(MuxError::Invalid(Mp4Error::MissingMoov))), "{merged:?}");
    }
}
