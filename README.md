# Zlatý Kalich — native

Native macOS / Windows / Linux runner for the Czech adventure game *Zlatý Kalich* (2002, enteron.cz).
The original was a Flash 5 projector plus a Delphi helper (`player.exe`) for MIDI music and saving.
This replaces both: the movie runs in [Ruffle](https://ruffle.rs), and the helper is reimplemented in Rust.

No game data is included. Point it at your own copy.

## Run

```sh
cargo run --release -- /path/to/ZlatyKalich.exe   # or the folder containing it
```

Without an argument it uses the last location, then the folder next to the binary, then asks.
The installer `32-ZKinstall.exe` is an old RAR SFX; extract it with `unar` (not Homebrew `7z`).

## Keys

| Key | |
|---|---|
| hold `Space` | highlight everything clickable |
| `Ctrl`/`Cmd` + `1`–`9` | choose save slot (title bar shows it) |
| `Ctrl`/`Cmd` + `0` | choose the autosave, written when you quit |
| `Ctrl`/`Cmd` + `↑` / `↓` | music volume |
| `F11` / `Alt+Enter` | fullscreen (remembered; overrides the in-game option) |

## Settings

`settings.toml` in the config dir (`~/Library/Application Support/zlaty-kalich` on macOS):

```toml
music_volume = 80
quality = "best"          # or "game": honour the in-game LOW/MEDIUM/HIGH
hotspots = "hold-space"   # or "always" / "off"
soundfont = "/path/to.sf2" # optional, replaces the bundled GeneralUser GS
slot = 1
```

## Editing the game's text

```sh
zlaty-kalich --export-texts texts.tsv [/path/to/game]
```

Fill in the `replacement` column, save as `texts.tsv` next to the saves
(data dir, e.g. `~/Library/Application Support/zlaty-kalich`). Format is
documented in `src/texts.rs`. Built-in fixes live in `assets/texts.tsv`.

## What `player.exe` did, and how it maps here

Decoded from the binary (Delphi, `Unit1.TForm1.FormCreate`):

| Invocation from the movie | Original | Here |
|---|---|---|
| `fscommand("exec", "player.exe")` | closes the running `enteron.cz player` window → music stops | `Music::stop` |
| `fscommand("exec", "player.exe\tx.mid")` | TMediaPlayer plays the file, Notify rewinds + replays (loop) | rustysynth + GeneralUser GS, looped |
| `fscommand("exec", "player.exe\t<flags>\t…")` | writes `kalich.sav` | `save::encode`, byte-identical (tested against the shipped file) |

Saves live in the platform data dir as `slot1.sav`–`slot9.sav` and `autosave.sav`;
the original install's `kalich.sav` becomes slot 1 on first run. The movie's
`loadVariables("kalich.sav")` is served from the selected slot.

## Fixes and changes to the 2001 game

Applied in memory at load; your files are never modified.

- **Czech text**: Flash 5 text is in the system code page (cp1250 on Czech Windows);
  Ruffle assumed cp1252 and turned ř/ě/ů into ø/ì/ù. Vendored `swf` crate takes a legacy code page.
- **Hover label over dialogue**: the game never cleared it when choices opened.
- **Typos**: "graffitti" → "graffiti".
- **Untranslated drawn text**: the ending line and the merchant certificate get English versions.
- **Host API**: an injected clip lets the app trigger the game's own `savegame()` (autosave on quit)
  and hear about language changes (`fscommand("zk", …)`).

## Bundled assets

- `assets/GeneralUser-GS.sf2` — GeneralUser GS 2.0.3 SoundFont, see its license file.
- `vendor/swf/` — Ruffle's `swf` crate (MIT/Apache) with a settable legacy code page and a text-override hook.
- `assets/fonts/` — Liberation Sans (Arial-metric) and DejaVu Sans (Verdana-like) stand in for the Windows device fonts; licenses alongside.
