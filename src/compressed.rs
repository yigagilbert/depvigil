//! Reading the compressed advisory snapshot.

use std::io::Read;
use std::path::Path;

use crate::advisory::Advisory;
use crate::{Error, Result};

/// Reads gzip-compressed JSON Lines.
pub(crate) fn read_gzip_json_lines(_reader: impl Read, origin: &Path) -> Result<Vec<Advisory>> {
    Err(Error::InvalidInput(format!(
        "{} is gzip-compressed, which this build cannot read yet",
        origin.display()
    )))
}
