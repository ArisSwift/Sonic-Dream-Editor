//! Inspect a real save file and prove the round trip is lossless.
//!
//! ```sh
//! cargo run -p sdt-save --example inspect -- "/path/to/PrimaryHL_Default_Save_ID"
//! ```

use sdt_save::{container::property, SaveFile};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("usage: inspect <save file>")?;
    let original = std::fs::read(&path)?;

    let file = SaveFile::load(&path)?;
    println!("file            : {path}");
    println!("bytes           : {}", original.len());
    println!("file type       : {}", file.container.file_type);
    println!("properties      : {}", file.container.entries.len());
    for entry in &file.container.entries {
        let preview: String = entry.value.chars().take(48).collect();
        println!("  {:<12} {:>6} bytes  {preview}", entry.id, entry.value.len());
    }

    let rebuilt = file.to_bytes()?;
    println!("rebuilt         : {} bytes", rebuilt.len());
    println!("byte identical  : {}", rebuilt == original);

    if let Some(version) = file.container.get(property::GAME_VERSION) {
        println!("game version    : {version}");
    }

    let save = file.game_save()?;
    let progress = save.progress();
    println!("\n--- progression ---");
    println!("missions        : {}/{}", progress.missions_complete, progress.missions_total);
    println!("levels complete : {}/{}", progress.levels_complete, progress.levels_total);
    println!("dream powers    : {}/{}", progress.dream_powers_owned, progress.dream_powers_total);
    println!("skins           : {}/{}", progress.skins_unlocked, progress.skins_total);
    println!("ornaments       : {}/{}", progress.ornaments_unlocked, progress.ornaments_total);
    println!("music tracks    : {}/{}", progress.music_seen, progress.music_total);
    println!("blue coins spent: {}", progress.blue_coins_spent);
    println!("xp earned       : {}", progress.xp.earned());
    println!("xp spent        : {} ({})", progress.xp.spent(), progress.xp.spent_breakdown());
    println!("xp available    : {}", progress.xp.available());
    println!("challenges      : {}", progress.challenges_completed);

    // The JSON must survive a parse/serialise cycle without changing meaning.
    let reserialised = sdt_save::GameSave::parse(&save.to_json())?;
    println!("\njson stable     : {}", reserialised.progress() == progress);
    Ok(())
}
