//! Reading the compressed advisory snapshot.

use std::io::Read;
use std::path::Path;

use flate2::read::GzDecoder;

use crate::advisory::{read_json_lines, Advisory};
use crate::Result;

/// Reads gzip-compressed JSON Lines: one OSV record per line.
pub(crate) fn read_gzip_json_lines(reader: impl Read, origin: &Path) -> Result<Vec<Advisory>> {
    read_json_lines(GzDecoder::new(reader), origin)
}
