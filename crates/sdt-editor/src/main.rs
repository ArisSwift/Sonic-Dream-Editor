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
use sdt_save::{
    find_saves, install_of, save_folder, thousands, FoundSave, GameSave, SaveFile, PRIMARY_FILE,
};
use std::cell::RefCell;
use std::sync::atomic::{AtomicU32, Ordering};
use std::path::{Path, PathBuf};
use std::rc::Rc;

const UI: &str = include_str!("ui.xml");

/// The game allows three saves.
const SLOTS: usize = 3;

struct State {
    window: Rid,
    /// The whole save file: the game keeps every slot inside it.
    file: Option<SaveFile>,
    /// The file the save was read from; Save writes it back here.
    path: Option<PathBuf>,
    /// The slot that is open in the editor.
    slot: Option<usize>,
    game: Option<GameSave>,
    /// The installation the saves were found in.
    install: Option<PathBuf>,
    /// The save file found there, for the slot list.
    found: Option<FoundSave>,
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

    // A folder or a save on the command line, else whatever is found nearby.
    if let Some(given) = args.first().map(PathBuf::from).or_else(guess_install) {
        let install = if given.is_dir() {
            Some(given)
        } else {
            install_of(&given)
        };
        if let Some(install) = install {
            open_install(&state, install);
        }
    }

    // `SDT_SLOT=1` opens that slot straight away, like `SDT_PAGE` picks a page.
    if let Ok(number) = std::env::var("SDT_SLOT") {
        if let Ok(number) = number.trim().parse::<usize>() {
            if (1..=SLOTS).contains(&number) {
                load_slot(&state, number - 1);
            }
        }
    }

    // `SDT_SHOT=<file>`: capture our own window once the page has settled, so a
    // check never needs the desktop. Raven hands back that window's pixels only.
    if let Ok(path) = std::env::var("SDT_SHOT") {
        let window = state.borrow().window;
        let data = Box::into_raw(Box::new((window, path)));
        unsafe { Rtimer_create(1.6, 0, Some(shot_timer), data as *mut std::ffi::c_void) };
    }

    unsafe {
        while Rwait_frame(-1.0) != 0 {}
        Rshutdown();
    }
}

unsafe extern "C" fn shot_timer(_timer: Rid, user_data: *mut std::ffi::c_void) {
    let (window, path) = &*(user_data as *const (Rid, String));
    let (mut width, mut height) = (0, 0);
    let pixels = Rwindow_capture(*window, &mut width, &mut height);
    if pixels.is_null() || width <= 0 || height <= 0 {
        eprintln!("sdt-editor: this backend cannot capture the window");
        return;
    }
    let len = (width as usize) * (height as usize) * 4;
    let mut out = Vec::with_capacity(len + 8);
    out.extend_from_slice(&(width as u32).to_le_bytes());
    out.extend_from_slice(&(height as u32).to_le_bytes());
    out.extend_from_slice(std::slice::from_raw_parts(pixels, len));
    Rpixels_free(pixels);
    match std::fs::write(path, &out) {
        Ok(()) => println!("shot: {path} ({width}x{height})"),
        Err(error) => eprintln!("shot failed: {error}"),
    }
    let _ = Box::from_raw(user_data as *mut (Rid, String));
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
    set_asset_base_path();
    apply_theme();
    apply_fonts();
    let window = window_from_xml(UI);
    if window == R_INVALID_ID {
        report_ui_error();
        unsafe { Rshutdown() };
        return None;
    }
    apply_title_font(window);
    apply_window_icon(window);
    apply_button_font(window);
    let state = Rc::new(RefCell::new(State {
        window,
        file: None,
        path: None,
        slot: None,
        game: None,
        install: None,
        found: None,
        dirty: false,
    }));
    bind_actions(&state);
    // `SDT_PAGE=progression` opens straight onto a page: handy for screenshots.
    let page = std::env::var("SDT_PAGE").unwrap_or_default();
    let page = PAGES
        .iter()
        .map(|(id, ..)| *id)
        .find(|id| *id == page)
        .unwrap_or("savegame");
    show_page(&state, page);
    Some(state)
}

/// The look lives in `assets/theme.xml`. Missing, the editor keeps Raven's own.
fn apply_theme() {
    for folder in asset_folders() {
        let theme = folder.join("theme.xml");
        if !theme.is_file() {
            continue;
        }
        unsafe {
            if Rtheme_load(cstr(&theme.to_string_lossy()).as_ptr()).is_null() {
                eprintln!("sdt-editor: {} did not load", theme.display());
                return;
            }
            Rtheme_use(cstr("Dream").as_ptr());
        }
        return;
    }
}

/// The logo font of the headings, kept for `apply_title_font`.
static TITLE_FONT: AtomicU32 = AtomicU32::new(R_INVALID_ID);

/// The body faces, first one that loads wins: a real Comic Sans if one is
/// dropped into `assets/fonts`, else Comic Neue, the open font of the same
/// shape. The logo font carries only A-Z, a-z and the space, so both fall back
/// to Raven's own font for digits and marks.
const BODY_FONTS: [&str; 3] = [
    "fonts/comic.ttf",
    "fonts/ComicSansMS.ttf",
    "fonts/ComicNeue-Regular.ttf",
];

fn apply_fonts() {
    let Some(folder) = asset_folders().into_iter().find(|folder| folder.is_dir()) else {
        return;
    };
    let title = load_font(&folder, "fonts/SEGA.TTF");
    let body = BODY_FONTS
        .iter()
        .map(|file| load_font(&folder, file))
        .find(|font| *font != R_INVALID_ID);
    if title != R_INVALID_ID {
        // The logo font has no digits: let it borrow the body face rather than
        // Raven's own, so a number inside a title is drawn in the same family
        // as the numbers everywhere else.
        unsafe {
            Rfont_set_fallback(
                title,
                body.unwrap_or_else(|| Rfont_get_default()),
            )
        };
        TITLE_FONT.store(title, Ordering::Relaxed);
    }
    if let Some(body) = body {
        unsafe { Rtheme_set_font(body) };
    }
}

fn load_font(folder: &Path, file: &str) -> Rid {
    let path = folder.join(file);
    if !path.is_file() {
        return R_INVALID_ID;
    }
    unsafe {
        let font = Rfont_create_from_file(cstr(&path.to_string_lossy()).as_ptr());
        if font == R_INVALID_ID {
            eprintln!("sdt-editor: {} did not load", path.display());
        } else {
            Rfont_set_fallback(font, Rfont_get_default());
        }
        font
    }
}

/// The Save button is the one piece of text that is not plain: the bold italic
/// of the body face, which Raven slants and thickens itself (Comic Neue ships
/// no italic file).
fn apply_button_font(window: Rid) {
    unsafe {
        let body = Rtheme_get_font();
        if body == R_INVALID_ID {
            return;
        }
        let variant = Rfont_get_variant(body, R_FONT_BOLD_ITALIC);
        if variant == R_INVALID_ID {
            return;
        }
        Rwidget_set_font(widget_find(window, "saveButton"), variant);
    }
}

/// The window icon: the dream orb, from `assets/icons/app.png`. Nothing on
/// Wayland carries a window icon, so there the desktop entry names it instead.
fn apply_window_icon(window: Rid) {
    let Some(folder) = asset_folders().into_iter().find(|folder| folder.is_dir()) else {
        return;
    };
    let path = folder.join("icons/app.png");
    let Ok(bytes) = std::fs::read(&path) else {
        return;
    };
    let Ok(image) = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png) else {
        eprintln!("sdt-editor: {} did not load", path.display());
        return;
    };
    let pixels = image.to_rgba8();
    let (width, height) = pixels.dimensions();
    unsafe {
        Rwindow_set_icon(window, pixels.as_raw().as_ptr(), width as i32, height as i32);
    }
}

/// The headings: the app's name, the page's name, and the three slot cards'.
fn apply_title_font(window: Rid) {
    let font = TITLE_FONT.load(Ordering::Relaxed);
    if font == R_INVALID_ID {
        return;
    }
    for name in [
        "appTitle",
        "pageTitle",
        "slot1Title",
        "slot2Title",
        "slot3Title",
    ] {
        unsafe { Rwidget_set_font(widget_find(window, name), font) };
    }
}

/// One page of the rail: the id its action and widgets are named after, its
/// heading, and the line under the heading. The rail itself carries the icon.
/// The About page's links, in the order the badges are shown: the action the
/// button fires, then where it goes. Only the platforms the badge pack covers:
/// itch.io is not in it, so its link is not offered.
const LINKS: [(&str, &str); 6] = [
    ("openYoutube", "https://www.youtube.com/@Swifty-i4n"),
    ("openTiktok", "https://www.tiktok.com/@swiftyaris"),
    ("openInstagram", "https://www.instagram.com/eltondavin/"),
    ("openX", "https://x.com/SwiftAvos"),
    ("openDiscord", "https://discord.com/invite/MdXqMPDxnd"),
    ("openGithub", "https://github.com/ArisSwift"),
];

const PAGES: [(&str, &str, &str); 6] = [
    (
        "savegame",
        "Savegame",
        "The saves the editor found in your game folder. Nothing is written until Save.",
    ),
    (
        "progression",
        "Progression",
        "What the save says is done, out of what the game contains.",
    ),
    (
        "currency",
        "Currency",
        "The shared XP pool behind the three reward tracks.",
    ),
    (
        "unlocks",
        "Unlocks",
        "Bulk edits. Each one changes the save in memory; Save writes it.",
    ),
    (
        "json",
        "JSON",
        "The game's save tree as it is. Anything the editor does not know is kept.",
    ),
    (
        "about",
        "About",
        "What this is, who made it, and what it's built with.",
    ),
];

/// Show one page: its content, its heading, and the rail entry in the accent.
/// `R_VISIBLE`/`R_COLLAPSED` keep the hidden pages out of the layout.
fn show_page(state: &Rc<RefCell<State>>, page: &str) {
    let state = state.borrow();
    for (id, title, subtitle) in PAGES {
        let shown = id == page;
        let visibility = if shown { R_VISIBLE } else { R_COLLAPSED };
        unsafe {
            let content = widget_find(state.window, &format!("page{}", capitalize(id)));
            Rwidget_set_visibility(content, visibility);
            let nav = widget_find(state.window, &format!("nav{}", capitalize(id)));
            Rwidget_set_class(
                nav,
                cstr(if shown { "nav navon" } else { "nav" }).as_ptr(),
            );
        }
        if shown {
            set_label(&state, "pageTitle", title);
            set_label(&state, "pageSubtitle", subtitle);
            // The pages share one scroll view, so a page opens at its top
            // rather than wherever the last one was left.
            unsafe { Rscroll_box_set_offset(widget_find(state.window, "pageScroll"), 0.0, 0.0) };
        }
    }
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
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
    let folder = dir.join("PCSaveData");
    let copy = folder.join(PRIMARY_FILE);
    let setup = std::fs::create_dir_all(&folder).and_then(|()| std::fs::write(&copy, &source));
    if let Err(error) = setup {
        eprintln!("self-test: cannot prepare {}: {error}", copy.display());
        std::process::exit(2);
    }
    let path = copy.to_str().expect("a temporary path is valid UTF-8");

    // Leave exactly one skin locked so the unlock action has something to do,
    // whatever state the save is in.
    if let Ok(mut file) = SaveFile::load(path) {
        if let Ok(mut game) = file.game_save() {
            if let Some(skins) = game
                .root_mut()
                .get_mut("m_characterSkins")
                .and_then(|skins| skins.as_array_mut())
            {
                for skin in skins.iter_mut() {
                    skin["m_unlocked"] = serde_json::json!(true);
                }
                if let Some(first) = skins.first_mut() {
                    first["m_unlocked"] = serde_json::json!(false);
                }
            }
            let _ = file.set_game_save(&game).and_then(|()| file.write_atomic(path));
        }
    }

    let Some(state) = start(true) else { std::process::exit(1) };
    let mut failures: Vec<String> = Vec::new();
    let mut check = |ok: bool, what: &str| {
        if !ok {
            failures.push(what.to_string());
        }
    };

    state.borrow_mut().install = Some(dir.clone());
    fire("refreshSaves");
    let window = state.borrow().window;
    check(
        widget_find(window, "folderPath") != R_INVALID_ID
            && widget_find(window, "slot1") != R_INVALID_ID,
        "the folder and the slot rows are in the interface",
    );
    let in_file = SaveFile::load(path).map(|file| file.slots().len()).unwrap_or(0);
    for number in 1..=SLOTS {
        let expected = if number <= in_file { R_VISIBLE } else { R_COLLAPSED };
        check(
            unsafe { Rwidget_get_visibility(widget_find(window, &format!("slot{number}"))) } == expected,
            "the slot rows match the slots in the file",
        );
    }
    check(in_file > 0, "the save file holds at least one slot");
    check(
        unsafe { Rwidget_get_visibility(widget_find(window, "slotsEmpty")) } == R_COLLAPSED,
        "the empty message goes away once a save is found",
    );
    fire("loadSlot1");
    check(
        label_text(&state.borrow(), "status").starts_with("Loaded"),
        "find and load a save from a folder",
    );
    if in_file > 1 {
        let first = json_text(&state.borrow());
        fire("loadSlot2");
        check(
            label_text(&state.borrow(), "status").starts_with("Loaded save 2"),
            "the second slot opens",
        );
        check(json_text(&state.borrow()) != first, "the slots hold different saves");
        fire("loadSlot1");
    }

    let before = skins_unlocked(&state.borrow());
    check(before.0 < before.1, "the copy starts with locked skins");
    fire("unlockSkins");
    let after = skins_unlocked(&state.borrow());
    check(after.0 == after.1, "every skin unlocked in memory");

    // The rail: one action per page, only the shown one in the layout.
    fire("navCurrency");
    let currency = widget_find(state.borrow().window, "pageCurrency");
    let savegame = widget_find(state.borrow().window, "pageSavegame");
    check(
        unsafe { Rwidget_get_visibility(currency) } == R_VISIBLE,
        "the rail shows the currency page",
    );
    check(
        unsafe { Rwidget_get_visibility(savegame) } == R_COLLAPSED,
        "and puts the savegame page away",
    );
    check(
        label_text(&state.borrow(), "pageTitle") == "Currency",
        "the page header follows the rail",
    );
    fire("navSavegame");

    let xp_before = section(&state.borrow(), "m_challenges")["m_totalStoryMissionXpEarned"]
        .as_i64()
        .unwrap_or(0);
    set_input(&state.borrow(), "xpDelta", "10,000");
    fire("addXp");
    let xp_after = section(&state.borrow(), "m_challenges")["m_totalStoryMissionXpEarned"]
        .as_i64()
        .unwrap_or(0);
    check(
        xp_after == xp_before + 10_000,
        "giving XP raises the earned total the game reads",
    );
    let available = {
        let state = state.borrow();
        state.game.as_ref().map(|game| game.xp().available()).unwrap_or(0)
    };
    check(
        label_text(&state.borrow(), "xpAvailable") == thousands(available),
        "the balance label matches the save",
    );

    fire("save");
    let status = label_text(&state.borrow(), "status");
    check(status.starts_with("Saved"), "save to disk");

    match SaveFile::load(path) {
        Ok(file) => {
            let progress = file.game_save().map(|game| game.progress()).unwrap_or_default();
            check(progress.skins_unlocked == progress.skins_total, "skins persisted");
            check(progress.xp.story_earned == xp_before + 10_000, "XP persisted");
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

/// Where `assets/` may be: the source folder while developing, then the folder
/// of the executable and the folders under it.
fn asset_folders() -> Vec<PathBuf> {
    let mut folders = vec![PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets")];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(folder) = exe.parent() {
            folders.push(folder.join("assets"));
            folders.push(folder.to_path_buf());
        }
    }
    folders
}

/// Icons and anything else the interface names with a relative path live in
/// `assets/`, beside the interface. The XML is embedded, so Raven has no
/// document folder to start from and has to be told.
fn set_asset_base_path() {
    if let Some(folder) = asset_folders().into_iter().find(|folder| folder.is_dir()) {
        unsafe { Rxml_set_base_path(cstr(&folder.to_string_lossy()).as_ptr()) };
    }
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
    on!("save", save);
    on!("chooseFolder", choose_folder);
    on!("refreshSaves", refresh_saves);
    on!("loadSlot1", |state: &Rc<RefCell<State>>| load_slot(state, 0));
    on!("loadSlot2", |state: &Rc<RefCell<State>>| load_slot(state, 1));
    on!("loadSlot3", |state: &Rc<RefCell<State>>| load_slot(state, 2));
    on!("applyJson", apply_json);
    on!("refreshJson", refresh);
    on!("addXp", add_xp);
    on!("unlockSkins", unlock_skins);
    on!("unlockPowers", unlock_powers);
    on!("unlockCollectibles", unlock_collectibles);
    on!("completeMissions", complete_missions);
    on!("resetMissions", reset_missions);
    on!("collectRewards", collect_rewards);
    // The About page's badges: one action each, named after the button that
    // fires it. itch.io has no badge in the pack, so its link is left out.
    for (action, url) in LINKS {
        let state = Rc::clone(state);
        action_bind(action, move |_, _| open_link(&state, url));
    }
    // One rail action per page, named after the button that fires it.
    for (id, ..) in PAGES {
        let state = Rc::clone(state);
        let name = format!("nav{}", capitalize(id));
        action_bind(&name, move |_, _| show_page(&state, id));
    }
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

/// Hands a link to the system: the browser, or the mail program for mailto:.
fn open_link(state: &Rc<RefCell<State>>, url: &str) {
    if unsafe { Ropen_url(cstr(url).as_ptr()) } == 0 {
        status_of(state, &format!("Could not open {} in your browser.", url));
    }
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
    set_label(&state, "xpAvailable", &thousands(progress.xp.available()));
    set_label(&state, "xpEarned", &thousands(progress.xp.earned()));
    set_label(&state, "xpSpent", &thousands(progress.xp.spent()));
    set_label(
        &state,
        "xpTracks",
        &format!("Spent per track: {}", progress.xp.spent_breakdown()),
    );
    set_json(&state, &json);
    set_label(&state, "dirty", if dirty { "● unsaved changes" } else { "" });
    let meta = match (&state.path, state.slot) {
        (Some(path), Some(slot)) => {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            format!("{name} · save {}", slot + 1)
        }
        _ => String::new(),
    };
    set_label(&state, "pageMeta", &meta);
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

/// Ask for the game folder with the system's folder picker, then look for saves.
fn choose_folder(state: &Rc<RefCell<State>>) {
    if !confirm_discard(state) {
        return status_of(state, "Kept the save as it is.");
    }
    let current = state.borrow().install.clone();
    let title = cstr("Where is Project Dream installed?");
    let start = current
        .as_ref()
        .map(|folder| cstr(&folder.to_string_lossy()));
    let desc = RFileDialogDesc {
        parent: state.borrow().window,
        title: title.as_ptr(),
        directory: start.as_ref().map_or(std::ptr::null(), |folder| folder.as_ptr()),
        file_name: std::ptr::null(),
        filters: std::ptr::null(),
        filter_count: 0,
        default_extension: std::ptr::null(),
        flags: 0,
    };
    let picked = unsafe { Rfile_dialog_select_folder(&desc) };
    if picked.is_null() {
        return status_of(state, "No folder chosen.");
    }
    open_install(state, PathBuf::from(unsafe { text(picked) }));
}

/// Take a folder as the installation: find its save file and list its slots.
fn open_install(state: &Rc<RefCell<State>>, install: PathBuf) {
    let found = find_saves(&install);
    let file = found.first().and_then(|save| SaveFile::load(&save.path).ok());
    let (slots, identifier) = match (&file, found.first()) {
        (Some(file), Some(save)) => (file.slots().len(), save.identifier.clone()),
        _ => (0, String::new()),
    };
    {
        let mut state = state.borrow_mut();
        state.install = Some(install.clone());
        state.found = found.first().cloned();
        state.file = file;
        state.path = found.first().map(|save| save.path.clone());
        state.slot = None;
        state.game = None;
        state.dirty = false;
    }
    set_label(&state.borrow(), "folderPath", &install.display().to_string());
    set_label(
        &state.borrow(),
        "saveFile",
        &if slots == 0 {
            "No save file yet.".to_string()
        } else {
            format!(
                "{identifier} · {slots} slot{}",
                if slots == 1 { "" } else { "s" }
            )
        },
    );
    refresh_slots(state);
    let message = match (save_folder(&install), slots) {
        (None, _) => format!(
            "{} has no PCSaveData folder. Pick the folder that has ProjectDream.exe in it.",
            install.display()
        ),
        (Some(_), 0) => format!(
            "No save in {}. Start the game once and press Refresh.",
            install.display()
        ),
        (Some(_), slots) => format!(
            "Found {slots} save slot{} in {}.",
            if slots == 1 { "" } else { "s" },
            install.display()
        ),
    };
    status_of(state, &message);
}

/// Read the installation again, so the slots match the disk.
fn refresh_saves(state: &Rc<RefCell<State>>) {
    let install = state.borrow().install.clone();
    let Some(install) = install else {
        return status_of(state, "Choose your game folder first.");
    };
    if !confirm_discard(state) {
        return status_of(state, "Kept the save as it is.");
    }
    open_install(state, install);
}

/// Opening something else drops the edits in memory: ask before doing it.
fn confirm_discard(state: &Rc<RefCell<State>>) -> bool {
    let window = state.borrow().window;
    if !state.borrow().dirty {
        return true;
    }
    let title = cstr("Unsaved changes");
    let text = cstr("The save has edits that are not written yet. Opening another save, or reading the folder again, drops them.");
    let answer = unsafe { Rmessage_box(window, title.as_ptr(), text.as_ptr(), R_MESSAGE_YES_NO) };
    answer == R_MESSAGE_RESULT_YES
}

/// Paint the slot rows from the save in memory: the game keeps all three in the
/// one file, so an empty slot is one the game has not written yet.
fn refresh_slots(state: &Rc<RefCell<State>>) {
    let state = state.borrow();
    let file = state.file.as_ref();
    let mut any = false;
    for number in 1..=SLOTS {
        let index = number - 1;
        let row = widget_find(state.window, &format!("slot{number}"));
        let game = file.filter(|file| file.slots().contains(&index)).map(|file| file.slot_game(index));
        let Some(Ok(game)) = game else {
            unsafe { Rwidget_set_visibility(row, R_COLLAPSED) };
            continue;
        };
        any = true;
        unsafe { Rwidget_set_visibility(row, R_VISIBLE) };
        // The card being edited wears the game's yellow frame. A theme style can
        // colour a border but not give it a width, so the frame is set here.
        let (width, color) = if state.slot == Some(index) {
            (2.0, (254.0 / 255.0, 208.0 / 255.0, 10.0 / 255.0))
        } else {
            (0.0, (0.0, 0.0, 0.0))
        };
        unsafe { Rborder_set_outline(row, width, color.0, color.1, color.2, 1.0) };
        // The title is drawn in the logo font, which has no digits: the slots
        // are numbered in roman, so the whole title stays one typeface.
        let slot_name = ["Slot I", "Slot II", "Slot III"][index];
        set_label(&state, &format!("slot{number}Title"), slot_name);
        let progress = game.progress();
        set_label(&state, &format!("slot{number}Orbs"), &progress.orbs.to_string());
        set_label(
            &state,
            &format!("slot{number}Statues"),
            &progress.ornaments_unlocked.to_string(),
        );
        set_label(
            &state,
            &format!("slot{number}Date"),
            &saved_stamp(progress.last_saved_ms),
        );
        set_label(
            &state,
            &format!("slot{number}Played"),
            &format!("Played: {}", progress.clock()),
        );
        set_label(
            &state,
            &format!("slot{number}Meta"),
            &format!(
                "{}/{} missions · {} XP",
                progress.missions_complete,
                progress.missions_total,
                thousands(progress.xp.available())
            ),
        );
    }
    let empty = if any { R_COLLAPSED } else { R_VISIBLE };
    unsafe { Rwidget_set_visibility(widget_find(state.window, "slotsEmpty"), empty) };
}

/// When the game last wrote the slot, from `m_lastSavedTimestampMs`, printed the
/// way the game's own card prints it: `11/11/2026 10:05:35 PM`, in local time.
fn saved_stamp(millis: i64) -> String {
    use chrono::{Local, TimeZone};
    if millis <= 0 {
        return "never".to_string();
    }
    match Local.timestamp_millis_opt(millis) {
        chrono::LocalResult::Single(when) => when.format("%-m/%-d/%Y %-I:%M:%S %p").to_string(),
        _ => "never".to_string(),
    }
}

/// Open one of the slots the save file holds, read fresh from disk: the game
/// rewrites the file on its own schedule, so the copy in memory can be behind.
fn load_slot(state: &Rc<RefCell<State>>, index: usize) {
    if !confirm_discard(state) {
        return status_of(state, "Kept the save as it is.");
    }
    let path = match state.borrow().path.clone() {
        Some(path) => path,
        None => return status_of(state, "Choose your game folder first."),
    };
    let file = match SaveFile::load(&path) {
        Ok(file) => file,
        Err(error) => return status_of(state, &format!("Could not read {}: {error}", path.display())),
    };
    let game = match file.slot_game(index) {
        Ok(game) => game,
        Err(error) => {
            return status_of(
                state,
                &format!("Save {} cannot be read: {error}", index + 1),
            )
        }
    };
    let progress = game.progress();
    {
        let mut state = state.borrow_mut();
        state.file = Some(file);
        state.game = Some(game);
        state.slot = Some(index);
        state.dirty = false;
    }
    refresh_slots(state);
    refresh(state);
    status_of(
        state,
        &format!(
            "Loaded save {}. {} of {} missions done, {} XP to spend.",
            index + 1,
            progress.missions_complete,
            progress.missions_total,
            thousands(progress.xp.available())
        ),
    );
}

fn save(state: &Rc<RefCell<State>>) {
    if let Err(message) = sync_json(state) {
        return status_of(state, &message);
    }

    let written = {
        let mut state = state.borrow_mut();
        let state = &mut *state;
        match (state.file.as_mut(), state.game.as_ref(), state.slot, state.path.clone()) {
            (Some(file), Some(game), Some(slot), Some(path)) => {
                match file.set_slot_game(slot, game) {
                    Err(error) => Err(format!("Could not update the save: {error}")),
                    Ok(()) => file
                        .write_atomic(&path)
                        .map(|()| path.clone())
                        .map_err(|error| format!("Could not write {}: {error}", path.display())),
                }
            }
            _ => Err("No save loaded.".to_string()),
        }
    };

    match written {
        Ok(path) => {
            state.borrow_mut().dirty = false;
            refresh_slots(state);
            status_of(
                state,
                &format!(
                    "Saved {}. The previous version is kept as -backup.",
                    path.file_name().unwrap_or_default().to_string_lossy()
                ),
            );
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

fn add_xp(state: &Rc<RefCell<State>>) {
    let raw = input_text(&state.borrow(), "xpDelta");
    let amount = match raw.trim().replace([',', '_'], "").parse::<i64>() {
        Ok(amount) if amount != 0 => amount,
        Ok(_) => return status_of(state, "Enter an XP amount to give (negative takes it away)."),
        Err(_) => return status_of(state, &format!("{raw:?} is not a whole number.")),
    };
    if let Err(message) = sync_json(state) {
        return status_of(state, &message);
    }

    let outcome = {
        let mut state = state.borrow_mut();
        let result = state.game.as_mut().map(|game| game.give_xp(amount));
        if result.is_some() {
            state.dirty = true;
        }
        result
    };

    let Some(now) = outcome else {
        return status_of(state, "No save loaded.");
    };
    set_input(&state.borrow(), "xpDelta", "");
    refresh(state);
    status_of(
        state,
        &format!(
            "Gave {} XP. The balance is now {}; save to keep it.",
            thousands(amount),
            thousands(now)
        ),
    );
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
/// The installation to open at start: a folder that has `PCSaveData` in it.
fn guess_install() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(dir) = std::env::var_os("SDT_SAVE_DIR") {
        let dir = PathBuf::from(dir);
        candidates.push(dir.parent().map(Path::to_path_buf).unwrap_or(dir));
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd);
    }
    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(PathBuf::from(home).join("Games/Project Dream PC"));
    }
    candidates.into_iter().find(|folder| save_folder(folder).is_some())
}
