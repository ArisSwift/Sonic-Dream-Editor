//! Dream Save Editor: a Raven window over the `sdt-save` crate.
//!
//! ```sh
//! RAVEN_LIB_DIR=~/Github/Raven/build cargo run -p sdt-editor -- "/path/to/PrimaryHL_Default_Save_ID"
//! ```
//!
//! The window edits the game save JSON in memory. Nothing touches the disk until
//! `Save` (Ctrl+S), which writes the same file the game reads and keeps the
//! previous version as `<name>-backup`.

use raven::*;
use sdt_save::{GameSave, SaveFile, PRIMARY_FILE};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

const UI: &str = include_str!("ui.xml");

struct State {
    window: Rid,
    file: Option<SaveFile>,
    game: Option<GameSave>,
    dirty: bool,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("usage: sdt-editor [SAVE_FILE]");
        println!("       SDT_SAVE_DIR=/path/to/PCSaveData sdt-editor");
        println!("       sdt-editor --self-test SAVE_COPY   (headless check of the whole chain)");
        return;
    }
    if let Some(index) = args.iter().position(|arg| arg == "--self-test") {
        match args.get(index + 1) {
            Some(path) => self_test(path),
            None => {
                eprintln!("sdt-editor: --self-test needs the path of a save copy");
                std::process::exit(2);
            }
        }
        return;
    }

    let Some(state) = start(false) else { std::process::exit(1) };

    let initial = args
        .first()
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .map(|path| path.display().to_string())
        .unwrap_or_else(guess_save_path);
    set_input(&state.borrow(), "path", &initial);
    if !initial.is_empty() {
        load(&state);
    }

    unsafe {
        while Rwait_frame(-1.0) != 0 {}
        Rshutdown();
    }
}

fn start(headless: bool) -> Option<Rc<RefCell<State>>> {
    unsafe {
        let mut desc: RInitDesc = std::mem::zeroed();
        desc.backend = if headless { R_BACKEND_NULL } else { R_BACKEND_VULKAN };
        desc.headless = headless as i32;
        if Rinit_ex(&desc) == 0 {
            eprintln!("sdt-editor: cannot start Raven");
            return None;
        }
    }
    let window = window_from_xml(UI);
    if window == R_INVALID_ID {
        report_ui_error();
        unsafe { Rshutdown() };
        return None;
    }
    let state = Rc::new(RefCell::new(State { window, file: None, game: None, dirty: false }));
    bind_actions(&state);
    Some(state)
}

/// Headless run of the whole chain against a throwaway copy of a save.
fn self_test(path: &str) {
    let source = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("self-test: cannot read {path}: {error}");
            std::process::exit(2);
        }
    };
    let dir = std::env::temp_dir().join(format!("sdt-editor-self-test-{}", std::process::id()));
    let copy = dir.join(PRIMARY_FILE);
    let setup = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&copy, &source));
    if let Err(error) = setup {
        eprintln!("self-test: cannot prepare {}: {error}", copy.display());
        std::process::exit(2);
    }
    let path = copy.to_str().expect("a temporary path is valid UTF-8");

    let Some(state) = start(true) else { std::process::exit(1) };
    let mut failures: Vec<String> = Vec::new();
    let mut check = |ok: bool, what: &str| {
        if !ok {
            failures.push(what.to_string());
        }
    };

    set_input(&state.borrow(), "path", path);
    fire("load");
    check(label_text(&state.borrow(), "status").starts_with("Loaded"), "load the save");

    let before = skins_unlocked(&state.borrow());
    check(before.0 < before.1, "the copy starts with locked skins");
    fire("unlockSkins");
    let after = skins_unlocked(&state.borrow());
    check(after.0 == after.1, "every skin unlocked in memory");

    set_input(&state.borrow(), "xp", "4242");
    fire("setXp");
    check(
        section(&state.borrow(), "m_adventureRewardTrack")["m_spentXP"] == 4242,
        "adventure XP set",
    );

    fire("save");
    let status = label_text(&state.borrow(), "status");
    check(status.starts_with("Saved"), "save to disk");

    match SaveFile::load(path) {
        Ok(file) => {
            let progress = file.game_save().map(|game| game.progress()).unwrap_or_default();
            check(progress.skins_unlocked == progress.skins_total, "skins persisted");
            check(progress.adventure_xp == 4242, "XP persisted");
            check(
                sdt_save::backup_path(std::path::Path::new(path)).is_file(),
                "previous version kept as -backup",
            );
        }
        Err(error) => failures.push(format!("reload the written save: {error}")),
    }

    unsafe { Rshutdown() };
    let _ = std::fs::remove_dir_all(&dir);

    if failures.is_empty() {
        println!("self-test: ok");
    } else {
        for failure in &failures {
            eprintln!("self-test: FAILED {failure}");
        }
        eprintln!("self-test: {} failure(s), status: {}", failures.len(), status);
        std::process::exit(1);
    }
}

fn fire(action: &str) {
    unsafe { Raction_fire(cstr(action).as_ptr(), R_INVALID_ID) }
}

fn label_text(state: &State, name: &str) -> String {
    unsafe { text(Rlabel_get_text(widget_find(state.window, name))) }
}

/// `(unlocked, total)` from the JSON in the editor, whatever the widgets show.
fn skins_unlocked(state: &State) -> (usize, usize) {
    let skins = section(state, "m_characterSkins");
    let skins = skins.as_array().cloned().unwrap_or_default();
    let unlocked = skins
        .iter()
        .filter(|skin| skin.get("m_unlocked").and_then(|flag| flag.as_bool()).unwrap_or(false))
        .count();
    (unlocked, skins.len())
}

fn section(state: &State, key: &str) -> serde_json::Value {
    let text = json_text(state);
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|root| root.get(key).cloned())
        .unwrap_or(serde_json::Value::Null)
}

fn report_ui_error() {
    eprintln!("sdt-editor: the interface did not load");
    unsafe {
        let mut error: RXmlError = std::mem::zeroed();
        if Rxml_document_parse(UI.as_ptr() as *const _, UI.len(), &mut error) == R_INVALID_ID {
            eprintln!("  line {}: {}", error.line, text(error.message.as_ptr()));
        }
    }
}

fn bind_actions(state: &Rc<RefCell<State>>) {
    macro_rules! on {
        ($name:literal, $handler:expr) => {{
            let state = Rc::clone(state);
            action_bind($name, move |_, _| $handler(&state));
        }};
    }
    on!("load", load);
    on!("save", save);
    on!("applyJson", apply_json);
    on!("refreshJson", refresh);
    on!("setXp", set_xp);
    on!("setCoins", set_coins);
    on!("unlockSkins", unlock_skins);
    on!("unlockPowers", unlock_powers);
    on!("unlockCollectibles", unlock_collectibles);
    on!("completeMissions", complete_missions);
    on!("resetMissions", reset_missions);
    on!("collectRewards", collect_rewards);
}

// ---------------------------------------------------------------- widget access

fn set_label(state: &State, name: &str, value: &str) {
    unsafe { Rlabel_set_text(widget_find(state.window, name), cstr(value).as_ptr()) }
}

fn set_input(state: &State, name: &str, value: &str) {
    unsafe { Rtext_input_set_text(widget_find(state.window, name), cstr(value).as_ptr()) }
}

fn input_text(state: &State, name: &str) -> String {
    unsafe { text(Rtext_input_get_text(widget_find(state.window, name))) }
}

fn json_text(state: &State) -> String {
    unsafe { text(Rtext_editor_get_text(widget_find(state.window, "json"))) }
}

fn set_json(state: &State, value: &str) {
    unsafe { Rtext_editor_set_text(widget_find(state.window, "json"), cstr(value).as_ptr()) }
}

fn status(state: &State, message: &str) {
    set_label(state, "status", message);
}

fn status_of(state: &Rc<RefCell<State>>, message: &str) {
    status(&state.borrow(), message);
}

// ---------------------------------------------------------------- state plumbing

/// Take the JSON in the editor as the truth, so an action never discards typing.
fn sync_json(state: &Rc<RefCell<State>>) -> Result<(), String> {
    let (text, loaded) = {
        let state = state.borrow();
        (json_text(&state), state.file.is_some())
    };
    if !loaded {
        return Err("No save loaded.".to_string());
    }
    let game = GameSave::parse(&text).map_err(|error| format!("The JSON in the editor is not valid: {error}"))?;
    state.borrow_mut().game = Some(game);
    Ok(())
}

/// Redraw every view of the in-memory save.
fn refresh(state: &Rc<RefCell<State>>) {
    let snapshot = {
        let state = state.borrow();
        state
            .game
            .as_ref()
            .map(|game| (game.progress(), game.to_json_pretty(), state.dirty))
    };
    let Some((progress, json, dirty)) = snapshot else { return };

    let state = state.borrow();
    set_label(&state, "statMissions", &count(progress.missions_complete, progress.missions_total));
    set_label(&state, "statLevels", &count(progress.levels_complete, progress.levels_total));
    set_label(&state, "statPowers", &count(progress.dream_powers_owned, progress.dream_powers_total));
    set_label(&state, "statSkins", &count(progress.skins_unlocked, progress.skins_total));
    set_label(&state, "statOrnaments", &count(progress.ornaments_unlocked, progress.ornaments_total));
    set_label(&state, "statMusic", &count(progress.music_seen, progress.music_total));
    set_label(&state, "statChallenges", &progress.challenges_completed.to_string());
    set_input(&state, "xp", &progress.adventure_xp.to_string());
    set_input(&state, "coins", &progress.blue_coins_spent.to_string());
    set_json(&state, &json);
    set_label(&state, "dirty", if dirty { "● unsaved changes" } else { "" });
}

fn count(done: usize, total: usize) -> String {
    format!("{done} / {total}")
}

/// Run an edit against the in-memory save, then redraw.
fn edit(state: &Rc<RefCell<State>>, change: impl FnOnce(&mut GameSave) -> Result<String, String>) {
    if let Err(message) = sync_json(state) {
        status_of(state, &message);
        return;
    }
    let outcome = {
        let mut state = state.borrow_mut();
        if state.game.is_none() {
            Err("No save loaded.".to_string())
        } else {
            let result = change(state.game.as_mut().unwrap());
            if result.is_ok() {
                state.dirty = true;
            }
            result
        }
    };
    match outcome {
        Ok(message) => {
            refresh(state);
            status_of(state, &message);
        }
        Err(message) => status_of(state, &message),
    }
}

// ---------------------------------------------------------------- actions

fn load(state: &Rc<RefCell<State>>) {
    let path = input_text(&state.borrow(), "path").trim().to_string();
    if path.is_empty() {
        return status_of(state, "Choose the save file to load first.");
    }

    let file = match SaveFile::load(&path) {
        Ok(file) => file,
        Err(error) => return status_of(state, &format!("Could not load {path}: {error}")),
    };
    let properties = file.container.entries.len();
    let game = match file.game_save() {
        Ok(game) => game,
        Err(error) => return status_of(state, &format!("{path} is a save, but its game data is unreadable: {error}")),
    };

    {
        let mut state = state.borrow_mut();
        state.file = Some(file);
        state.game = Some(game);
        state.dirty = false;
    }
    refresh(state);
    status_of(state, &format!("Loaded {properties} properties from {path}."));
}

fn save(state: &Rc<RefCell<State>>) {
    if let Err(message) = sync_json(state) {
        return status_of(state, &message);
    }
    let path = input_text(&state.borrow(), "path").trim().to_string();

    let written = {
        let mut state = state.borrow_mut();
        let state = &mut *state;
        let loaded = state.file.is_some() && state.game.is_some();
        if !loaded {
            Err("No save loaded.".to_string())
        } else {
            let file = state.file.as_mut().unwrap();
            let game = state.game.as_ref().unwrap();
            match file.set_game_save(game) {
                Err(error) => Err(format!("Could not update the save: {error}")),
                Ok(()) => file
                    .write_atomic(&path)
                    .map(|()| format!("Saved {path}. The previous version is kept as -backup."))
                    .map_err(|error| format!("Could not write {path}: {error}")),
            }
        }
    };

    match written {
        Ok(message) => {
            state.borrow_mut().dirty = false;
            status_of(state, &message);
        }
        Err(message) => status_of(state, &message),
    }
}

fn apply_json(state: &Rc<RefCell<State>>) {
    match sync_json(state) {
        Ok(()) => {
            state.borrow_mut().dirty = true;
            refresh(state);
            status_of(state, "JSON applied. Save to write it to disk.");
        }
        Err(message) => status_of(state, &message),
    }
}

fn set_xp(state: &Rc<RefCell<State>>) {
    set_number(state, "xp");
}

fn set_coins(state: &Rc<RefCell<State>>) {
    set_number(state, "coins");
}

fn set_number(state: &Rc<RefCell<State>>, field: &str) {
    if let Err(message) = sync_json(state) {
        return status_of(state, &message);
    }
    let raw = input_text(&state.borrow(), field);
    let Ok(value) = raw.trim().parse::<u64>() else {
        return status_of(state, &format!("{raw:?} is not a whole number."));
    };

    let applied = {
        let mut state = state.borrow_mut();
        if state.game.is_none() {
            false
        } else {
            let game = state.game.as_mut().unwrap();
            if field == "xp" {
                game.set_adventure_xp(value);
            } else {
                game.set_blue_coins_spent(value);
            }
            true
        }
    };
    if !applied {
        return status_of(state, "No save loaded.");
    }
    state.borrow_mut().dirty = true;

    refresh(state);
    let what = if field == "xp" { "Adventure XP" } else { "blue coins spent" };
    status_of(state, &format!("Set {what} to {value}."));
}

fn unlock_skins(state: &Rc<RefCell<State>>) {
    edit(state, |game| {
        game.unlock_all_skins()
            .map(|changed| format!("Unlocked {changed} skins."))
            .map_err(|error| error.to_string())
    });
}

fn unlock_powers(state: &Rc<RefCell<State>>) {
    edit(state, |game| {
        game.unlock_all_dream_powers()
            .map(|changed| format!("Bought {changed} dream powers."))
            .map_err(|error| error.to_string())
    });
}

fn unlock_collectibles(state: &Rc<RefCell<State>>) {
    edit(state, |game| {
        game.unlock_all_collectibles()
            .map(|changed| format!("Unlocked {changed} ornaments and music tracks."))
            .map_err(|error| error.to_string())
    });
}

fn complete_missions(state: &Rc<RefCell<State>>) {
    edit(state, |game| {
        game.set_all_missions_complete(true)
            .map(|changed| format!("Completed {changed} missions."))
            .map_err(|error| error.to_string())
    });
}

fn reset_missions(state: &Rc<RefCell<State>>) {
    edit(state, |game| {
        game.set_all_missions_complete(false)
            .map(|changed| format!("Reset {changed} missions."))
            .map_err(|error| error.to_string())
    });
}

fn collect_rewards(state: &Rc<RefCell<State>>) {
    edit(state, |game| {
        game.set_all_rewards_collected(true)
            .map(|changed| format!("Collected {changed} rewards."))
            .map_err(|error| error.to_string())
    });
}

// ---------------------------------------------------------------- discovery

/// Where the save most likely is: `SDT_SAVE_DIR`, then the usual install folders.
fn guess_save_path() -> String {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(dir) = std::env::var_os("SDT_SAVE_DIR") {
        candidates.push(PathBuf::from(dir).join(PRIMARY_FILE));
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("PCSaveData").join(PRIMARY_FILE));
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        candidates.push(home.join("Games/Project Dream PC/PCSaveData").join(PRIMARY_FILE));
    }
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .map(|path| path.display().to_string())
        .unwrap_or_default()
}
