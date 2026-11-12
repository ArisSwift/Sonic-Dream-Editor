//! Finding the save that lives in an installation.
//!
//! The game keeps it in `<install>/PCSaveData`. Discovery is by content, not by
//! name: the folder also holds logs and cloud caches, so a file counts when it
//! decrypts and carries the game save. The file itself holds all the slots.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::{SaveFile, BACKUP_SUFFIX, PRIMARY_FILE};

/// A save file found inside an installation.
#[derive(Debug, Clone)]
pub struct FoundSave {
    pub path: PathBuf,
    /// The file's name, without the game's `-backup` copy.
    pub identifier: String,
    /// When the file was last written.
    pub modified: Option<SystemTime>,
    /// How many slots it holds, and which one the game last used.
    pub slots: usize,
    pub active_slot: Option<usize>,
}

impl FoundSave {
    pub fn is_primary(&self) -> bool {
        self.identifier == PRIMARY_FILE
    }
}

/// `<install>/PCSaveData`, where the game keeps its saves.
pub fn save_folder(install: &Path) -> Option<PathBuf> {
    let folder = install.join("PCSaveData");
    folder.is_dir().then_some(folder)
}

/// The installation a save belongs to: the folder that holds `PCSaveData`.
pub fn install_of(save: &Path) -> Option<PathBuf> {
    save.parent()?.parent().map(Path::to_path_buf)
}

/// Every save file in `<install>/PCSaveData`, the authoritative one first.
pub fn find_saves(install: &Path) -> Vec<FoundSave> {
    let Some(folder) = save_folder(install) else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(&folder) else {
        return Vec::new();
    };
    let mut found: Vec<FoundSave> = entries
        .flatten()
        .filter_map(|entry| read_save(&entry.path()))
        .collect();
    found.sort_by(|a, b| b.is_primary().cmp(&a.is_primary()).then(b.modified.cmp(&a.modified)));
    found
}

fn read_save(path: &Path) -> Option<FoundSave> {
    if !path.is_file() {
        return None;
    }
    let identifier = path.file_name()?.to_string_lossy().into_owned();
    if identifier.ends_with(BACKUP_SUFFIX) || identifier.starts_with('.') {
        return None;
    }
    let file = SaveFile::load(path).ok()?;
    let slots = file.slots().len();
    // A game save has a challenges block; anything else that happens to decode
    // is not a slot.
    if slots == 0 || file.slot_game(0).ok()?.root().get("m_challenges").is_none() {
        return None;
    }
    Some(FoundSave {
        path: path.to_path_buf(),
        identifier,
        modified: fs::metadata(path).ok().and_then(|meta| meta.modified().ok()),
        slots,
        active_slot: file.active_slot(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::container::{property, Entry};
    use crate::SLOT_IDS;

    fn write_save(folder: &Path, name: &str, game: &str, slot_two: Option<&str>) {
        let mut entries = vec![Entry::new(property::SAVE_DATA_GAME, game)];
        if let Some(slot_two) = slot_two {
            entries.push(Entry::new(SLOT_IDS[1], slot_two));
        }
        let container = crate::Container::new(0, entries);
        SaveFile { container }
            .write_atomic(folder.join(name))
            .unwrap();
    }

    #[test]
    fn finds_the_save_and_ignores_the_rest() {
        let root = std::env::temp_dir().join(format!("sdt-discover-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let folder = root.join("PCSaveData");
        fs::create_dir_all(&folder).unwrap();

        let game = "{\"m_challenges\":{}}";
        write_save(&folder, PRIMARY_FILE, game, Some(game));
        write_save(&folder, "PrimaryHL_Default_Save_ID-backup", game, None);
        fs::write(folder.join("icloud_kvs.txt"), b"not a save").unwrap();
        write_save(&folder, "other", "{\"m_settings\":{}}", None);

        let found = find_saves(&root);
        assert_eq!(found.len(), 1, "one real save, no backup, no junk");
        assert_eq!(found[0].identifier, PRIMARY_FILE);
        assert_eq!(found[0].slots, 2, "both slots in the file are counted");
        assert_eq!(install_of(&found[0].path), Some(root.clone()));

        fs::remove_dir_all(&root).unwrap();
    }
}
