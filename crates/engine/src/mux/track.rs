//! One input track: its init segment and fragments, as a recorded or downloaded stream holds them.

use super::{
    Mp4Error, MuxError,
    boxes::{Atom, FourCc, atoms, full_box_field, locate, patch, read_u32, read_u64, version},
    fragment_duration, write_duration,
};

pub(super) struct Track<'a> {
    pub(super) ftyp: Option<&'a [u8]>,
    pub(super) mvhd: &'a [u8],
    trak: &'a [u8],
    trex: &'a [u8],
    /// Media timescale (ticks per second).
    timescale: u32,
    /// End of the last sample, in media ticks: the real track duration.
    end: u64,
    /// (decode time in seconds, moof…mdat bytes)
    pub(super) fragments: Vec<(f64, &'a [u8])>,
}

impl<'a> Track<'a> {
    /// Parses one track's init segment and fragments. Tolerates what a recorded stream can hold:
    /// a truncated last box (recording stopped mid-segment), repeated init segments, duplicated
    /// or out-of-order fragments. If the init segment changed (quality switch), the group of
    /// fragments with the most data wins: mixing encodings would not decode.
    pub(super) fn parse(buf: &'a [u8]) -> Result<Self, MuxError> {
        let mut ftyp = None;
        // (init segment, its fragments) in order of appearance; identical inits are merged.
        let mut groups: Vec<(Atom<'a>, Vec<&'a [u8]>)> = Vec::new();
        let mut moof_start = None;
        for atom in atoms(buf) {
            let Ok(atom) = atom else { break }; // truncated tail: keep everything before it
            match &atom.kind {
                b"ftyp" => ftyp = Some(atom.bytes),
                b"moov" => {
                    if !groups.last().is_some_and(|(init, _)| init.bytes == atom.bytes) {
                        groups.push((atom, Vec::new()));
                    }
                }
                b"moof" => moof_start = Some(atom.start),
                b"mdat" => {
                    if let (Some(start), Some((_, frags))) = (moof_start.take(), groups.last_mut()) {
                        frags.push(&buf[start..atom.start + atom.bytes.len()]);
                    }
                }
                _ => {}
            }
        }
        let (moov, raw) = groups
            .into_iter()
            .max_by_key(|(_, frags)| frags.iter().map(|f| f.len()).sum::<usize>())
            .ok_or(MuxError::Invalid(Mp4Error::MissingMoov))?;
        let body = &moov.bytes[moov.head..];
        let child = |parent: &'a [u8], kind: &FourCc| atoms(parent).map_while(Result::ok).find(|a| &a.kind == kind);
        let mvex = child(body, b"mvex").ok_or(MuxError::Invalid(Mp4Error::NotFragmented))?;
        let mvex_body = &mvex.bytes[mvex.head..];
        let trak = child(body, b"trak").ok_or(MuxError::Invalid(Mp4Error::MissingTrak))?.bytes;

        let mdhd = locate(trak, &[b"trak", b"mdia", b"mdhd"]).ok_or(MuxError::Invalid(Mp4Error::MissingMdhd))?;
        let timescale = read_u32(trak, full_box_field(trak, &mdhd, 8, 16)).filter(|&t| t > 0).ok_or(MuxError::Invalid(Mp4Error::BadTimescale))?;
        let trex = child(mvex_body, b"trex").ok_or(MuxError::Invalid(Mp4Error::MissingTrex))?.bytes;
        let default_duration = read_u32(trex, 8 + 12).unwrap_or(0);

        let mut end = 0u64;
        let mut timed = raw
            .into_iter()
            .map(|frag| {
                let tfdt = locate(frag, &[b"moof", b"traf", b"tfdt"]).ok_or(MuxError::Invalid(Mp4Error::MissingTfdt))?;
                let t = match version(frag, &tfdt) {
                    1 => read_u64(frag, tfdt.start + 4),
                    _ => read_u32(frag, tfdt.start + 4).map(u64::from),
                }
                .ok_or(MuxError::Invalid(Mp4Error::BadTfdt))?;
                end = end.max(t.saturating_add(fragment_duration(frag, default_duration).unwrap_or(0)));
                Ok((t, frag))
            })
            .collect::<Result<Vec<_>, MuxError>>()?;
        // Recorded streams may repeat a fragment (re-fetch after a seek) or append out of order.
        timed.sort_by_key(|(t, _)| *t);
        timed.dedup_by_key(|(t, _)| *t);
        let fragments: Vec<(f64, &[u8])> = timed.into_iter().map(|(t, f)| (t as f64 / f64::from(timescale), f)).collect();
        if fragments.is_empty() {
            return Err(MuxError::Invalid(Mp4Error::NoFragments));
        }

        Ok(Self {
            ftyp,
            mvhd: child(body, b"mvhd").ok_or(MuxError::Invalid(Mp4Error::MissingMvhd))?.bytes,
            trak,
            trex,
            timescale,
            end,
            fragments,
        })
    }

    /// Track box renumbered to `id`, with its real duration (`movie_duration` in movie ticks).
    pub(super) fn trak_with(&self, id: u32, movie_duration: u64) -> Result<Vec<u8>, MuxError> {
        let mut trak = self.trak.to_vec();
        patch(&mut trak, &[b"trak", b"tkhd"], |b, r| full_box_field(b, r, 8, 16), id)?;
        write_duration(&mut trak, &[b"trak", b"tkhd"], (16, 24), movie_duration)?;
        write_duration(&mut trak, &[b"trak", b"mdia", b"mdhd"], (12, 20), self.end)?;
        Ok(trak)
    }

    /// Duration converted to another timescale.
    pub(super) fn duration_in(&self, timescale: u32) -> u64 {
        let d = u128::from(self.end) * u128::from(timescale) / u128::from(self.timescale);
        u64::try_from(d).unwrap_or(u64::MAX)
    }

    pub(super) fn trex_with_id(&self, id: u32) -> Result<Vec<u8>, MuxError> {
        let mut trex = self.trex.to_vec();
        patch(&mut trex, &[b"trex"], |_, r| r.start + 4, id)?;
        Ok(trex)
    }
}
