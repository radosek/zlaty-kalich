//! The game ships as a Flash 5 projector: the player exe with the movie
//! appended and a trailer giving its length.

use anyhow::{anyhow, bail};

/// Trailer the Flash projector appends after the embedded SWF.
const PROJECTOR_MAGIC: u32 = 0xFA12_3456;

/// The SWF inside a projector, or the data itself if it already is one.
pub fn extract_swf(data: &[u8]) -> anyhow::Result<&[u8]> {
    if data.starts_with(b"FWS") || data.starts_with(b"CWS") {
        return Ok(data);
    }
    let Some(trailer) = data.len().checked_sub(8).map(|at| &data[at..]) else {
        bail!("file too small");
    };
    let magic = u32::from_le_bytes(trailer[..4].try_into()?);
    let len = u32::from_le_bytes(trailer[4..].try_into()?) as usize;
    if magic != PROJECTOR_MAGIC {
        bail!("no Flash projector trailer");
    }
    let start = data
        .len()
        .checked_sub(8 + len)
        .ok_or_else(|| anyhow!("projector trailer length out of range"))?;
    let swf = &data[start..data.len() - 8];
    if !swf.starts_with(b"FWS") && !swf.starts_with(b"CWS") {
        bail!("embedded movie has no SWF header");
    }
    Ok(swf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_appended_swf() {
        let swf = b"FWS\x05\x10\x00\x00\x00payload!";
        let mut exe = b"MZ fake projector stub".to_vec();
        exe.extend_from_slice(swf);
        exe.extend_from_slice(&PROJECTOR_MAGIC.to_le_bytes());
        exe.extend_from_slice(&(swf.len() as u32).to_le_bytes());
        assert_eq!(extract_swf(&exe).unwrap(), swf);
    }

    #[test]
    fn rejects_other_executables() {
        assert!(extract_swf(b"MZ not a projector at all").is_err());
    }
}
