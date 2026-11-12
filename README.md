<p align="center">
  <img src="docs/logo.png" width="420" alt="Sonic Dream Team">
</p>

<p align="center">A save editor for Sonic Dream Team, made for the Project Dream PC port.</p>

It opens the game's save file, shows you what's unlocked in each of the three
slots, and writes your changes back the same way the game does. It's a small
tool with one job.

I built it to learn Raven, the UI toolkit that draws the whole interface. The
editor was the excuse. If you're here for the save format, `crates/sdt-save`
works on its own and doesn't need a window at all.

<p align="center">
  <img src="docs/savegame.png" width="860" alt="The Savegame page, one card per slot">
</p>

## Download and run

Take the archive for your system from the
[releases page](https://github.com/ArisSwift/SonicDreamEditor/releases/latest),
unpack it, and run `sdt-editor` from the folder. There is nothing to install and
no compiler involved.

I use the Linux build every day. The Windows one builds and packages on CI, but I
have not run it myself, so treat it as untested.

## What comes in the archive

Each archive is the app on its own: the binary, the Raven library it links, the
assets, and both licences. Unpack it and run it, with no toolkit to install
first.

The Linux build is x86-64 and needs X11 libraries present, which any desktop has.
The Windows build needs the Microsoft C++ runtime, which most machines already
have from something else. If it complains about a missing DLL, that is the one.

## What you can edit

- **Savegame**: the saves found in your game folder, one card per slot.
- **Progression**: missions, levels, dream powers, skins, statues, music and
  challenges, each counted against what the game contains.
- **Currency**: the shared XP pool behind the three reward tracks. Add as much
  as you like, or take it back down.
- **Unlocks**: bulk edits for skins, dream powers, collectibles, missions and
  reward-track rewards.
- **JSON**: the save tree as it really is, for anything the editor doesn't
  cover yet.
- **About**: who made it and what it's built with.

Nothing is written until you press **Save**. Every unlock only changes the save
in memory, so you can change your mind as many times as you want.

<p align="center">
  <img src="docs/unlocks.png" width="620" alt="The Unlocks page, bulk edits for skins, powers, collectibles and missions">
</p>

## Where the save lives

<img align="right" width="180" src="docs/shadow-idol.png" alt="Shadow the Hedgehog">

The game keeps it at `PCSaveData/PrimaryHL_Default_Save_ID` inside the install
folder. All three slots live in that one file, as separate properties, so
choosing the install folder is enough. The editor finds the save on its own.

Saving writes the same file the game reads and keeps your previous version as
`PrimaryHL_Default_Save_ID-backup`, which is the name the game uses too. So
there's always a step back.

<br clear="right">

## How the save file works

<img align="right" width="110" src="docs/tails.png" alt="Tails">

Worth writing down, since it took a while.

The file is a `u32_le` length followed by AES-128-CBC ciphertext. The key is the
first 16 bytes of the base64 of `H4RDl1ght_S4VE`, the IV is `7%o18m'~)mQD%o1`,
and the plaintext is NUL-padded to the next 16-byte block. Both came out of the
game binary.

Inside, the plaintext is text, not binary:

```
<fileType>
<entryCount>
<id>
<value>
... one id/value pair per entry
```

Each slot is one entry holding the game's JSON. The three slot ids are
`3294653793`, `3009895927` and `711855181`, and `3764495936` says which slot the
game used last. Settings, language, game version and notifications sit beside
them as their own entries.

Those ids belong to the game, not to the file. Nothing in a save carries a slot
table, and the same ids show up in the local file and in the game's cloud store,
so any PC running the same build has the same three. If an update ever changes
them, the editor says so on the Savegame page and leaves the saves it cannot name
alone rather than guessing which is which.

XP is the one part that isn't a plain number. Each track stores a major and a
minor part (`m_spentXP` and `m_spentXPMinor`), scaled by 100, and the reward
screens show one shared balance: everything earned minus everything every track
has spent. Giving XP therefore raises `m_totalStoryMissionXpEarned` rather than
touching the individual tracks. Written this way, the game accepts it and the
tracks stay consistent.

## Building

You need a recent Rust toolchain and
[Raven](https://github.com/ArisSwift/Raven), the UI toolkit the window is made
of. Raven is built from source, and the editor links it by a path, so the two
need to sit beside each other:

```
somewhere/
  Raven/                 the toolkit
  sonic-dream-editor/    this repository
```

Clone [my fork of Raven](https://github.com/ArisSwift/Raven) into `Raven`, build
it once:

```sh
cmake -S Raven -B Raven/build -G Ninja -DCMAKE_BUILD_TYPE=Release \
  -DRAVEN_BUILD_SHARED=ON -DRAVEN_BUILD_TOOLS=OFF -DRAVEN_BUILD_EXAMPLES=OFF
cmake --build Raven/build --parallel
```

Then build the editor. Raven's build folder is the default, so with the two
folders side by side there is nothing to set:

```sh
cargo build -p sdt-editor
cargo build --release -p sdt-editor
```

That default lives in `.cargo/config.toml`. If Raven is somewhere else, say so:

```sh
RAVEN_LIB_DIR=/path/to/Raven/build cargo build -p sdt-editor
```

Run it with the library path, until you copy the library next to the binary:

```sh
LD_LIBRARY_PATH=Raven/build ./target/debug/sdt-editor "/path/to/install"
```

On Windows the library is `raven.dll` in `Raven/build/Release` instead, and it
has to sit next to `sdt-editor.exe` when you run it.

Pass the install folder, the save file, or nothing at all. With nothing, it
looks around for an install itself.

```sh
./target/debug/sdt-editor --help
./target/debug/sdt-editor --self-test /tmp/copy-of-the-save
```

`--self-test` runs the whole chain on a copy with no window: it finds the save,
loads a slot, unlocks skins, gives XP, saves, and then checks that every one of
those actually stuck in the file.

The save core has no dependency on Raven, so its tests run without a build:

```sh
cargo test -p sdt-save
```

To look inside a real save, and confirm the round trip is byte for byte:

```sh
cargo run -p sdt-save --example inspect -- "/path/to/PrimaryHL_Default_Save_ID"
```

## Repository layout

```
crates/sdt-save      the save format: container, JSON, slot metadata, discovery
crates/sdt-editor    the Raven window: ui.xml, theme.xml, icons, fonts
```

`crates/sdt-save/examples/inspect.rs` is the round-trip check. Everything the
editor shows comes from that crate, so the window is only drawing.

## Status

<img align="right" width="150" src="docs/amy.png" alt="Amy Rose">

Working on Linux (CachyOS): save, XP, unlocks, JSON and all six pages. The
Windows build compiles and packages on CI, but I have not run it, so treat that
one as untested.

<br clear="right">

## Licence

MIT. Use it, learn from it, put parts of it in your own tool. Keep the copyright
notice and you're square.

## Not affiliated with SEGA or Sonic Team

<img align="right" width="110" src="docs/sonic.png" alt="Sonic the Hedgehog">

Sonic Dream Team, its art, its icons and its logo belong to them. Every icon and
the logo in this editor are taken from the game. I'm just a person who wanted
their save file to make sense.

<br clear="right">

## My Socials

<img align="right" width="96" src="docs/shadow-symbol.png" alt="Shadow">

- [YouTube](https://www.youtube.com/@Swifty-i4n)
- [TikTok](https://www.tiktok.com/@swiftyaris)
- [Instagram](https://www.instagram.com/eltondavin/)
- [X](https://x.com/SwiftAvos)
- [itch.io](https://arisswift.itch.io/)
- [Discord](https://discord.com/invite/MdXqMPDxnd)
- [GitHub](https://github.com/ArisSwift)

<br clear="right">
