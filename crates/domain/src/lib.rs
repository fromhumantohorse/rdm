//! Pure domain model: no I/O, no async.

mod checksum;
mod download;
mod failure;
mod file_kind;
mod segment;

pub use checksum::{Checksum, HashAlgo};
pub use download::{Download, DownloadError, DownloadId, Source, Status};
pub use failure::Failure;
pub use file_kind::{Category, default_captured, is_capturable};
pub use segment::{Segment, plan_segments};

pub const DEFAULT_CONNECTIONS: u8 = 32;
pub const MAX_CONNECTIONS: u8 = 64;
