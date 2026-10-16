// SPDX-License-Identifier: EPL-2.0

//! Mouse and keyboard on the battlefield for interactive robots (classic's
//! `InteractiveHandler`): egui's input events become the java.awt-style `InputEvent`s
//! that the server hands to robots with the next turn (design §10).
//!
//! The pointer counts over the battlefield only (a window over it hides it); a drag or
//! release that began there is followed outside. Keys go to robots while the
//! battlefield has focus (the pointer is over it, or it was clicked last) and no text
//! field takes them; egui still sees every event, so no shortcut is lost.

use crate::view::Transform;
use eframe::egui::{self, Event, Key, Modifiers, MouseWheelUnit, PointerButton, Pos2, Rect};
use robocode_s_protocol::v1;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// java.awt.event.InputEvent's extended modifiers.
const SHIFT_DOWN: i32 = 1 << 6;
const CTRL_DOWN: i32 = 1 << 7;
const META_DOWN: i32 = 1 << 8;
const ALT_DOWN: i32 = 1 << 9;
const BUTTON_DOWN: [i32; 3] = [1 << 10, 1 << 11, 1 << 12];
/// `KeyEvent.CHAR_UNDEFINED`.
const CHAR_UNDEFINED: i32 = 0xFFFF;
/// `KeyEvent.KEY_LOCATION_STANDARD`, `_LEFT`, `_RIGHT`.
const STANDARD: i32 = 1;
const LEFT: i32 = 2;
const RIGHT: i32 = 3;
/// Clicks closer together than this count as one multi-click (`getClickCount`).
const MULTI_CLICK: Duration = Duration::from_millis(500);

/// The battlefield's pointer and keyboard state between frames.
#[derive(Default)]
pub struct Interaction {
    /// The pointer is over the battlefield.
    inside: bool,
    /// The battlefield was clicked last: it gets keys even when the pointer is elsewhere.
    focused: bool,
    /// Buttons pressed on the battlefield (java.awt numbers 1–3), still held.
    held: [bool; 3],
    /// The pointer moved since the last press (no click then, as java.awt).
    moved: bool,
    last_pos: Option<(i32, i32)>,
    /// The last click: when, which button, and how many in a row.
    last_click: Option<(Instant, i32, i32)>,
    /// Wheel movement not yet a whole notch.
    wheel: f32,
}

impl Interaction {
    /// This frame's input over the battlefield at `rect` (drawn with `t` on `layer`).
    pub fn events(
        &mut self,
        ctx: &egui::Context,
        layer: egui::LayerId,
        rect: Rect,
        t: &Transform,
    ) -> Vec<v1::InputEvent> {
        let when = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64);
        let over = |pos: Pos2| rect.contains(pos) && ctx.layer_id_at(pos) == Some(layer);
        let keyboard = !ctx.egui_wants_keyboard_input();
        let events = ctx.input(|i| i.events.clone());
        let mut out = Vec::new();
        for event in events {
            match event {
                Event::PointerMoved(pos) => {
                    let Some((x, y)) = t.field_point(pos) else {
                        continue;
                    };
                    let inside = over(pos);
                    let dragging = self.held.iter().any(|&h| h);
                    if inside != self.inside {
                        self.inside = inside;
                        let action = if inside {
                            v1::MouseAction::MouseEntered
                        } else {
                            v1::MouseAction::MouseExited
                        };
                        out.push(self.mouse(action, (x, y), 0, 0, Modifiers::NONE, when));
                    }
                    if self.last_pos == Some((x, y)) || !(inside || dragging) {
                        continue;
                    }
                    self.last_pos = Some((x, y));
                    self.moved = true;
                    let action = if dragging {
                        v1::MouseAction::MouseDragged
                    } else {
                        v1::MouseAction::MouseMoved
                    };
                    // One move a frame: the latest (the server keeps one a turn).
                    if let Some(last) = out.last_mut()
                        && mouse_action(last) == Some(action)
                    {
                        out.pop();
                    }
                    out.push(self.mouse(action, (x, y), 0, 0, current_modifiers(ctx), when));
                }
                Event::PointerGone => {
                    if self.inside {
                        self.inside = false;
                        let at = self.last_pos.unwrap_or_default();
                        let action = v1::MouseAction::MouseExited;
                        out.push(self.mouse(action, at, 0, 0, Modifiers::NONE, when));
                    }
                }
                Event::PointerButton {
                    pos,
                    button,
                    pressed,
                    modifiers,
                } => {
                    let Some(at) = t.field_point(pos) else {
                        continue;
                    };
                    let Some(b) = awt_button(button) else {
                        continue;
                    };
                    let slot = (b - 1) as usize;
                    if pressed {
                        self.focused = over(pos);
                        if !self.focused {
                            continue;
                        }
                        self.held[slot] = true;
                        self.moved = false;
                        let count = self.click_count(b);
                        let action = v1::MouseAction::MousePressed;
                        out.push(self.mouse(action, at, b, count, modifiers, when));
                    } else if self.held[slot] {
                        let count = self.click_count(b);
                        let action = v1::MouseAction::MouseReleased;
                        out.push(self.mouse(action, at, b, count, modifiers, when));
                        self.held[slot] = false;
                        if !self.moved && over(pos) {
                            self.last_click = Some((Instant::now(), b, count));
                            let action = v1::MouseAction::MouseClicked;
                            out.push(self.mouse(action, at, b, count, modifiers, when));
                        }
                    }
                }
                Event::MouseWheel {
                    unit,
                    delta,
                    modifiers,
                    ..
                } => {
                    let Some(pos) = ctx.input(|i| i.pointer.latest_pos()) else {
                        continue;
                    };
                    let Some(at) = t.field_point(pos).filter(|_| over(pos)) else {
                        continue;
                    };
                    // Notches; positive delta moves the content down: the wheel turned up,
                    // away from the user, which java.awt counts negative.
                    let per_notch = match unit {
                        MouseWheelUnit::Point => 50.0,
                        MouseWheelUnit::Line | MouseWheelUnit::Page => 1.0,
                    };
                    self.wheel -= delta.y / per_notch;
                    let notches = self.wheel.trunc();
                    if notches == 0.0 {
                        continue;
                    }
                    self.wheel -= notches;
                    let mut e =
                        self.mouse(v1::MouseAction::MouseWheelMoved, at, 0, 0, modifiers, when);
                    if let Some(v1::input_event::Kind::Mouse(m)) = &mut e.kind {
                        m.scroll_type = 0; // WHEEL_UNIT_SCROLL
                        m.scroll_amount = 3;
                        m.wheel_rotation = notches as i32;
                    }
                    out.push(e);
                }
                Event::Key {
                    key,
                    pressed,
                    modifiers,
                    ..
                } if keyboard && (self.inside || self.focused) => {
                    let Some((code, location)) = vk(key) else {
                        continue;
                    };
                    let action = if pressed {
                        v1::KeyAction::KeyPressed
                    } else {
                        v1::KeyAction::KeyReleased
                    };
                    let ch = key_char(key, modifiers);
                    out.push(key_event(action, code, ch, location, modifiers, when));
                    // java.awt types these too; egui sends no text for them.
                    if pressed
                        && matches!(
                            key,
                            Key::Enter | Key::Tab | Key::Backspace | Key::Escape | Key::Delete
                        )
                    {
                        let action = v1::KeyAction::KeyTyped;
                        out.push(key_event(action, 0, ch, 0, modifiers, when));
                    }
                }
                Event::Text(text) if keyboard && (self.inside || self.focused) => {
                    let modifiers = current_modifiers(ctx);
                    for unit in text.encode_utf16() {
                        let action = v1::KeyAction::KeyTyped;
                        out.push(key_event(action, 0, i32::from(unit), 0, modifiers, when));
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// Presses within [`MULTI_CLICK`] of a click with the same button count up.
    fn click_count(&self, button: i32) -> i32 {
        match self.last_click {
            Some((at, b, n)) if b == button && at.elapsed() < MULTI_CLICK => n + 1,
            _ => 1,
        }
    }

    fn mouse(
        &self,
        action: v1::MouseAction,
        (x, y): (i32, i32),
        button: i32,
        click_count: i32,
        modifiers: Modifiers,
        when: i64,
    ) -> v1::InputEvent {
        let held = self
            .held
            .iter()
            .zip(BUTTON_DOWN)
            .filter(|(h, _)| **h)
            .fold(0, |m, (_, mask)| m | mask);
        // As java.awt: a press includes its button's mask, a release doesn't.
        let held = match action {
            v1::MouseAction::MousePressed => held | BUTTON_DOWN[(button - 1) as usize],
            v1::MouseAction::MouseReleased | v1::MouseAction::MouseClicked => {
                held & !BUTTON_DOWN[(button - 1) as usize]
            }
            _ => held,
        };
        mouse_event(
            action,
            x,
            y,
            button,
            click_count,
            awt_modifiers(modifiers) | held,
            when,
        )
    }
}

/// A mouse event at a battlefield point.
pub fn mouse_event(
    action: v1::MouseAction,
    x: i32,
    y: i32,
    button: i32,
    click_count: i32,
    modifiers: i32,
    when: i64,
) -> v1::InputEvent {
    v1::InputEvent {
        kind: Some(v1::input_event::Kind::Mouse(v1::MouseInput {
            action: action as i32,
            x,
            y,
            button,
            click_count,
            modifiers,
            when,
            ..Default::default()
        })),
    }
}

/// A key event; `key_code` 0 (`VK_UNDEFINED`) and `key_location` 0 for typed keys.
pub fn key_event(
    action: v1::KeyAction,
    key_code: i32,
    key_char: i32,
    key_location: i32,
    modifiers: Modifiers,
    when: i64,
) -> v1::InputEvent {
    v1::InputEvent {
        kind: Some(v1::input_event::Kind::Key(v1::KeyInput {
            action: action as i32,
            key_code,
            key_char,
            key_location,
            modifiers: awt_modifiers(modifiers),
            when,
        })),
    }
}

/// `--input-at`'s `TURN:EVENT`: `move X,Y`, `drag X,Y`, `press X,Y[,BUTTON]`,
/// `release X,Y[,BUTTON]`, `click X,Y[,BUTTON]` (a press, release and click), `wheel
/// X,Y,NOTCHES`, `key NAME` (pressed; egui's key names, like ArrowUp or W) or `keyup
/// NAME`. Coordinates are the battlefield's (y up); buttons 1–3 (left, middle, right).
pub fn parse_scripted(text: &str) -> Result<(i64, Vec<v1::InputEvent>), String> {
    let (turn, event) = text.split_once(':').ok_or("no TURN:")?;
    let turn: i64 = turn.trim().parse().map_err(|_| "bad TURN")?;
    let (verb, args) = event.trim().split_once(' ').ok_or("no event")?;
    let numbers = || -> Result<Vec<i32>, String> {
        args.split(',')
            .map(|n| n.trim().parse().map_err(|_| format!("bad number {n:?}")))
            .collect()
    };
    let key = || -> Result<(i32, i32, i32), String> {
        let k = Key::from_name(args.trim()).ok_or(format!("unknown key {args:?}"))?;
        let (code, location) = vk(k).ok_or(format!("{args:?} has no java.awt key"))?;
        Ok((code, key_char(k, Modifiers::NONE), location))
    };
    let mouse = |action, n: &[i32]| -> Result<v1::InputEvent, String> {
        let (button, count) = match action {
            v1::MouseAction::MousePressed
            | v1::MouseAction::MouseReleased
            | v1::MouseAction::MouseClicked => (n.get(2).copied().unwrap_or(1), 1),
            _ => (0, 0),
        };
        if !(0..=3).contains(&button) {
            return Err(format!("bad button {button}"));
        }
        let held = match action {
            v1::MouseAction::MousePressed => BUTTON_DOWN[(button - 1).max(0) as usize],
            v1::MouseAction::MouseDragged => BUTTON_DOWN[0],
            _ => 0,
        };
        match n {
            [x, y, ..] => Ok(mouse_event(action, *x, *y, button, count, held, 0)),
            _ => Err("wants X,Y".into()),
        }
    };
    use v1::MouseAction as M;
    let events = match verb {
        "move" => vec![mouse(M::MouseMoved, &numbers()?)?],
        "drag" => vec![mouse(M::MouseDragged, &numbers()?)?],
        "press" => vec![mouse(M::MousePressed, &numbers()?)?],
        "release" => vec![mouse(M::MouseReleased, &numbers()?)?],
        "click" => {
            let n = numbers()?;
            vec![
                mouse(M::MousePressed, &n)?,
                mouse(M::MouseReleased, &n)?,
                mouse(M::MouseClicked, &n)?,
            ]
        }
        "wheel" => {
            let n = numbers()?;
            let mut e = mouse(M::MouseWheelMoved, &n)?;
            if let (Some(v1::input_event::Kind::Mouse(m)), Some(notches)) = (&mut e.kind, n.get(2))
            {
                m.scroll_amount = 3;
                m.wheel_rotation = *notches;
            }
            vec![e]
        }
        "key" | "keyup" => {
            let (code, ch, location) = key()?;
            let mut events = Vec::new();
            if verb == "key" {
                events.push(key_event(
                    v1::KeyAction::KeyPressed,
                    code,
                    ch,
                    location,
                    Modifiers::NONE,
                    0,
                ));
                if ch != CHAR_UNDEFINED {
                    events.push(key_event(
                        v1::KeyAction::KeyTyped,
                        0,
                        ch,
                        0,
                        Modifiers::NONE,
                        0,
                    ));
                }
            } else {
                events.push(key_event(
                    v1::KeyAction::KeyReleased,
                    code,
                    ch,
                    location,
                    Modifiers::NONE,
                    0,
                ));
            }
            events
        }
        _ => return Err(format!("unknown event {verb:?}")),
    };
    Ok((turn, events))
}

fn mouse_action(e: &v1::InputEvent) -> Option<v1::MouseAction> {
    match &e.kind {
        Some(v1::input_event::Kind::Mouse(m)) => Some(m.action()),
        _ => None,
    }
}

fn current_modifiers(ctx: &egui::Context) -> Modifiers {
    ctx.input(|i| i.modifiers)
}

fn awt_modifiers(m: Modifiers) -> i32 {
    let mut mask = 0;
    if m.shift {
        mask |= SHIFT_DOWN;
    }
    if m.ctrl {
        mask |= CTRL_DOWN;
    }
    if m.alt {
        mask |= ALT_DOWN;
    }
    if m.mac_cmd {
        mask |= META_DOWN;
    }
    mask
}

/// java.awt's `MouseEvent.BUTTON1..3`: left, middle, right.
fn awt_button(button: PointerButton) -> Option<i32> {
    match button {
        PointerButton::Primary => Some(1),
        PointerButton::Middle => Some(2),
        PointerButton::Secondary => Some(3),
        _ => None,
    }
}

/// The character a key press carries (`getKeyChar`), or `CHAR_UNDEFINED`.
fn key_char(key: Key, m: Modifiers) -> i32 {
    let name = key.name();
    let c = match key {
        Key::Enter => '\n',
        Key::Tab => '\t',
        Key::Backspace => '\u{8}',
        Key::Escape => '\u{1b}',
        Key::Delete => '\u{7f}',
        Key::Space => ' ',
        _ if name.len() == 1 => {
            let c = name.chars().next().unwrap_or_default();
            if c.is_ascii_alphabetic() && !m.shift {
                c.to_ascii_lowercase()
            } else if c.is_ascii_alphanumeric() {
                c
            } else {
                return CHAR_UNDEFINED;
            }
        }
        _ => return CHAR_UNDEFINED,
    };
    c as i32
}

/// java.awt's virtual key code (`KeyEvent.VK_*`) and key location, for the keys that
/// have one.
pub fn vk(key: Key) -> Option<(i32, i32)> {
    use Key as K;
    let code = match key {
        K::ShiftLeft => return Some((16, LEFT)),
        K::ShiftRight => return Some((16, RIGHT)),
        K::ControlLeft => return Some((17, LEFT)),
        K::ControlRight => return Some((17, RIGHT)),
        K::AltLeft => return Some((18, LEFT)),
        K::AltRight => return Some((18, RIGHT)),
        K::SuperLeft => return Some((524, LEFT)),
        K::SuperRight => return Some((524, RIGHT)),
        K::Backspace => 8,
        K::Tab => 9,
        K::Enter => 10,
        K::Escape => 27,
        K::Space => 32,
        K::PageUp => 33,
        K::PageDown => 34,
        K::End => 35,
        K::Home => 36,
        K::ArrowLeft => 37,
        K::ArrowUp => 38,
        K::ArrowRight => 39,
        K::ArrowDown => 40,
        K::Comma => 44,
        K::Minus => 45,
        K::Period => 46,
        K::Slash => 47,
        K::Semicolon => 59,
        K::Equals => 61,
        K::OpenBracket => 91,
        K::Backslash => 92,
        K::CloseBracket => 93,
        K::Delete => 127,
        K::Insert => 155,
        K::Backtick => 192,
        K::Quote => 222,
        K::Colon => 513,
        K::Exclamationmark => 517,
        K::Plus => 521,
        K::OpenCurlyBracket => 161,
        K::CloseCurlyBracket => 162,
        K::Copy => 65485,
        K::Paste => 65487,
        K::Cut => 65489,
        _ => {
            let name = key.name();
            if let Some(n) = name.strip_prefix('F').and_then(|n| n.parse::<i32>().ok()) {
                // VK_F1..F12, then VK_F13..F24; no VK beyond.
                match n {
                    1..=12 => 111 + n,
                    13..=24 => 61440 + n - 13,
                    _ => return None,
                }
            } else if let [c] = name.as_bytes()
                && c.is_ascii_alphanumeric()
            {
                // VK_0..9 and VK_A..Z are their ASCII codes.
                i32::from(*c)
            } else {
                return None;
            }
        }
    };
    Some((code, STANDARD))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_have_java_codes() {
        assert_eq!(vk(Key::ArrowUp), Some((38, STANDARD)));
        assert_eq!(vk(Key::W), Some((87, STANDARD)));
        assert_eq!(vk(Key::Num7), Some((55, STANDARD)));
        assert_eq!(vk(Key::F1), Some((112, STANDARD)));
        assert_eq!(vk(Key::F13), Some((61440, STANDARD)));
        assert_eq!(vk(Key::ShiftRight), Some((16, RIGHT)));
        assert_eq!(vk(Key::Pipe), None);
        assert_eq!(key_char(Key::A, Modifiers::NONE), 'a' as i32);
        assert_eq!(key_char(Key::A, Modifiers::SHIFT), 'A' as i32);
        assert_eq!(key_char(Key::Num3, Modifiers::NONE), '3' as i32);
        assert_eq!(key_char(Key::ArrowUp, Modifiers::NONE), CHAR_UNDEFINED);
    }

    #[test]
    fn scripted_events_parse() {
        let (turn, events) = parse_scripted("30:move 100,200").unwrap();
        assert_eq!(turn, 30);
        assert_eq!(
            events,
            vec![mouse_event(
                v1::MouseAction::MouseMoved,
                100,
                200,
                0,
                0,
                0,
                0
            )]
        );
        let (_, events) = parse_scripted("5:click 1,2,3").unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(
            mouse_action(&events[2]),
            Some(v1::MouseAction::MouseClicked)
        );
        let (_, events) = parse_scripted("5:key ArrowUp").unwrap();
        assert_eq!(
            events,
            vec![key_event(
                v1::KeyAction::KeyPressed,
                38,
                CHAR_UNDEFINED,
                STANDARD,
                Modifiers::NONE,
                0
            )]
        );
        assert!(parse_scripted("5:key Nope").is_err());
        assert!(parse_scripted("move 1,2").is_err());
        assert!(parse_scripted("5:press 1,2,7").is_err());
    }
}
