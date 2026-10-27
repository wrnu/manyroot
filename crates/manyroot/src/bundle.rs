//! Bundles: a portable stream of contributions for moving them between
//! replicas over any transport (a file, a pipe, an HTTP body).
//!
//! ```text
//! b"MRBUNDL1"  ground id (32 bytes)  { u32 big-endian length, contribution }*
//! ```

use std::io::{self, Read, Write};

use crate::contribution::{Contribution, DecodeError, MAX_CONTRIBUTION_BYTES};
use crate::id::GroundId;

const MAGIC: &[u8; 8] = b"MRBUNDL1";

#[derive(Debug, thiserror::Error)]
pub enum BundleError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("not a Manyroot bundle")]
    BadMagic,
    #[error("bundle entry of {0} bytes exceeds the size limit")]
    TooLarge(usize),
    #[error(transparent)]
    Decode(#[from] DecodeError),
}

pub fn write(
    mut w: impl Write,
    ground: GroundId,
    contributions: impl IntoIterator<Item = Contribution>,
) -> Result<usize, BundleError> {
    w.write_all(MAGIC)?;
    w.write_all(ground.as_bytes())?;
    let mut n = 0;
    for c in contributions {
        let bytes = c.encode();
        let len = u32::try_from(bytes.len()).map_err(|_| BundleError::TooLarge(bytes.len()))?;
        w.write_all(&len.to_be_bytes())?;
        w.write_all(&bytes)?;
        n += 1;
    }
    w.flush()?;
    Ok(n)
}

pub fn read(mut r: impl Read) -> Result<(GroundId, Vec<Contribution>), BundleError> {
    let mut magic = [0u8; 8];
    r.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(BundleError::BadMagic);
    }
    let mut ground = [0u8; 32];
    r.read_exact(&mut ground)?;

    let mut out = Vec::new();
    loop {
        let mut len = [0u8; 4];
        let mut filled = 0;
        while filled < len.len() {
            match r.read(&mut len[filled..])? {
                0 if filled == 0 => return Ok((GroundId(ground), out)),
                0 => return Err(io::Error::from(io::ErrorKind::UnexpectedEof).into()),
                n => filled += n,
            }
        }
        let len = u32::from_be_bytes(len) as usize;
        if len > MAX_CONTRIBUTION_BYTES {
            return Err(BundleError::TooLarge(len));
        }
        let mut buf = vec![0u8; len];
        r.read_exact(&mut buf)?;
        out.push(Contribution::decode(&buf)?);
    }
}
