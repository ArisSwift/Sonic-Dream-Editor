//! Typed access to the game save JSON (`SaveDataGame`), on top of a generic JSON tree.
//!
//! Everything the editor does not understand is preserved untouched: the tree is only
//! mutated where an edit is explicitly requested, and `to_json` writes it back.

use serde_json::{json, Map, Value};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SaveError {
    #[error("save JSON is not valid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("expected an object at {0}")]
    NotAnObject(&'static str),
    #[error("expected an array at {0}")]
    NotAnArray(&'static str),
}

/// Aggregate counts shown in the UI.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Progress {
    pub missions_complete: usize,
    pub missions_total: usize,
    pub levels_complete: usize,
    pub levels_total: usize,
    pub dream_powers_owned: usize,
    pub dream_powers_total: usize,
    pub skins_unlocked: usize,
    pub skins_total: usize,
    pub ornaments_unlocked: usize,
    pub ornaments_total: usize,
    pub music_seen: usize,
    pub music_total: usize,
    pub blue_coins_spent: u64,
    pub adventure_xp: u64,
    pub challenges_completed: u64,
}

/// The game save, as stored in property `3294653793`.
#[derive(Debug, Clone, PartialEq)]
pub struct GameSave {
    root: Value,
}

impl GameSave {
    pub fn parse(text: &str) -> Result<Self, SaveError> {
        Ok(Self { root: serde_json::from_str(text)? })
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, SaveError> {
        Ok(Self { root: serde_json::from_slice(bytes)? })
    }

    pub fn to_json(&self) -> String {
        self.root.to_string()
    }

    /// The same JSON, indented for a human to read. `to_json` keeps the compact
    /// form the game writes, which is what goes back into the save.
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(&self.root).unwrap_or_else(|_| self.to_json())
    }

    pub fn root(&self) -> &Value {
        &self.root
    }

    pub fn root_mut(&mut self) -> &mut Value {
        &mut self.root
    }

    pub fn progress(&self) -> Progress {
        let mut p = Progress::default();

        if let Some(levels) = self.array("m_levels") {
            p.levels_total = levels.len();
            for level in levels {
                let missions = level.get("m_missions").and_then(Value::as_array);
                let mut all = true;
                for mission in missions.into_iter().flatten() {
                    p.missions_total += 1;
                    if flag(mission, "m_complete") {
                        p.missions_complete += 1;
                    } else {
                        all = false;
                    }
                }
                if all && missions.map_or(false, |m| !m.is_empty()) {
                    p.levels_complete += 1;
                }
            }
        }

        if let Some(powers) = self.pointer("/m_dreamPowerStore/m_dreamPowers").and_then(Value::as_array) {
            p.dream_powers_total = powers.len();
            p.dream_powers_owned = powers.iter().filter(|power| flag(power, "m_bought")).count();
        }

        if let Some(skins) = self.array("m_characterSkins") {
            p.skins_total = skins.len();
            p.skins_unlocked = skins.iter().filter(|skin| flag(skin, "m_unlocked")).count();
        }

        if let Some(ornaments) = self.array("m_ornaments") {
            p.ornaments_total = ornaments.len();
            p.ornaments_unlocked = ornaments.iter().filter(|o| flag(o, "m_unlocked")).count();
        }

        if let Some(tracks) = self.array("m_musicTracks") {
            p.music_total = tracks.len();
            p.music_seen = tracks.iter().filter(|track| flag(track, "m_seen")).count();
        }

        p.blue_coins_spent = self
            .pointer("/m_blueCoins/m_spent")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        p.adventure_xp = self
            .pointer("/m_adventureRewardTrack/m_spentXP")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        p.challenges_completed = self
            .pointer("/m_challenges/m_challengesCompleted")
            .and_then(Value::as_u64)
            .unwrap_or(0);

        p
    }

    /// Unlock every character skin. Returns how many were locked before the call.
    pub fn unlock_all_skins(&mut self) -> Result<usize, SaveError> {
        let items = self.array_mut("m_characterSkins")?;
        Ok(set_flag(items, "m_unlocked", true))
    }

    /// Lock every character skin except those in `keep`.
    pub fn lock_skins_except(&mut self, keep: &[&str]) -> Result<usize, SaveError> {
        let items = self.array_mut("m_characterSkins")?;
        let mut changed = 0;
        for item in items.iter_mut() {
            let guid = item.get("m_guid").and_then(Value::as_str).unwrap_or_default();
            if !keep.contains(&guid) && set_flag_one(item, "m_unlocked", false) {
                changed += 1;
            }
        }
        Ok(changed)
    }

    /// Mark every dream power as bought. The shop will consider them owned.
    pub fn unlock_all_dream_powers(&mut self) -> Result<usize, SaveError> {
        let items = self.array_mut("m_dreamPowerStore/m_dreamPowers")?;
        Ok(set_flag(items, "m_bought", true))
    }

    /// Unlock every ornament and mark every music track as seen.
    pub fn unlock_all_collectibles(&mut self) -> Result<usize, SaveError> {
        let ornaments = set_flag(self.array_mut("m_ornaments")?, "m_unlocked", true);
        let music = set_flag(self.array_mut("m_musicTracks")?, "m_seen", true);
        Ok(ornaments + music)
    }

    /// Complete (or reset) every mission in every level.
    pub fn set_all_missions_complete(&mut self, complete: bool) -> Result<usize, SaveError> {
        let levels = self.array_mut("m_levels")?;
        let mut changed = 0;
        for level in levels.iter_mut() {
            let Some(missions) = level.get_mut("m_missions").and_then(Value::as_array_mut) else {
                continue;
            };
            for mission in missions.iter_mut() {
                if set_flag_one(mission, "m_complete", complete) {
                    changed += 1;
                }
                if complete {
                    set_number(mission, "m_progress", json!(1));
                }
            }
        }
        Ok(changed)
    }

    /// Collect (or un-collect) every reward of the adventure track and of the
    /// challenge tracks.
    pub fn set_all_rewards_collected(&mut self, collected: bool) -> Result<usize, SaveError> {
        let mut changed = set_flag(
            self.array_mut("m_adventureRewardTrack/m_rewards")?,
            "m_collected",
            collected,
        );
        if let Some(tracks) = self
            .root
            .pointer_mut("/m_challenges/m_rewardTracks")
            .and_then(Value::as_array_mut)
        {
            for track in tracks {
                if let Some(rewards) = track.get_mut("m_rewards").and_then(Value::as_array_mut) {
                    changed += set_flag(rewards, "m_collected", collected);
                }
            }
        }
        Ok(changed)
    }

    pub fn set_blue_coins_spent(&mut self, value: u64) {
        self.set_number("/m_blueCoins/m_spent", json!(value));
    }

    pub fn set_adventure_xp(&mut self, value: u64) {
        self.set_number("/m_adventureRewardTrack/m_spentXP", json!(value));
    }

    /// Write any JSON value at an RFC 6901 pointer, creating nothing that does not exist.
    pub fn set_value(&mut self, pointer: &str, value: Value) -> Result<(), SaveError> {
        match self.root.pointer_mut(pointer) {
            Some(slot) => {
                *slot = value;
                Ok(())
            }
            None => Err(SaveError::NotAnObject("pointer")),
        }
    }

    pub fn set_number(&mut self, pointer: &str, value: Value) {
        if let Some(slot) = self.root.pointer_mut(pointer) {
            *slot = value;
        }
    }

    fn pointer(&self, pointer: &str) -> Option<&Value> {
        self.root.pointer(&normalize(pointer))
    }

    fn array(&self, pointer: &str) -> Option<&Vec<Value>> {
        self.pointer(pointer).and_then(Value::as_array)
    }

    fn array_mut(&mut self, pointer: &str) -> Result<&mut Vec<Value>, SaveError> {
        self.root
            .pointer_mut(&normalize(pointer))
            .and_then(Value::as_array_mut)
            .ok_or(SaveError::NotAnArray("pointer"))
    }
}

/// Accept both `m_levels` and `/m_levels` as a JSON pointer.
fn normalize(pointer: &str) -> String {
    if pointer.starts_with('/') {
        pointer.to_string()
    } else {
        format!("/{pointer}")
    }
}

fn flag(object: &Value, key: &str) -> bool {
    object.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn set_flag(items: &mut [Value], key: &str, value: bool) -> usize {
    let mut changed = 0;
    for item in items.iter_mut() {
        if set_flag_one(item, key, value) {
            changed += 1;
        }
    }
    changed
}

fn set_flag_one(item: &mut Value, key: &str, value: bool) -> bool {
    let Some(object) = item.as_object_mut() else { return false };
    let before = object.get(key).and_then(Value::as_bool).unwrap_or(false);
    object.insert(key.to_string(), Value::Bool(value));
    before != value
}

fn set_number(item: &mut Value, key: &str, value: Value) {
    if let Some(object) = item.as_object_mut() {
        object.insert(key.to_string(), value);
    }
}

/// Build an empty save-shaped object; useful for tests and for a "new save" template.
pub fn empty_save_json() -> Map<String, Value> {
    Map::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "m_levels": [
            {"m_guid": "a", "m_missions": [
                {"m_guid": "m1", "m_complete": true, "m_progress": 1},
                {"m_guid": "m2", "m_complete": false, "m_progress": 0}
            ]},
            {"m_guid": "b", "m_missions": [
                {"m_guid": "m3", "m_complete": true, "m_progress": 1}
            ]}
        ],
        "m_dreamPowerStore": {"m_dreamPowers": [
            {"m_guid": "p1", "m_bought": true},
            {"m_guid": "p2", "m_bought": false}
        ]},
        "m_characterSkins": [
            {"m_guid": "s1", "m_unlocked": true, "m_unlockSeen": true},
            {"m_guid": "s2", "m_unlocked": false, "m_unlockSeen": true}
        ],
        "m_ornaments": [{"m_guid": "o1", "m_unlocked": false}],
        "m_musicTracks": [{"m_guid": "t1", "m_seen": false}],
        "m_blueCoins": {"m_guid": "", "m_spent": 230},
        "m_adventureRewardTrack": {"m_spentXP": 650, "m_rewards": [
            {"m_trackGUID": "x", "m_rewardGUID": "y", "m_collected": false}
        ]},
        "m_challenges": {"m_challengesCompleted": 6}
    }"#;

    #[test]
    fn reads_progress() {
        let save = GameSave::parse(SAMPLE).unwrap();
        let p = save.progress();
        assert_eq!(p.missions_total, 3);
        assert_eq!(p.missions_complete, 2);
        assert_eq!(p.levels_total, 2);
        assert_eq!(p.levels_complete, 1);
        assert_eq!(p.dream_powers_total, 2);
        assert_eq!(p.dream_powers_owned, 1);
        assert_eq!(p.skins_total, 2);
        assert_eq!(p.skins_unlocked, 1);
        assert_eq!(p.blue_coins_spent, 230);
        assert_eq!(p.adventure_xp, 650);
        assert_eq!(p.challenges_completed, 6);
    }

    #[test]
    fn unlocking_is_counted_and_idempotent() {
        let mut save = GameSave::parse(SAMPLE).unwrap();
        assert_eq!(save.unlock_all_skins().unwrap(), 1);
        assert_eq!(save.unlock_all_skins().unwrap(), 0);
        assert_eq!(save.progress().skins_unlocked, 2);
        assert_eq!(save.unlock_all_dream_powers().unwrap(), 1);
        assert_eq!(save.progress().dream_powers_owned, 2);
    }

    #[test]
    fn completing_missions_is_reversible() {
        let mut save = GameSave::parse(SAMPLE).unwrap();
        assert_eq!(save.set_all_missions_complete(true).unwrap(), 1);
        assert_eq!(save.progress().missions_complete, 3);
        assert_eq!(save.progress().levels_complete, 2);
        assert_eq!(save.set_all_missions_complete(false).unwrap(), 3);
        assert_eq!(save.progress().missions_complete, 0);
    }

    #[test]
    fn collects_every_reward_of_both_tracks() {
        let mut save = GameSave::parse(SAMPLE).unwrap();
        save.root_mut().as_object_mut().unwrap().insert(
            "m_challenges".into(),
            json!({
                "m_challengesCompleted": 6,
                "m_rewardTracks": [
                    {"m_rewards": [{"m_rewardGUID": "a", "m_collected": false}]},
                    {"m_rewards": [{"m_rewardGUID": "b", "m_collected": false}]}
                ]
            }),
        );
        assert_eq!(save.set_all_rewards_collected(true).unwrap(), 3);
        assert_eq!(save.set_all_rewards_collected(true).unwrap(), 0);

        let root = save.root();
        assert_eq!(root["m_adventureRewardTrack"]["m_rewards"][0]["m_collected"], json!(true));
        assert_eq!(root["m_challenges"]["m_rewardTracks"][1]["m_rewards"][0]["m_collected"], json!(true));
    }

    #[test]
    fn unknown_fields_survive_edits() {
        let mut save = GameSave::parse(SAMPLE).unwrap();
        save.root_mut()
            .as_object_mut()
            .unwrap()
            .insert("m_somethingNew".into(), json!({"keep": [1, 2, 3]}));
        save.unlock_all_skins().unwrap();
        let reread = GameSave::parse(&save.to_json()).unwrap();
        assert_eq!(reread.root()["m_somethingNew"]["keep"][2], json!(3));
    }

    #[test]
    fn lock_skins_except_keeps_the_listed_ones() {
        let mut save = GameSave::parse(SAMPLE).unwrap();
        save.unlock_all_skins().unwrap();
        assert_eq!(save.lock_skins_except(&["s1"]).unwrap(), 1);
        let skins = save.root()["m_characterSkins"].as_array().unwrap();
        assert_eq!(skins[0]["m_unlocked"], json!(true));
        assert_eq!(skins[1]["m_unlocked"], json!(false));
    }
}
