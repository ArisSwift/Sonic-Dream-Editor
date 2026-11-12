//! Reader/writer for Sonic Dream Team save files (Project Dream PC port).
//!
//! The game keeps its authoritative save in `PCSaveData/PrimaryHL_Default_Save_ID`,
//! encrypted with a fixed key/IV recovered from the game binary. This crate decodes the
//! container, exposes the game save JSON, and re-encodes it so the game accepts the result.
//!
//! ```no_run
//! use sdt_save::SaveFile;
//!
//! let mut file = SaveFile::load("PCSaveData/PrimaryHL_Default_Save_ID")?;
//! let mut save = file.game_save()?;
//! save.unlock_all_skins()?;
//! file.set_game_save(&save)?;
//! file.write_atomic("PCSaveData/PrimaryHL_Default_Save_ID")?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

pub mod container;
pub mod discover;
pub mod save;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub use container::{Container, ContainerError, Entry, SLOT_IDS};
pub use discover::{find_saves, install_of, save_folder, FoundSave};
pub use save::{thousands, GameSave, Progress, SaveError, XpAccount, XpTrack, XP_SCALE};

/// File name of the authoritative save inside `PCSaveData`.
pub const PRIMARY_FILE: &str = "PrimaryHL_Default_Save_ID";
/// Suffix the game itself uses for the previous version of the save.
pub const BACKUP_SUFFIX: &str = "-backup";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("{0}")]
    Container(#[from] ContainerError),
    #[error("{0}")]
    Save(#[from] SaveError),
    #[error("property {0} is missing from the save container")]
    MissingProperty(u32),
}

/// A save file: the encrypted container plus access to the game save inside it.
#[derive(Debug, Clone)]
pub struct SaveFile {
    pub container: Container,
}

impl SaveFile {
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        Ok(Self { container: Container::decrypt(bytes)? })
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, Error> {
        Self::parse(&fs::read(path)?)
    }

    /// The game save JSON of slot 1 (`SaveDataGame`).
    pub fn game_save(&self) -> Result<GameSave, Error> {
        self.slot_game(0)
    }

    /// Replace the game save JSON of slot 1.
    pub fn set_game_save(&mut self, save: &GameSave) -> Result<(), Error> {
        self.set_slot_game(0, save)
    }

    /// The slots this file holds, in slot order: the game allows three, and a
    /// file written before more than one existed holds only the first.
    pub fn slots(&self) -> Vec<usize> {
        (0..container::SLOT_IDS.len())
            .filter(|index| self.container.get(container::SLOT_IDS[*index]).is_some())
            .collect()
    }

    /// The game save JSON of one slot, indexed from 0.
    pub fn slot_game(&self, index: usize) -> Result<GameSave, Error> {
        let id = *container::SLOT_IDS
            .get(index)
            .ok_or(Error::MissingProperty(u32::MAX))?;
        let text = self
            .container
            .get(id)
            .ok_or(Error::MissingProperty(id))?;
        Ok(GameSave::parse(text)?)
    }

    /// Replace the game save JSON of one slot, indexed from 0.
    pub fn set_slot_game(&mut self, index: usize, save: &GameSave) -> Result<(), Error> {
        let id = *container::SLOT_IDS
            .get(index)
            .ok_or(Error::MissingProperty(u32::MAX))?;
        if self.container.get(id).is_none() {
            return Err(Error::MissingProperty(id));
        }
        self.container.set(id, save.to_json());
        Ok(())
    }

    /// Which slot the game last used, when the file says.
    pub fn active_slot(&self) -> Option<usize> {
        self.container
            .get(container::property::ACTIVE_SLOT)
            .and_then(|value| value.trim().parse::<usize>().ok())
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        Ok(self.container.encrypt()?)
    }

    /// Write the save, keeping the previous file as `<name>-backup` the way the game does.
    ///
    /// The new file is written to a temporary sibling first and renamed into place, so an
    /// interrupted write never leaves a half-written save behind.
    pub fn write_atomic(&self, path: impl AsRef<Path>) -> Result<(), Error> {
        let path = path.as_ref();
        let bytes = self.to_bytes()?;
        let dir = path.parent().unwrap_or_else(|| Path::new("."));
        let tmp = dir.join(format!(
            ".{}.tmp",
            path.file_name().unwrap_or_default().to_string_lossy()
        ));

        fs::write(&tmp, &bytes)?;
        if let Ok(handle) = fs::File::open(&tmp) {
            let _ = handle.sync_all();
        }
        if path.exists() {
            let backup = backup_path(path);
            fs::copy(path, &backup)?;
        }
        fs::rename(&tmp, path)?;
        Ok(())
    }
}

/// `PrimaryHL_Default_Save_ID` -> `PrimaryHL_Default_Save_ID-backup`, matching the game.
pub fn backup_path(primary: &Path) -> PathBuf {
    let mut name = primary.file_name().unwrap_or_default().to_os_string();
    name.push(BACKUP_SUFFIX);
    primary.with_file_name(name)
}

/// The save directory the game shim uses (`SDT_SAVE_DIR`, else `PCSaveData` beside the exe).
pub fn default_save_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("SDT_SAVE_DIR") {
        return Some(PathBuf::from(dir));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_backup_and_primary() {
        let dir = std::env::temp_dir().join(format!("sdt-save-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(PRIMARY_FILE);

        let mut container = Container::new(0, vec![Entry::new(container::property::SAVE_DATA_GAME, "{}")]);
        container.set(container::property::GAME_VERSION, "1.10.1");
        let file = SaveFile { container };

        file.write_atomic(&path).unwrap();
        assert!(path.exists());
        assert!(!backup_path(&path).exists(), "no backup on first write");

        let mut updated = file.clone();
        updated.container.set(container::property::GAME_VERSION, "1.10.2");
        updated.write_atomic(&path).unwrap();

        let backup = SaveFile::load(backup_path(&path)).unwrap();
        assert_eq!(backup.container.get(container::property::GAME_VERSION), Some("1.10.1"));
        let current = SaveFile::load(&path).unwrap();
        assert_eq!(current.container.get(container::property::GAME_VERSION), Some("1.10.2"));

        fs::remove_dir_all(&dir).unwrap();
    }
}
