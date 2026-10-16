//! Locating the user's copy of the game and pulling the movie out of it.

use anyhow::{Context, anyhow, bail};
use std::path::{Path, PathBuf};

pub const PROJECTOR: &str = "ZlatyKalich.exe";

/// Trailer the Flash projector appends after the embedded SWF.
const PROJECTOR_MAGIC: u32 = 0xFA12_3456;

pub struct Game {
    pub dir: PathBuf,
    pub swf: Vec<u8>,
}

impl Game {
    /// `path` is the projector itself or the folder holding it.
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let exe = if path.is_dir() {
            find_case_insensitive(path, PROJECTOR)
                .ok_or_else(|| anyhow!("{PROJECTOR} not found in {}", path.display()))?
        } else {
            path.to_owned()
        };
        let data = std::fs::read(&exe).with_context(|| format!("reading {}", exe.display()))?;
        let swf = extract_swf(&data).with_context(|| format!("{} is not the game", exe.display()))?;
        let dir = exe
            .parent()
            .map(Path::to_owned)
            .unwrap_or_else(|| PathBuf::from("."));
        Ok(Self {
            dir,
            swf: swf.to_vec(),
        })
    }

    pub fn file(&self, name: &str) -> PathBuf {
        find_in(&self.dir, name)
    }
}

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

/// Game files keep their DOS-era casing on case-sensitive filesystems.
pub fn find_in(dir: &Path, name: &str) -> PathBuf {
    find_case_insensitive(dir, name).unwrap_or_else(|| dir.join(name))
}

fn find_case_insensitive(dir: &Path, name: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.eq_ignore_ascii_case(name))
        })
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
