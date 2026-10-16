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

/// RAR block signature; the 2002 installer is a RAR self-extractor.
const RAR_MAGIC: &[u8] = b"Rar!\x1a\x07";

impl Game {
    /// Like [`Game::open`], but also accepts the original installer
    /// (`32-ZKinstall.exe`), which is unpacked into `install_dir` first.
    pub fn open_or_install(path: &Path, install_dir: &Path) -> anyhow::Result<Self> {
        if path.is_file() && is_installer(path)? {
            unpack_installer(path, install_dir)?;
            return Self::open(install_dir);
        }
        Self::open(path)
    }

    /// `path` is the projector itself or the folder holding it.
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let exe = if path.is_dir() {
            find_case_insensitive(path, PROJECTOR)
                .ok_or_else(|| anyhow!("{PROJECTOR} not found in {}", path.display()))?
        } else {
            path.to_owned()
        };
        let data = std::fs::read(&exe).with_context(|| format!("reading {}", exe.display()))?;
        let swf =
            extract_swf(&data).with_context(|| format!("{} is not the game", exe.display()))?;
        let dir = exe
            .parent()
            .map(Path::to_owned)
            .unwrap_or_else(|| PathBuf::from("."));
        Ok(Self {
            dir,
            swf: swf.to_vec(),
        })
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

/// Files the 2002 installer contains; anything else in an archive is skipped.
const INSTALLER_EXTENSIONS: [&str; 3] = ["exe", "mid", "sav"];
const MAX_ENTRY_BYTES: u64 = 64 << 20;
const MAX_ENTRIES: usize = 64;

/// Extracts the installer's top-level game files only. Each destination is
/// built here from a bare file name, so entry names can't escape `dest`.
fn unpack_installer(installer: &Path, dest: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dest)?;
    let err = |e| anyhow!("unpacking {}: {e}", installer.display());
    let mut cursor = unrar_ng::Archive::new(installer)
        .open_for_processing()
        .map_err(err)?;
    for _ in 0..MAX_ENTRIES {
        let Some(entry) = cursor.read_header().map_err(err)? else {
            return Ok(());
        };
        let header = entry.entry();
        let target = safe_name(&header.filename)
            .filter(|_| header.is_file() && header.unpacked_size <= MAX_ENTRY_BYTES)
            .map(|name| dest.join(name));
        cursor = match target {
            Some(target) => entry.extract_to(target).map_err(err)?,
            None => {
                tracing::warn!("Skipping installer entry {:?}", header.filename);
                entry.skip().map_err(err)?
            }
        };
    }
    bail!(
        "{} has too many entries for the game installer",
        installer.display()
    )
}

/// A plain top-level file name with an expected extension, or nothing.
fn safe_name(name: &Path) -> Option<&std::ffi::OsStr> {
    let mut parts = name.components();
    let (Some(std::path::Component::Normal(file)), None) = (parts.next(), parts.next()) else {
        return None;
    };
    let ext = Path::new(file).extension()?.to_str()?.to_ascii_lowercase();
    INSTALLER_EXTENSIONS.contains(&ext.as_str()).then_some(file)
}

fn is_installer(path: &Path) -> anyhow::Result<bool> {
    let data = std::fs::read(path)?;
    Ok(extract_swf(&data).is_err() && data.windows(RAR_MAGIC.len()).any(|w| w == RAR_MAGIC))
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
    #[ignore]
    fn unpacks_original_installer() {
        // ZK_INSTALLER=/path/to/32-ZKinstall.exe cargo test -- --ignored
        let installer = std::env::var("ZK_INSTALLER").expect("set ZK_INSTALLER");
        let dir = tempfile::tempdir().unwrap();
        let game = Game::open_or_install(Path::new(&installer), dir.path()).unwrap();
        assert!(game.swf.starts_with(b"FWS"));
        assert!(find_case_insensitive(dir.path(), "menu.mid").is_some());
    }

    #[test]
    fn only_plain_game_files_are_unpacked() {
        assert!(safe_name(Path::new("menu.mid")).is_some());
        assert!(safe_name(Path::new("ZlatyKalich.EXE")).is_some());
        assert!(safe_name(Path::new("../evil.exe")).is_none());
        assert!(safe_name(Path::new("/etc/evil.sav")).is_none());
        assert!(safe_name(Path::new("sub/menu.mid")).is_none());
        assert!(safe_name(Path::new("run.sh")).is_none());
    }

    #[test]
    fn rejects_other_executables() {
        assert!(extract_swf(b"MZ not a projector at all").is_err());
    }
}
