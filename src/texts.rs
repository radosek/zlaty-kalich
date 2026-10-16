//! Editable game text.
//!
//! `--export-texts FILE` writes every line of game text as UTF-8 TSV:
//!
//! ```text
//! lang	id	original	replacement
//! *		Vypadá to jako graffitti.	Vypadá to jako graffiti.
//! en	2725	Konečně je vše jak má být!	Finally, everything is as it should be!
//! ```
//!
//! - Rows with an empty `id` are script text (dialogue, hover labels, item
//!   names). They're matched by `original` and rewritten into the game's
//!   bytecode at load. The game keeps Czech and English lines separately, so
//!   `lang` is informational for these.
//! - Rows with an `id` are text drawn into the scenes. Only `en` is supported
//!   for those: the replacement is shown when the game runs in English.
//! - Empty `replacement` keeps the original. `\t`, `\n` and `\\` are escaped.
//!
//! Put the edited file at `<data dir>/texts.tsv`. Built-in fixes ship in
//! `assets/texts.tsv` and are overridden by the user's rows.

use crate::patch::StaticTranslation;
use anyhow::{Context, bail};
use std::collections::{BTreeSet, HashMap};
use std::fmt::Write as _;
use swf::avm1::read::Reader;
use swf::avm1::types::{Action, Value};
use swf::{CharacterId, SwfStr, Tag};

const BUILTIN: &str = include_str!("../assets/texts.tsv");
const HEADER: &str = "lang\tid\toriginal\treplacement";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    Any,
    Czech,
    English,
}

#[derive(Debug, PartialEq)]
struct Row {
    lang: Lang,
    id: Option<CharacterId>,
    original: String,
    replacement: String,
}

#[derive(Default)]
pub struct Texts {
    script: Vec<Row>,
    statics: Vec<Row>,
}

impl Texts {
    pub fn load(user: Option<&str>) -> anyhow::Result<Self> {
        let mut texts = Self::default();
        texts.add(BUILTIN).context("built-in texts")?;
        if let Some(user) = user {
            texts.add(user).context("texts.tsv")?;
        }
        Ok(texts)
    }

    fn add(&mut self, tsv: &str) -> anyhow::Result<()> {
        for row in parse(tsv)? {
            let list = if row.id.is_some() {
                &mut self.statics
            } else {
                &mut self.script
            };
            list.retain(|r| !(r.lang == row.lang && r.id == row.id && r.original == row.original));
            list.push(row);
        }
        Ok(())
    }

    pub fn static_translations(&self) -> Vec<StaticTranslation> {
        self.statics
            .iter()
            .filter(|r| r.lang == Lang::English)
            .map(|r| StaticTranslation {
                id: r.id.expect("static rows have ids"),
                english: r.replacement.clone(),
            })
            .collect()
    }

    /// Script replacements: original cp1250 bytes -> new text.
    pub fn script_overrides(&self) -> HashMap<Vec<u8>, String> {
        self.script
            .iter()
            .map(|r| (encode(&r.original), r.replacement.clone()))
            .collect()
    }
}

fn parse(tsv: &str) -> anyhow::Result<Vec<Row>> {
    let mut rows = Vec::new();
    for (n, line) in tsv.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') || line == HEADER {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        let [lang, id, original, replacement] = cols[..] else {
            bail!("line {}: expected 4 tab-separated columns", n + 1);
        };
        if replacement.is_empty() {
            continue;
        }
        let lang = match lang {
            "*" => Lang::Any,
            "cs" => Lang::Czech,
            "en" => Lang::English,
            other => bail!("line {}: unknown language {other:?}", n + 1),
        };
        let id = match id {
            "" => None,
            id => Some(
                id.parse()
                    .with_context(|| format!("line {}: bad id", n + 1))?,
            ),
        };
        if id.is_some() && lang != Lang::English {
            bail!("line {}: drawn text can only be replaced for `en`", n + 1);
        }
        rows.push(Row {
            lang,
            id,
            original: unescape(original),
            replacement: unescape(replacement),
        });
    }
    Ok(rows)
}

fn encode(s: &str) -> Vec<u8> {
    encoding_rs::WINDOWS_1250.encode(s).0.into_owned()
}

fn decode(s: &SwfStr) -> String {
    s.to_string_lossy(encoding_rs::WINDOWS_1250)
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\r', "")
        .replace('\n', "\\n")
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// Dialogue and labels, not identifiers, paths or file names.
fn is_human_text(s: &str) -> bool {
    let letters = s.chars().filter(|c| c.is_alphabetic()).count();
    letters >= 2
        && (s.contains(' ') || !s.is_ascii())
        && !s.starts_with("_root")
        && !s.starts_with("FSCommand:")
        && !s.contains("/:")
        && !s.contains(".mid")
}

pub fn export(swf_data: &[u8]) -> anyhow::Result<String> {
    let buf = swf::decompress_swf(swf_data)?;
    let parsed = swf::parse_swf(&buf)?;
    let version = parsed.header.version();

    let mut script = BTreeSet::new();
    let collect = |script: &mut BTreeSet<String>, code: &[u8]| {
        let mut reader = Reader::new(code, version);
        while let Ok(action) = reader.read_action() {
            match action {
                Action::End => break,
                Action::ConstantPool(pool) => script.extend(pool.strings.iter().map(|s| decode(s))),
                Action::Push(push) => script.extend(push.values.iter().filter_map(|v| match v {
                    Value::Str(s) => Some(decode(s)),
                    _ => None,
                })),
                _ => {}
            }
        }
    };
    let mut statics = Vec::new();
    let fonts = font_codes(&parsed.tags);
    visit(&parsed.tags, &mut |tag| match tag {
        Tag::DoAction(code) => collect(&mut script, code),
        Tag::DefineButton2(b) => b
            .actions
            .iter()
            .for_each(|a| collect(&mut script, a.action_data)),
        Tag::PlaceObject(po) => {
            if let Some(clip) = &po.clip_actions {
                clip.records
                    .iter()
                    .for_each(|r| collect(&mut script, r.action_data));
            }
        }
        Tag::DefineEditText(e) => {
            if let Some(text) = e.initial_text() {
                script.insert(decode(text));
            }
        }
        Tag::DefineText(t) | Tag::DefineText2(t) => {
            let text = static_text(t, &fonts);
            if is_human_text(&text) {
                statics.push((t.id, text));
            }
        }
        _ => {}
    });

    let mut out = String::new();
    writeln!(out, "{HEADER}")?;
    writeln!(
        out,
        "# Fill in `replacement`; see `texts.rs` docs for the format."
    )?;
    for s in script.iter().filter(|s| is_human_text(s)) {
        writeln!(out, "*\t\t{}\t", escape(s))?;
    }
    for (id, text) in statics {
        writeln!(out, "en\t{id}\t{}\t", escape(&text))?;
    }
    Ok(out)
}

fn visit<'a>(tags: &[Tag<'a>], f: &mut impl FnMut(&Tag<'a>)) {
    for tag in tags {
        if let Tag::DefineSprite(sprite) = tag {
            visit(&sprite.tags, f);
        }
        f(tag);
    }
}

/// Glyph index -> character code for each embedded font.
fn font_codes(tags: &[Tag]) -> HashMap<CharacterId, Vec<u16>> {
    let mut map = HashMap::new();
    for tag in tags {
        match tag {
            Tag::DefineFont2(f) => {
                map.insert(f.id, f.glyphs.iter().map(|g| g.code).collect());
            }
            Tag::DefineFontInfo(f) => {
                map.insert(f.id, f.code_table.clone());
            }
            _ => {}
        }
    }
    map
}

fn static_text(text: &swf::Text, fonts: &HashMap<CharacterId, Vec<u16>>) -> String {
    let mut out = String::new();
    let mut font = None;
    for (i, record) in text.records.iter().enumerate() {
        font = record.font_id.or(font);
        if i > 0 && record.y_offset.is_some() {
            out.push('\n');
        }
        let codes = font.and_then(|f| fonts.get(&f));
        for glyph in &record.glyphs {
            let code = codes.and_then(|c| c.get(glyph.index as usize)).copied();
            match code {
                Some(c) if c < 256 => out.push_str(&decode(SwfStr::from_bytes(&[c as u8]))),
                Some(c) => out.extend(char::from_u32(c.into())),
                None => out.push('?'),
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_escapes() {
        let s = "a\tb\nc\\d";
        assert_eq!(unescape(&escape(s)), s);
    }

    #[test]
    fn parses_rows_and_skips_untouched() {
        let rows = parse(&format!(
            "{HEADER}\n# c\n*\t\tAhoj světe\tNazdar světe\n*\t\tKeep me\t\nen\t2725\tKonečně\tFinally\n"
        ))
        .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].replacement, "Nazdar světe");
        assert_eq!(rows[1].id, Some(2725));
    }

    #[test]
    fn rejects_czech_static_rows() {
        assert!(parse("cs\t5\tx\ty\n").is_err());
    }

    #[test]
    fn script_overrides_are_keyed_by_cp1250_bytes() {
        let mut t = Texts::default();
        t.add("*\t\tŘeka\tPotok\n").unwrap();
        assert_eq!(t.script_overrides().get(&encode("Řeka")).unwrap(), "Potok");
    }

    #[test]
    fn filters_identifiers() {
        assert!(is_human_text("Vypadá to jako graffitti."));
        assert!(is_human_text("Čech"));
        assert!(!is_human_text("_root.infoline"));
        assert!(!is_human_text("talkq"));
        assert!(!is_human_text("player.exe\tmenu.mid"));
    }
}
