//! Load-time changes to the game movie. The user's files are never modified;
//! the SWF is rewritten in memory before Ruffle sees it.
//!
//! - Host API: an invisible clip on the root timeline that lets the app talk
//!   to the game (see [`host`]).
//! - Hotspot highlight: every button gets a yellow-tinted copy of its own hit
//!   area (the shape the authors traced around the object), shown while Space
//!   is held. Scene hotspots are invisible hit-only buttons that
//!   the 2001 game placed *under* the scenery (Flash buttons stay clickable
//!   when covered), so those are raised above it; they draw nothing else, so
//!   only the overlay becomes visible.
//! - Dialogue fix: the hover label is cleared when dialogue choices appear;
//!   the 2001 game left the last label drawn over the first choice.
//! - Static text translations: a few drawn texts never got an English version;
//!   they're swapped for a text field when `_root.english == 1`.

use anyhow::Context;
use std::collections::{BTreeMap, HashMap, HashSet};
use swf::avm1::types::{Action, GetUrl, GetUrl2, If, Push, SendVarsMethod, Value};
use swf::{
    BlendMode, ButtonRecord, ButtonState, CharacterId, ClipAction, ClipActions, ClipEventFlag,
    Color, ColorTransform, EditText, Fixed8, Matrix, PlaceObject, PlaceObjectAction, Point,
    Rectangle, Sprite, SwfStr, Tag, Twips,
};

/// Flash key codes the app sends to the host clip. The game reads no keys.
pub mod host {
    /// Calls the game's own `savegame()` if a game is running; the clip first
    /// sends `fscommand("zk", "autosave")` so the app can route the file.
    pub const KEY_AUTOSAVE: u8 = 124; // F13
    /// The clip reports `fscommand("zk", "english=<0|1>")` whenever the game
    /// language changes.
    pub const COMMAND: &str = "zk";
}

const SPACE: i32 = 32;
const PROP_VISIBLE: i32 = 7;
/// New characters are numbered from here; the game uses ids below 3000.
const FIRST_NEW_ID: CharacterId = 60000;

/// When button hit areas are highlighted.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Hotspots {
    /// While Space is held.
    #[default]
    HoldSpace,
    Always,
    Off,
}

pub struct StaticTranslation {
    pub id: CharacterId,
    pub english: String,
}

pub fn patch(
    swf_data: &[u8],
    translations: &[StaticTranslation],
    script_texts: &HashMap<Vec<u8>, String>,
    hotspots: Hotspots,
) -> anyhow::Result<Vec<u8>> {
    let buf = swf::decompress_swf(swf_data).context("decompressing movie")?;
    let parsed = swf::parse_swf(&buf).context("parsing movie")?;
    let version = parsed.header.version();
    let replace = |s: &SwfStr| script_texts.get(s.as_bytes()).map(|t| str_cp1250(t));
    let rewrite = |code: &[u8]| crate::bytecode::rewrite(code, version, &replace).map(leak);
    let mut tags = parsed.tags;
    let mut next_id = FIRST_NEW_ID;
    let mut alloc = || {
        next_id += 1;
        next_id
    };

    let mut defs: Vec<Tag> = Vec::new();

    // Hotspot highlight: per button, a copy of its hit area tinted yellow.
    let toggle = leak(set_visible_to(match hotspots {
        Hotspots::HoldSpace => key_is_down(SPACE),
        Hotspots::Always => assemble(&[push(vec![Value::Bool(true)])]),
        Hotspots::Off => assemble(&[push(vec![Value::Bool(false)])]),
    }));
    let stage = &parsed.header.swf_header().stage_size;
    let shape_bounds: HashMap<CharacterId, Rectangle<Twips>> = tags
        .iter()
        .filter_map(|t| match t {
            Tag::DefineShape(s) => Some((s.id, s.shape_bounds.clone())),
            Tag::DefineText(t) | Tag::DefineText2(t) => {
                Some((t.id, transform(&t.matrix, &t.bounds)))
            }
            _ => None,
        })
        .collect();
    let mut overlays = HashMap::new();
    for tag in &tags {
        let Tag::DefineButton2(button) = tag else {
            continue;
        };
        let hit: Vec<_> = button
            .records
            .iter()
            .filter(|r| r.states.contains(ButtonState::HIT_TEST))
            .collect();
        let area = hit
            .iter()
            .filter_map(|r| Some(transform(&r.matrix, shape_bounds.get(&r.id)?)))
            .fold(Rectangle::INVALID, |acc, r| acc.union(&r));
        // Full-screen "click anywhere" buttons would tint the whole scene.
        if hit.is_empty() || covers_most(&area, stage) {
            continue;
        }
        let overlay = alloc();
        let mut frame: Vec<Tag> = hit
            .iter()
            .zip(1..)
            .map(|(r, depth)| place_tinted(r.id, depth, r.matrix))
            .collect();
        frame.push(Tag::DoAction(toggle));
        frame.push(Tag::ShowFrame);
        frame.push(Tag::DoAction(toggle));
        frame.push(Tag::ShowFrame);
        defs.push(Tag::DefineSprite(Sprite {
            id: overlay,
            num_frames: 2,
            tags: frame,
        }));
        overlays.insert(button.id, overlay);
    }

    // Host API clip.
    let host = alloc();
    defs.push(Tag::DefineSprite(Sprite {
        id: host,
        num_frames: 1,
        tags: vec![Tag::ShowFrame],
    }));

    // Static text swaps, set in the game's dialogue font.
    let font = font_named(&tags, "Arial CE").context("no Arial CE font in movie")?;
    let mut swaps = HashMap::new();
    for t in translations {
        let Some(text) = find_text(&tags, t.id) else {
            anyhow::bail!("no static text #{}", t.id);
        };
        let field = alloc();
        defs.push(Tag::DefineEditText(Box::new(english_field(
            field, font, text, &t.english,
        ))));
        let swap = alloc();
        defs.push(Tag::DefineSprite(Sprite {
            id: swap,
            num_frames: 2,
            tags: vec![
                place(t.id, 1, None),
                Tag::DoAction(leak(goto_by_language())),
                Tag::ShowFrame,
                Tag::RemoveObject(swf::RemoveObject {
                    depth: 1,
                    character_id: None,
                }),
                place(field, 2, None),
                Tag::ShowFrame,
            ],
        }));
        swaps.insert(t.id, swap);
    }

    let hidden = hit_only(&tags);
    let root_top = raise(&mut tags, &hidden);
    walk(&mut tags, &mut |tag| {
        if let Tag::DefineSprite(sprite) = tag {
            raise(&mut sprite.tags, &hidden);
        }
    });

    walk(&mut tags, &mut |tag| match tag {
        Tag::DefineButton2(button) => {
            for action in &mut button.actions {
                if let Some(code) = rewrite(action.action_data) {
                    action.action_data = code;
                }
            }
            if let Some(&overlay) = overlays.get(&button.id) {
                add_overlay(button, overlay);
            }
        }
        Tag::DoAction(data) => {
            if let Some(code) = rewrite(data) {
                *data = code;
            }
            if opens_dialogue(data) {
                *data = leak(clear_infoline_then(data));
            }
        }
        Tag::DefineEditText(text) => {
            if let Some(new) = text.initial_text().and_then(replace) {
                **text = (**text).clone().with_initial_text(Some(new));
            }
        }
        Tag::PlaceObject(po) => {
            if let Some(clip) = &mut po.clip_actions {
                for record in &mut clip.records {
                    if let Some(code) = rewrite(record.action_data) {
                        record.action_data = code;
                    }
                }
            }
            if let PlaceObjectAction::Place(id) | PlaceObjectAction::Replace(id) = &mut po.action
                && let Some(swap) = swaps.get(id)
            {
                *id = *swap;
            }
        }
        _ => {}
    });

    let first_frame = tags
        .iter()
        .position(|t| matches!(t, Tag::ShowFrame))
        .context("movie has no frames")?;
    tags.insert(first_frame, host_clip(host, root_top + 1));
    tags.splice(0..0, defs);

    let mut out = Vec::new();
    swf::write_swf(parsed.header.swf_header(), &tags, &mut out).context("writing movie")?;
    Ok(out)
}

fn walk<'a>(tags: &mut [Tag<'a>], f: &mut impl FnMut(&mut Tag<'a>)) {
    for tag in tags {
        if let Tag::DefineSprite(sprite) = tag {
            walk(&mut sprite.tags, f);
        }
        f(tag);
    }
}

/// Buttons with nothing but a hit area, and sprites made only of those.
fn hit_only(tags: &[Tag]) -> HashSet<CharacterId> {
    let mut hidden: HashSet<CharacterId> = tags
        .iter()
        .filter_map(|t| match t {
            Tag::DefineButton2(b)
                if !b.records.iter().any(|r| {
                    r.states
                        .intersects(ButtonState::UP | ButtonState::OVER | ButtonState::DOWN)
                }) =>
            {
                Some(b.id)
            }
            _ => None,
        })
        .collect();
    loop {
        let before = hidden.len();
        for tag in tags {
            if let Tag::DefineSprite(sprite) = tag {
                let mut placed = sprite
                    .tags
                    .iter()
                    .filter_map(|t| match t {
                        Tag::PlaceObject(po) => match po.action {
                            PlaceObjectAction::Place(id) | PlaceObjectAction::Replace(id) => {
                                Some(id)
                            }
                            PlaceObjectAction::Modify => None,
                        },
                        _ => None,
                    })
                    .peekable();
                let only_hidden = placed.peek().is_some() && placed.all(|id| hidden.contains(&id));
                if only_hidden {
                    hidden.insert(sprite.id);
                }
            }
        }
        if hidden.len() == before {
            return hidden;
        }
    }
}

/// Moves depths that only ever hold hit-only content above everything else in
/// this timeline, keeping their relative order. Returns the timeline's new top
/// depth.
fn raise(tags: &mut [Tag], hidden: &HashSet<CharacterId>) -> u16 {
    let mut placed: BTreeMap<u16, bool> = BTreeMap::new();
    let mut top = 0;
    for tag in tags.iter() {
        let depth = match tag {
            Tag::PlaceObject(po) => {
                if let PlaceObjectAction::Place(id) | PlaceObjectAction::Replace(id) = po.action {
                    let all = placed.entry(po.depth).or_insert(true);
                    *all &= hidden.contains(&id);
                }
                po.depth
            }
            Tag::RemoveObject(ro) => ro.depth,
            _ => continue,
        };
        top = top.max(depth);
    }
    let remap: HashMap<u16, u16> = placed
        .into_iter()
        .filter(|&(_, all_hidden)| all_hidden)
        .enumerate()
        .map(|(i, (depth, _))| (depth, top + 1 + i as u16))
        .collect();
    for tag in tags.iter_mut() {
        let depth = match tag {
            Tag::PlaceObject(po) => &mut po.depth,
            Tag::RemoveObject(ro) => &mut ro.depth,
            _ => continue,
        };
        if let Some(&new) = remap.get(depth) {
            *depth = new;
        }
    }
    top + remap.len() as u16
}

fn find_text<'b>(tags: &'b [Tag], id: CharacterId) -> Option<&'b swf::Text> {
    tags.iter().find_map(|t| match t {
        Tag::DefineText(t) | Tag::DefineText2(t) if t.id == id => Some(&**t),
        _ => None,
    })
}

fn covers_most(area: &Rectangle<Twips>, stage: &Rectangle<Twips>) -> bool {
    let size = |r: &Rectangle<Twips>| f64::from(r.width().get()) * f64::from(r.height().get());
    area.is_valid() && size(area) > 0.5 * size(stage)
}

fn transform(m: &Matrix, r: &Rectangle<Twips>) -> Rectangle<Twips> {
    [
        Point::new(r.x_min, r.y_min),
        Point::new(r.x_max, r.y_min),
        Point::new(r.x_min, r.y_max),
        Point::new(r.x_max, r.y_max),
    ]
    .into_iter()
    .fold(Rectangle::INVALID, |acc, p| acc.encompass(m.clone() * p))
}

fn add_overlay(button: &mut swf::Button, overlay: CharacterId) {
    let depth = button.records.iter().map(|r| r.depth).max().unwrap_or(0) + 1;
    button.records.push(ButtonRecord {
        states: ButtonState::UP | ButtonState::OVER | ButtonState::DOWN,
        id: overlay,
        depth,
        matrix: Matrix::IDENTITY,
        color_transform: ColorTransform::IDENTITY,
        filters: vec![],
        blend_mode: BlendMode::Normal,
    });
}

/// Places a character flattened to translucent yellow.
fn place_tinted<'a>(id: CharacterId, depth: u16, matrix: Matrix) -> Tag<'a> {
    let Tag::PlaceObject(mut po) = place(id, depth, None) else {
        unreachable!()
    };
    po.matrix = Some(matrix);
    po.color_transform = Some(ColorTransform {
        r_multiply: Fixed8::ZERO,
        g_multiply: Fixed8::ZERO,
        b_multiply: Fixed8::ZERO,
        a_multiply: Fixed8::from_f32(0.45),
        r_add: 0xFF,
        g_add: 0xD0,
        b_add: 0x30,
        a_add: 0,
    });
    Tag::PlaceObject(po)
}

fn font_named(tags: &[Tag], name: &str) -> Option<CharacterId> {
    tags.iter().find_map(|t| match t {
        Tag::DefineFont2(f) if f.name.to_str_lossy(encoding_rs::WINDOWS_1250) == name => Some(f.id),
        Tag::DefineFontInfo(f) if f.name.to_str_lossy(encoding_rs::WINDOWS_1250) == name => {
            Some(f.id)
        }
        _ => None,
    })
}

fn english_field<'a>(
    id: CharacterId,
    font: CharacterId,
    text: &swf::Text,
    english: &str,
) -> EditText<'a> {
    let first = text.records.first();
    let height = first.and_then(|r| r.height).unwrap_or(Twips::new(280));
    let color = first.and_then(|r| r.color.clone()).unwrap_or(Color::BLACK);
    let mut bounds = transform(&text.matrix, &text.bounds);
    // English runs longer than the Czech glyph run it replaces.
    bounds.x_max += bounds.width();
    EditText::new()
        .with_id(id)
        .with_bounds(bounds)
        .with_font_id(font, height)
        .with_color(Some(color))
        .with_initial_text(Some(str_cp1250(english)))
        .with_is_read_only(true)
        .with_is_selectable(false)
        .with_is_multiline(true)
        .with_is_word_wrap(true)
}

fn place<'a>(id: CharacterId, depth: u16, clip_actions: Option<ClipActions<'a>>) -> Tag<'a> {
    Tag::PlaceObject(Box::new(PlaceObject {
        version: 2,
        action: PlaceObjectAction::Place(id),
        depth,
        matrix: Some(Matrix::IDENTITY),
        color_transform: None,
        ratio: None,
        name: None,
        clip_depth: None,
        class_name: None,
        filters: None,
        background_color: None,
        blend_mode: None,
        clip_actions,
        has_image: false,
        is_bitmap_cached: None,
        is_visible: None,
        amf_data: None,
    }))
}

fn host_clip<'a>(id: CharacterId, depth: u16) -> Tag<'a> {
    let records = vec![
        ClipAction {
            events: ClipEventFlag::KEY_DOWN,
            key_code: None,
            action_data: leak(autosave_on_key()),
        },
        ClipAction {
            events: ClipEventFlag::ENTER_FRAME,
            key_code: None,
            action_data: leak(report_language()),
        },
    ];
    place(
        id,
        depth,
        Some(ClipActions {
            all_event_flags: ClipEventFlag::KEY_DOWN | ClipEventFlag::ENTER_FRAME,
            records,
        }),
    )
}

fn opens_dialogue(data: &[u8]) -> bool {
    contains(data, b"mzvol1") && contains(data, b"_visible")
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

// --- AVM1 assembly -------------------------------------------------------

fn str_cp1250(s: &str) -> &'static SwfStr {
    let (bytes, _, _) = encoding_rs::WINDOWS_1250.encode(s);
    SwfStr::from_bytes(Box::leak(bytes.into_owned().into_boxed_slice()))
}

fn leak(bytes: Vec<u8>) -> &'static [u8] {
    Box::leak(bytes.into_boxed_slice())
}

fn s(text: &str) -> Value<'static> {
    Value::Str(str_cp1250(text))
}

fn push(values: Vec<Value<'static>>) -> Action<'static> {
    Action::Push(Push { values })
}

fn assemble(actions: &[Action]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut w = swf::avm1::write::Writer::new(&mut out, 5);
    for a in actions {
        w.write_action(a).expect("writing to a Vec");
    }
    out
}

/// Terminates an action list for DoAction / clip / button use.
fn finish(mut code: Vec<u8>) -> Vec<u8> {
    code.push(0);
    code
}

/// `if (cond) { body }` with `cond` already assembled.
fn if_then(cond: Vec<u8>, body: Vec<u8>) -> Vec<u8> {
    let mut out = cond;
    out.extend(assemble(&[
        Action::Not,
        Action::If(If {
            offset: body.len() as i16,
        }),
    ]));
    out.extend(body);
    out
}

fn get(path: &str) -> Vec<u8> {
    assemble(&[push(vec![s(path)]), Action::GetVariable])
}

fn call_method(object: &str, method: &str, args: Vec<Value<'static>>) -> Vec<u8> {
    let argc = args.len() as i32;
    let mut values = args;
    values.push(Value::Int(argc));
    assemble(&[
        push(values),
        push(vec![s(object)]),
        Action::GetVariable,
        push(vec![s(method)]),
        Action::CallMethod,
    ])
}

fn key_is_down(code: i32) -> Vec<u8> {
    call_method("Key", "isDown", vec![Value::Int(code)])
}

fn equals(mut a: Vec<u8>, b: Value<'static>) -> Vec<u8> {
    a.extend(assemble(&[push(vec![b]), Action::Equals2]));
    a
}

/// `setProperty("", _visible, <value>)` on the running clip.
fn set_visible_to(value: Vec<u8>) -> Vec<u8> {
    let mut out = assemble(&[push(vec![s(""), Value::Int(PROP_VISIBLE)])]);
    out.extend(value);
    out.extend(assemble(&[Action::SetProperty]));
    finish(out)
}

fn fscommand(args: &str) -> Vec<u8> {
    assemble(&[Action::GetUrl(GetUrl {
        url: str_cp1250(&format!("FSCommand:{}", host::COMMAND)),
        target: str_cp1250(args),
    })])
}

/// `if (Key.getCode() == F13 && _root.hrajede == 1) { fscommand; _root.savegame(); }`
fn autosave_on_key() -> Vec<u8> {
    let mut cond = equals(
        call_method("Key", "getCode", vec![]),
        Value::Int(i32::from(host::KEY_AUTOSAVE)),
    );
    cond.extend(equals(get("_root.hrajede"), Value::Int(1)));
    cond.extend(assemble(&[Action::And]));
    let mut body = fscommand("autosave");
    body.extend(call_method("_root", "savegame", vec![]));
    body.extend(assemble(&[Action::Pop]));
    finish(if_then(cond, body))
}

/// `if (_root.english != zkl) { zkl = _root.english; fscommand("zk", "english=" add zkl); }`
fn report_language() -> Vec<u8> {
    let mut cond = get("_root.english");
    cond.extend(get("zkl"));
    cond.extend(assemble(&[Action::Equals2, Action::Not]));
    let mut body = assemble(&[push(vec![s("zkl")])]);
    body.extend(get("_root.english"));
    body.extend(assemble(&[
        Action::SetVariable,
        push(vec![
            s(&format!("FSCommand:{}", host::COMMAND)),
            s("english="),
        ]),
    ]));
    body.extend(get("zkl"));
    body.extend(assemble(&[
        Action::StringAdd,
        Action::GetUrl2(GetUrl2::for_get_url(SendVarsMethod::None)),
    ]));
    finish(if_then(cond, body))
}

/// `if (_root.english == 1) gotoAndStop(2); else stop();`
fn goto_by_language() -> Vec<u8> {
    let english = equals(get("_root.english"), Value::Int(1));
    let mut out = if_then(
        english,
        assemble(&[Action::GotoFrame(swf::avm1::types::GotoFrame { frame: 1 })]),
    );
    out.extend(assemble(&[Action::Stop]));
    finish(out)
}

/// Prepends `_root.infoline = "";`. Inserting before the first action leaves
/// all relative branch offsets in the original code valid.
fn clear_infoline_then(code: &[u8]) -> Vec<u8> {
    let mut out = assemble(&[push(vec![s("_root.infoline"), s("")]), Action::SetVariable]);
    out.extend_from_slice(code);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use swf::avm1::read::Reader;

    fn decode(code: &[u8]) -> Vec<String> {
        let mut r = Reader::new(code, 5);
        let mut out = Vec::new();
        while let Ok(a) = r.read_action() {
            if matches!(a, Action::End) {
                break;
            }
            out.push(format!("{a:?}"));
        }
        out
    }

    #[test]
    fn autosave_handler_decodes() {
        let actions = decode(&autosave_on_key());
        assert!(actions.iter().any(|a| a.contains("getCode")));
        assert!(actions.iter().any(|a| a.contains("savegame")));
        assert!(actions.iter().any(|a| a.contains("FSCommand:zk")));
    }

    #[test]
    fn if_skips_exactly_the_body() {
        let body = assemble(&[Action::Stop, Action::Play]);
        let code = if_then(assemble(&[push(vec![Value::Bool(true)])]), body.clone());
        assert!(code.ends_with(&body));
        let tail = &code[..code.len() - body.len()];
        // If: opcode 0x9D, length 2, offset == body length.
        assert_eq!(&tail[tail.len() - 5..], &[0x9D, 2, 0, body.len() as u8, 0]);
    }
}

#[cfg(test)]
mod game_tests {
    //! Run against a real copy: `ZK_GAME_EXE=/path/to/ZlatyKalich.exe cargo test -p kalich-core -- --ignored`
    use super::*;

    fn game_swf() -> Vec<u8> {
        let path = std::env::var("ZK_GAME_EXE").expect("set ZK_GAME_EXE");
        crate::extract_swf(&std::fs::read(path).unwrap())
            .unwrap()
            .to_vec()
    }

    #[test]
    #[ignore]
    fn every_button_but_full_screen_gets_an_overlay() {
        let patched = patch(&game_swf(), &[], &HashMap::new(), Hotspots::HoldSpace).unwrap();
        let buf = swf::decompress_swf(&patched[..]).unwrap();
        let parsed = swf::parse_swf(&buf).unwrap();
        let (mut with, mut without) = (0, vec![]);
        for tag in &parsed.tags {
            if let Tag::DefineButton2(b) = tag {
                if b.records.iter().any(|r| r.id > FIRST_NEW_ID) {
                    with += 1;
                } else {
                    without.push(b.id);
                }
            }
        }
        // 2269 is the full-screen "click to skip" intro button.
        assert_eq!(without, [2269], "{with} patched");
    }

    #[test]
    #[ignore]
    fn builtin_text_fixes_land_in_bytecode() {
        let texts = crate::texts::Texts::load(None).unwrap();
        let patched = patch(
            &game_swf(),
            &texts.static_translations(),
            &texts.script_overrides(),
            Hotspots::HoldSpace,
        )
        .unwrap();
        let body = swf::decompress_swf(&patched[..]).unwrap().data;
        let has = |s: &str| contains(&body, &encoding_rs::WINDOWS_1250.encode(s).0);
        assert!(has("Vypadá to jako graffiti."));
        assert!(!has("Vypadá to jako graffitti."));
    }
}
