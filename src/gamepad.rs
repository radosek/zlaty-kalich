//! Game controller input. The game is point-and-click, so the left stick
//! drives the mouse pointer.

use gilrs::{Axis, Button, EventType, Gilrs};

const DEADZONE: f32 = 0.18;

pub enum PadAction {
    /// Left mouse button down (true) / up (false).
    Click(bool),
    RightClick(bool),
    /// Hotspot highlight, same as holding Space.
    Highlight(bool),
    /// Opens the game's own menu (the remote control).
    GameMenu,
    Settings,
}

pub struct Gamepad {
    gilrs: Option<Gilrs>,
}

impl Gamepad {
    pub fn new() -> Self {
        let gilrs = Gilrs::new()
            .inspect_err(|e| tracing::warn!("No controller support: {e}"))
            .ok();
        Self { gilrs }
    }

    pub fn connected(&self) -> bool {
        self.gilrs
            .as_ref()
            .is_some_and(|g| g.gamepads().next().is_some())
    }

    pub fn poll(&mut self) -> Vec<PadAction> {
        let Some(gilrs) = &mut self.gilrs else {
            return vec![];
        };
        let mut actions = vec![];
        while let Some(event) = gilrs.next_event() {
            let (button, pressed) = match event.event {
                EventType::ButtonPressed(b, _) => (b, true),
                EventType::ButtonReleased(b, _) => (b, false),
                _ => continue,
            };
            actions.push(match (button, pressed) {
                (Button::South | Button::RightTrigger2, p) => PadAction::Click(p),
                (Button::East | Button::LeftTrigger2, p) => PadAction::RightClick(p),
                (Button::West | Button::North, p) => PadAction::Highlight(p),
                (Button::Start, true) => PadAction::GameMenu,
                (Button::Select | Button::Mode, true) => PadAction::Settings,
                _ => continue,
            });
        }
        actions
    }

    /// Left stick (or d-pad) deflection, -1..1 per axis, y pointing down.
    pub fn stick(&self) -> (f32, f32) {
        let Some(gilrs) = &self.gilrs else {
            return (0.0, 0.0);
        };
        let mut out = (0.0f32, 0.0f32);
        for (_, pad) in gilrs.gamepads() {
            let mut x = pad.value(Axis::LeftStickX);
            let mut y = -pad.value(Axis::LeftStickY);
            if pad.is_pressed(Button::DPadLeft) {
                x = -1.0;
            }
            if pad.is_pressed(Button::DPadRight) {
                x = 1.0;
            }
            if pad.is_pressed(Button::DPadUp) {
                y = -1.0;
            }
            if pad.is_pressed(Button::DPadDown) {
                y = 1.0;
            }
            if x.hypot(y) > out.0.hypot(out.1) {
                out = (x, y);
            }
        }
        if out.0.hypot(out.1) < DEADZONE {
            (0.0, 0.0)
        } else {
            out
        }
    }
}
