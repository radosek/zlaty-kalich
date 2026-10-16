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

Keys: `F11` / `Alt+Enter` toggle fullscreen, `Esc` leaves it.

## What `player.exe` did, and how it maps here

Decoded from the binary (Delphi, `Unit1.TForm1.FormCreate`):

| Invocation from the movie | Original | Here |
|---|---|---|
| `fscommand("exec", "player.exe")` | closes the running `enteron.cz player` window → music stops | `Music::stop` |
| `fscommand("exec", "player.exe\tx.mid")` | TMediaPlayer plays the file, Notify rewinds + replays (loop) | rustysynth + GeneralUser GS, looped |
| `fscommand("exec", "player.exe\t<flags>\t…")` | writes `kalich.sav` | `save::encode`, byte-identical (tested against the shipped file) |

`kalich.sav` lives in the platform data dir (`~/Library/Application Support/zlaty-kalich` etc.);
the one from the original install is copied there on first run. The movie reads it back with `loadVariables`.

## Bundled assets

- `assets/GeneralUser-GS.sf2` — GeneralUser GS 2.0.3 SoundFont, see its license file.
- `assets/fonts/` — Liberation Sans (Arial-metric) and DejaVu Sans (Verdana-like) stand in for the Windows device fonts; licenses alongside.
