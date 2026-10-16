//! Load-time changes to the game movie. The user's files are never modified;
//! the SWF is rewritten in memory before Ruffle sees it.
//!
//! - Host API: an invisible clip on the root timeline that lets the app talk
//!   to the game (see [`host`]).
//! - Hotspot highlight: every button gets an overlay over its hit area, shown
//!   while Space is held. Scene hotspots are invisible hit-only buttons that
//!   the 2001 game placed *under* the scenery (Flash buttons stay clickable
//!   when covered), so those are raised above it; they draw nothing else, so
//!   only the overlay becomes visible.
//! - Dialogue fix: the hover label is cleared when dialogue choices appear;
//!   the 2001 game left the last label drawn over the first choice.
//! - Static text translations: a few drawn texts never got an English version;
//!   they're swapped for a text field when `_root.english == 1`.

use crate::settings::Hotspots;
use anyhow::Context;
use std::collections::{BTreeMap, HashMap, HashSet};
use swf::avm1::types::{Action, GetUrl, GetUrl2, If, Push, SendVarsMethod, Value};
use swf::{
    BlendMode, ButtonRecord, ButtonState, CharacterId, ClipAction, ClipActions, ClipEventFlag,
    Color, ColorTransform, EditText, FillStyle, Fixed16, Matrix, PlaceObject, PlaceObjectAction,
    Point, Rectangle, Shape, ShapeFlag, ShapeRecord, ShapeStyles, Sprite, StyleChangeData, SwfStr,
    Tag, Twips,
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
const HIGHLIGHT_UNIT: i32 = 2000;

pub struct StaticTranslation {
    pub id: CharacterId,
    pub english: String,
}

pub fn patch(
    swf_data: &[u8],
    translations: &[StaticTranslation],
    hotspots: Hotspots,
) -> anyhow::Result<Vec<u8>> {
    let buf = swf::decompress_swf(swf_data).context("decompressing movie")?;
    let parsed = swf::parse_swf(&buf).context("parsing movie")?;
    let mut tags = parsed.tags;
    let mut next_id = FIRST_NEW_ID;
    let mut alloc = || {
        next_id += 1;
        next_id
    };

    let bounds = character_bounds(&tags);
    let mut defs: Vec<Tag> = Vec::new();

    // Hotspot highlight overlay, shared by every button.
    let rect = alloc();
    defs.push(Tag::DefineShape(Box::new(filled_rect(
        rect,
        Color {
            r: 0xFF,
            g: 0xE0,
            b: 0x40,
            a: 0x60,
        },
    ))));
    let overlay = alloc();
    let toggle = leak(set_visible_to(match hotspots {
        Hotspots::HoldSpace => key_is_down(SPACE),
        Hotspots::Always => assemble(&[push(vec![Value::Bool(true)])]),
        Hotspots::Off => assemble(&[push(vec![Value::Bool(false)])]),
    }));
    defs.push(Tag::DefineSprite(Sprite {
        id: overlay,
        num_frames: 2,
        tags: vec![
            place(rect, 1, None),
            Tag::DoAction(toggle),
            Tag::ShowFrame,
            Tag::DoAction(toggle),
            Tag::ShowFrame,
        ],
    }));

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
        defs.push(Tag::DefineEditText(Box::new(english_field(field, font, text, &t.english))));
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
        Tag::DefineButton2(button) => add_highlight(button, overlay, &bounds),
        Tag::DoAction(data) if opens_dialogue(data) => *data = leak(clear_infoline_then(data)),
        Tag::PlaceObject(po) => {
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

fn character_bounds(tags: &[Tag]) -> HashMap<CharacterId, Rectangle<Twips>> {
    let mut map = HashMap::new();
    for tag in tags {
        match tag {
            Tag::DefineShape(s) => {
                map.insert(s.id, s.shape_bounds.clone());
            }
            Tag::DefineText(t) | Tag::DefineText2(t) => {
                map.insert(t.id, transform(&t.matrix, &t.bounds));
            }
            Tag::DefineEditText(e) => {
                map.insert(e.id(), e.bounds().clone());
            }
            _ => {}
        }
    }
    map
}

fn find_text<'b>(tags: &'b [Tag], id: CharacterId) -> Option<&'b swf::Text> {
    tags.iter().find_map(|t| match t {
        Tag::DefineText(t) | Tag::DefineText2(t) if t.id == id => Some(&**t),
        _ => None,
    })
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

fn add_highlight(
    button: &mut swf::Button,
    overlay: CharacterId,
    bounds: &HashMap<CharacterId, Rectangle<Twips>>,
) {
    let hit = button
        .records
        .iter()
        .filter(|r| r.states.contains(ButtonState::HIT_TEST))
        .filter_map(|r| Some(transform(&r.matrix, bounds.get(&r.id)?)))
        .fold(Rectangle::INVALID, |acc, r| acc.union(&r));
    if !hit.is_valid() {
        return;
    }
    let depth = button.records.iter().map(|r| r.depth).max().unwrap_or(0) + 1;
    let scale = |len: Twips| Fixed16::from_f64(f64::from(len.get()) / f64::from(HIGHLIGHT_UNIT));
    button.records.push(ButtonRecord {
        states: ButtonState::UP | ButtonState::OVER | ButtonState::DOWN,
        id: overlay,
        depth,
        matrix: Matrix {
            a: scale(hit.width()),
            d: scale(hit.height()),
            tx: hit.x_min,
            ty: hit.y_min,
            ..Matrix::IDENTITY
        },
        color_transform: ColorTransform::IDENTITY,
        filters: vec![],
        blend_mode: BlendMode::Normal,
    });
}

fn filled_rect(id: CharacterId, color: Color) -> Shape {
    let size = Twips::new(HIGHLIGHT_UNIT);
    let edge = |dx: i32, dy: i32| ShapeRecord::StraightEdge {
        delta: swf::PointDelta::new(Twips::new(dx), Twips::new(dy)),
    };
    let bounds = Rectangle {
        x_min: Twips::ZERO,
        x_max: size,
        y_min: Twips::ZERO,
        y_max: size,
    };
    Shape {
        version: 3,
        id,
        shape_bounds: bounds.clone(),
        edge_bounds: bounds,
        flags: ShapeFlag::empty(),
        styles: ShapeStyles {
            fill_styles: vec![FillStyle::Color(color)],
            line_styles: vec![],
        },
        shape: vec![
            ShapeRecord::StyleChange(Box::new(StyleChangeData {
                move_to: Some(Point::new(Twips::ZERO, Twips::ZERO)),
                fill_style_0: None,
                fill_style_1: Some(1),
                line_style: None,
                new_styles: None,
            })),
            edge(HIGHLIGHT_UNIT, 0),
            edge(0, HIGHLIGHT_UNIT),
            edge(-HIGHLIGHT_UNIT, 0),
            edge(0, -HIGHLIGHT_UNIT),
        ],
    }
}

fn font_named(tags: &[Tag], name: &str) -> Option<CharacterId> {
    tags.iter().find_map(|t| match t {
        Tag::DefineFont2(f) if f.name.to_str_lossy(encoding_rs::WINDOWS_1250) == name => Some(f.id),
        Tag::DefineFontInfo(f) if f.name.to_str_lossy(encoding_rs::WINDOWS_1250) == name => Some(f.id),
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
        push(vec![s(&format!("FSCommand:{}", host::COMMAND)), s("english=")]),
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
    //! Run against a real copy: `ZK_GAME=/path/to/ZlatyKalich.exe cargo test -- --ignored`
    use super::*;

    fn game_swf() -> Vec<u8> {
        let path = std::env::var("ZK_GAME").expect("set ZK_GAME");
        crate::game::Game::open(std::path::Path::new(&path)).unwrap().swf
    }

    #[test]
    #[ignore]
    fn every_button_gets_an_overlay() {
        let patched = patch(&game_swf(), &[], Hotspots::HoldSpace).unwrap();
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
        assert!(without.is_empty(), "{with} patched, missing: {without:?}");
    }
}
