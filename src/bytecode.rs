//! Replacing strings inside AVM1 bytecode.
//!
//! Strings live in `Push` and `ConstantPool` actions. Changing their length
//! moves every later action, so `If`/`Jump` offsets are relinked; function,
//! `with` and `try` bodies are rewritten recursively and re-emitted with their
//! new sizes.

use std::collections::HashMap;
use swf::SwfStr;
use swf::avm1::read::Reader;
use swf::avm1::types::{
    Action, ConstantPool, DefineFunction, DefineFunction2, If, Jump, Push, Value, With,
};
use swf::avm1::write::Writer;

/// Returns the replacement bytes for a string, if it should change.
pub type Replace<'r> = &'r dyn Fn(&SwfStr) -> Option<&'static SwfStr>;

/// Rewrites `code`, or returns `None` if nothing changed (or the code uses
/// branches that don't land on action boundaries, which is left untouched).
pub fn rewrite(code: &[u8], version: u8, replace: Replace) -> Option<Vec<u8>> {
    struct Item<'a> {
        start: usize,
        end: usize,
        action: Action<'a>,
        changed: bool,
    }

    let mut reader = Reader::new(code, version);
    let mut items = Vec::new();
    let mut any = false;
    loop {
        let start = code.len() - reader.get_ref().len();
        let Ok(action) = reader.read_action() else {
            return None;
        };
        let end = code.len() - reader.get_ref().len();
        let is_end = matches!(action, Action::End);
        let (action, changed) = replace_in(action, version, replace);
        any |= changed;
        items.push(Item {
            start,
            end,
            action,
            changed,
        });
        if is_end || end >= code.len() {
            break;
        }
    }
    if !any {
        return None;
    }

    // New layout: unchanged actions keep their size, changed ones are re-encoded.
    let encoded: Vec<Option<Vec<u8>>> = items
        .iter()
        .map(|i| i.changed.then(|| encode(&i.action, version)))
        .collect();
    let mut new_pos = HashMap::with_capacity(items.len() + 1);
    let mut pos = 0usize;
    for (item, bytes) in items.iter().zip(&encoded) {
        new_pos.insert(item.start, pos);
        pos += bytes.as_ref().map_or(item.end - item.start, Vec::len);
    }
    let tail = items.last().map_or(0, |i| i.end);
    new_pos.insert(tail, pos);

    let mut out = Vec::with_capacity(pos + code.len() - tail);
    for (item, bytes) in items.iter().zip(encoded) {
        let new_end = out.len() + bytes.as_ref().map_or(item.end - item.start, Vec::len);
        let relink = |offset: i16| -> Option<i16> {
            let target = item.end.checked_add_signed(offset.into())?;
            let new_target = *new_pos.get(&target)?;
            i16::try_from(new_target as isize - new_end as isize).ok()
        };
        match (&item.action, bytes) {
            (Action::If(If { offset }), _) => {
                let offset = relink(*offset)?;
                out.extend(encode(&Action::If(If { offset }), version));
            }
            (Action::Jump(Jump { offset }), _) => {
                let offset = relink(*offset)?;
                out.extend(encode(&Action::Jump(Jump { offset }), version));
            }
            (_, Some(bytes)) => out.extend(bytes),
            (_, None) => out.extend_from_slice(&code[item.start..item.end]),
        }
    }
    out.extend_from_slice(&code[tail..]);
    Some(out)
}

fn encode(action: &Action, version: u8) -> Vec<u8> {
    let mut out = Vec::new();
    Writer::new(&mut out, version)
        .write_action(action)
        .expect("writing to a Vec");
    out
}

fn leak(bytes: Vec<u8>) -> &'static [u8] {
    Box::leak(bytes.into_boxed_slice())
}

fn replace_in<'a>(action: Action<'a>, version: u8, replace: Replace) -> (Action<'a>, bool) {
    let nested = |body: &'a [u8]| rewrite(body, version, replace).map(leak);
    match action {
        Action::Push(Push { values }) => {
            let mut changed = false;
            let values = values
                .into_iter()
                .map(|v| match v {
                    Value::Str(s) => match replace(s) {
                        Some(new) => {
                            changed = true;
                            Value::Str(new)
                        }
                        None => Value::Str(s),
                    },
                    other => other,
                })
                .collect();
            (Action::Push(Push { values }), changed)
        }
        Action::ConstantPool(ConstantPool { strings }) => {
            let mut changed = false;
            let strings = strings
                .into_iter()
                .map(|s| replace(s).inspect(|_| changed = true).unwrap_or(s))
                .collect();
            (Action::ConstantPool(ConstantPool { strings }), changed)
        }
        Action::DefineFunction(f) => match nested(f.actions) {
            Some(actions) => (
                Action::DefineFunction(DefineFunction { actions, ..f }),
                true,
            ),
            None => (Action::DefineFunction(f), false),
        },
        Action::DefineFunction2(f) => match nested(f.actions) {
            Some(actions) => (
                Action::DefineFunction2(DefineFunction2 { actions, ..f }),
                true,
            ),
            None => (Action::DefineFunction2(f), false),
        },
        Action::With(w) => match nested(w.actions) {
            Some(actions) => (Action::With(With { actions }), true),
            None => (Action::With(w), false),
        },
        other => (other, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swf::avm1::types::GotoFrame;

    fn s(text: &str) -> &'static SwfStr {
        SwfStr::from_bytes(leak(text.as_bytes().to_vec()))
    }

    fn assemble(actions: &[Action]) -> Vec<u8> {
        let mut out: Vec<u8> = actions.iter().flat_map(|a| encode(a, 5)).collect();
        out.push(0);
        out
    }

    fn decode(code: &[u8]) -> Vec<String> {
        let mut r = Reader::new(code, 5);
        let mut out = vec![];
        while let Ok(a) = r.read_action() {
            if matches!(a, Action::End) {
                break;
            }
            out.push(format!("{a:?}"));
        }
        out
    }

    fn longer(x: &SwfStr) -> Option<&'static SwfStr> {
        (x.as_bytes() == b"old").then(|| s("a much longer replacement"))
    }

    #[test]
    fn relinks_forward_and_backward_branches() {
        // 0: push "old"; If -> past the stop; Stop; GotoFrame; Jump -> back to push.
        let push = Action::Push(Push {
            values: vec![Value::Str(s("old"))],
        });
        let stop = encode(&Action::Stop, 5).len() as i16;
        let goto = encode(&Action::GotoFrame(GotoFrame { frame: 1 }), 5).len() as i16;
        let push_len = encode(&push, 5).len() as i16;
        let code = assemble(&[
            push.clone(),
            Action::If(If { offset: stop }),
            Action::Stop,
            Action::GotoFrame(GotoFrame { frame: 1 }),
            Action::Jump(Jump {
                offset: -(push_len + 5 + stop + goto + 5),
            }),
        ]);
        let out = rewrite(&code, 5, &longer).unwrap();
        let new_push = encode(
            &Action::Push(Push {
                values: vec![Value::Str(s("a much longer replacement"))],
            }),
            5,
        )
        .len() as i16;
        let actions = decode(&out);
        assert!(actions[0].contains("a much longer replacement"));
        assert_eq!(actions[1], format!("{:?}", Action::If(If { offset: stop })));
        assert_eq!(
            actions[4],
            format!(
                "{:?}",
                Action::Jump(Jump {
                    offset: -(new_push + 5 + stop + goto + 5)
                })
            )
        );
    }

    #[test]
    fn rewrites_inside_function_bodies() {
        let body = assemble(&[Action::Push(Push {
            values: vec![Value::Str(s("old"))],
        })]);
        let code = assemble(&[Action::DefineFunction(DefineFunction {
            name: s("f"),
            params: vec![],
            actions: leak(body),
        })]);
        let out = rewrite(&code, 5, &longer).unwrap();
        let mut r = Reader::new(&out, 5);
        let Action::DefineFunction(f) = r.read_action().unwrap() else {
            panic!()
        };
        assert!(decode(f.actions)[0].contains("a much longer replacement"));
    }

    #[test]
    fn unchanged_code_is_left_alone() {
        let code = assemble(&[Action::Stop]);
        assert!(rewrite(&code, 5, &longer).is_none());
    }
}
