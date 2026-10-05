//! ISO BMFF boxes ("atoms"): reading them, finding one by its path, patching a field, building one.

use std::ops::Range;

use super::{Mp4Error, MuxError};

pub(super) type FourCc = [u8; 4];

pub(super) struct Atom<'a> {
    pub(super) kind: FourCc,
    pub(super) start: usize,
    pub(super) head: usize,
    pub(super) bytes: &'a [u8],
}

pub(super) fn atoms(buf: &[u8]) -> impl Iterator<Item = Result<Atom<'_>, MuxError>> {
    let mut pos = 0;
    std::iter::from_fn(move || {
        (pos < buf.len()).then(|| {
            let atom = parse_at(buf, pos);
            pos = atom.as_ref().map_or(buf.len(), |a| pos + a.bytes.len());
            atom
        })
    })
}

fn parse_at(buf: &[u8], pos: usize) -> Result<Atom<'_>, MuxError> {
    let bad = MuxError::Invalid(Mp4Error::BadBoxHeader);
    let h = buf.get(pos..pos + 8).ok_or(MuxError::Invalid(Mp4Error::TruncatedBox))?;
    let kind: FourCc = h[4..8].try_into().map_err(|_| MuxError::Invalid(Mp4Error::TruncatedBox))?;
    let (size, head) = match u32::from_be_bytes([h[0], h[1], h[2], h[3]]) {
        0 => (buf.len() - pos, 8),
        1 => {
            let large = buf.get(pos + 8..pos + 16).ok_or(MuxError::Invalid(Mp4Error::TruncatedBox))?;
            (usize::try_from(u64::from_be_bytes(large.try_into().map_err(|_| MuxError::Invalid(Mp4Error::TruncatedBox))?)).map_err(|_| MuxError::Invalid(Mp4Error::BoxTooLarge))?, 16)
        }
        n => (n as usize, 8),
    };
    let end = pos.checked_add(size).filter(|&e| e <= buf.len() && size >= head).ok_or(bad)?;
    Ok(Atom { kind, start: pos, head, bytes: &buf[pos..end] })
}

/// Absolute range of the body of the box at `path` (e.g. `moof/traf/tfdt`).
pub(super) fn locate(buf: &[u8], path: &[&FourCc]) -> Option<Range<usize>> {
    let (first, rest) = path.split_first()?;
    let atom = atoms(buf).map_while(Result::ok).find(|a| &a.kind == *first)?;
    let body = atom.start + atom.head..atom.start + atom.bytes.len();
    if rest.is_empty() {
        return Some(body);
    }
    let inner = locate(&buf[body.clone()], rest)?;
    Some(body.start + inner.start..body.start + inner.end)
}

pub(super) fn read_u32(buf: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(buf.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

pub(super) fn read_u64(buf: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(buf.get(at..at.checked_add(8)?)?.try_into().ok()?))
}

pub(super) fn write_u32(buf: &mut [u8], at: usize, v: u32) -> Result<(), MuxError> {
    buf.get_mut(at..at + 4).ok_or(MuxError::Invalid(Mp4Error::FieldOutOfBounds))?.copy_from_slice(&v.to_be_bytes());
    Ok(())
}

/// Offset of a field in a full box (`version`/`flags` first) whose layout widens in version 1.
pub(super) fn full_box_field(buf: &[u8], body: &Range<usize>, v0: usize, v1: usize) -> usize {
    body.start + 4 + if version(buf, body) == 1 { v1 } else { v0 }
}

/// Full-box version byte; 0 for an (invalid) empty body instead of panicking.
pub(super) fn version(buf: &[u8], body: &Range<usize>) -> u8 {
    buf.get(body.start).copied().unwrap_or(0)
}

pub(super) fn patch(buf: &mut [u8], path: &[&FourCc], field: impl Fn(&[u8], &Range<usize>) -> usize, v: u32) -> Result<(), MuxError> {
    let body = locate(buf, path).ok_or(MuxError::Invalid(Mp4Error::MissingBox))?;
    let at = field(buf, &body);
    write_u32(buf, at, v)
}

pub(super) fn boxed(kind: &FourCc, parts: &[&[u8]]) -> Vec<u8> {
    let len: usize = 8 + parts.iter().map(|p| p.len()).sum::<usize>();
    let mut out = Vec::with_capacity(len);
    out.extend_from_slice(&u32::try_from(len).unwrap_or(0).to_be_bytes());
    out.extend_from_slice(kind);
    parts.iter().for_each(|p| out.extend_from_slice(p));
    out
}
