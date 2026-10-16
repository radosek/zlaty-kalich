//! Browser entry points for the Zlatý Kalich player: the same patching and
//! save encoding as the desktop app, compiled to WebAssembly.

use kalich_core::patch::{self, Hotspots};
use kalich_core::texts::Texts;
use kalich_core::{extract_swf, save};
use wasm_bindgen::prelude::*;

/// Whether `file` is (or contains) the game movie.
#[wasm_bindgen(js_name = isGame)]
pub fn is_game(file: &[u8]) -> bool {
    extract_swf(file).is_ok()
}

/// Turns the user's `ZlatyKalich.exe` (or a bare SWF) into the patched movie
/// the page hands to Ruffle. `hotspots` is "hold-space", "always" or "off";
/// `user_texts` is an optional `texts.tsv` with the player's own edits.
#[wasm_bindgen]
pub fn prepare(file: &[u8], hotspots: &str, user_texts: Option<String>) -> Result<Vec<u8>, JsError> {
    let swf = extract_swf(file).map_err(|e| JsError::new(&format!("{e:#}")))?;
    let texts = Texts::load(user_texts.as_deref()).map_err(|e| JsError::new(&format!("{e:#}")))?;
    let hotspots = match hotspots {
        "always" => Hotspots::Always,
        "off" => Hotspots::Off,
        _ => Hotspots::HoldSpace,
    };
    patch::patch(
        swf,
        &texts.static_translations(),
        &texts.script_overrides(),
        hotspots,
    )
    .map_err(|e| JsError::new(&format!("{e:#}")))
}

/// What the original `player.exe` did with its arguments.
#[wasm_bindgen(getter_with_clone)]
pub struct PlayerCommand {
    /// "stop", "play" or "save".
    pub kind: String,
    /// The MIDI file for "play", the `kalich.sav` contents for "save".
    pub value: String,
}

/// Interprets the argument string of `fscommand("exec", "player.exe\t…")`.
#[wasm_bindgen(js_name = playerExe)]
pub fn player_exe(args: &str) -> PlayerCommand {
    let params = save::param_str(args);
    let params = &params[1.min(params.len())..];
    let (kind, value) = match params {
        [] => ("stop", String::new()),
        [track] => ("play", track.clone()),
        _ => ("save", String::from_utf8_lossy(&save::encode(params)).into_owned()),
    };
    PlayerCommand {
        kind: kind.to_owned(),
        value,
    }
}
