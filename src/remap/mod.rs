//! Mouse button remapping.
//!
//! Mirrors mac-mouse-fix's button capture: a source mouse button can be
//! remapped to nothing (captured/disabled), to another mouse button, or to a
//! keyboard key. The low-level hook swallows the original button event and the
//! executor synthesizes the target event via `SendInput` (down on press, up on
//! release). Events injected by us carry `LLMHF_INJECTED`, so the hook passes
//! them through untouched — no feedback loop.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP, MOUSEEVENTF_LEFTDOWN,
    MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_RIGHTDOWN,
    MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{XBUTTON1, XBUTTON2};

/// Mouse button identifiers (Windows wheel/mouse hook convention).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    X1,
    X2,
}

/// Runtime action produced for a remapped button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonAction {
    Disabled,
    Button(MouseButton),
    Key(u32),
}

/// One remap entry as written in `config.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemapEntry {
    pub source: MouseButton,
    /// `"disabled"` | `"button"` | `"key"`.
    #[serde(default)]
    pub action: String,
    /// For `action = "button"`: the target button.
    #[serde(default)]
    pub target: Option<MouseButton>,
    /// For `action = "key"`: the target virtual-key code.
    #[serde(default)]
    pub vk: Option<u32>,
}

impl RemapEntry {
    pub fn to_action(&self) -> ButtonAction {
        match self.action.as_str() {
            "button" => ButtonAction::Button(self.target.unwrap_or(MouseButton::Left)),
            "key" => ButtonAction::Key(self.vk.unwrap_or(0)),
            _ => ButtonAction::Disabled,
        }
    }
}

/// Lookup table from source button to action.
#[derive(Debug, Clone, Default)]
pub struct RemapTable {
    map: HashMap<MouseButton, ButtonAction>,
}

impl RemapTable {
    pub fn from_entries(entries: &[RemapEntry]) -> Self {
        let mut map = HashMap::new();
        for e in entries {
            map.insert(e.source, e.to_action());
        }
        Self { map }
    }

    pub fn lookup(&self, btn: MouseButton) -> Option<ButtonAction> {
        self.map.get(&btn).copied()
    }
}

/// Synthesize the target event for `action`. `down` selects press vs release.
pub fn execute(action: ButtonAction, down: bool) {
    match action {
        ButtonAction::Disabled => {}
        ButtonAction::Button(b) => unsafe { send_mouse_button(b, down) },
        ButtonAction::Key(vk) => unsafe { send_key(vk, down) },
    }
}

unsafe fn send_mouse_button(btn: MouseButton, down: bool) {
    let (flags, data) = match btn {
        MouseButton::Left => (
            if down { MOUSEEVENTF_LEFTDOWN } else { MOUSEEVENTF_LEFTUP },
            0,
        ),
        MouseButton::Right => (
            if down { MOUSEEVENTF_RIGHTDOWN } else { MOUSEEVENTF_RIGHTUP },
            0,
        ),
        MouseButton::Middle => (
            if down { MOUSEEVENTF_MIDDLEDOWN } else { MOUSEEVENTF_MIDDLEUP },
            0,
        ),
        MouseButton::X1 => (if down { MOUSEEVENTF_XDOWN } else { MOUSEEVENTF_XUP }, XBUTTON1),
        MouseButton::X2 => (if down { MOUSEEVENTF_XDOWN } else { MOUSEEVENTF_XUP }, XBUTTON2),
    };
    let mut input = INPUT {
        r#type: INPUT_MOUSE,
        ..std::mem::zeroed()
    };
    input.Anonymous.mi = MOUSEINPUT {
        dx: 0,
        dy: 0,
        mouseData: data as u32,
        dwFlags: flags,
        time: 0,
        dwExtraInfo: 0,
    };
    SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
}

unsafe fn send_key(vk: u32, down: bool) {
    let mut input = INPUT {
        r#type: INPUT_KEYBOARD,
        ..std::mem::zeroed()
    };
    input.Anonymous.ki = KEYBDINPUT {
        wVk: vk as u16,
        wScan: 0,
        dwFlags: if down { 0 } else { KEYEVENTF_KEYUP },
        time: 0,
        dwExtraInfo: 0,
    };
    SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_lookup_returns_action() {
        let entries = vec![
            RemapEntry {
                source: MouseButton::Middle,
                action: "key".into(),
                target: None,
                vk: Some(0x20), // space
            },
            RemapEntry {
                source: MouseButton::X1,
                action: "disabled".into(),
                target: None,
                vk: None,
            },
            RemapEntry {
                source: MouseButton::Right,
                action: "button".into(),
                target: Some(MouseButton::Middle),
                vk: None,
            },
        ];
        let t = RemapTable::from_entries(&entries);
        assert_eq!(t.lookup(MouseButton::Middle), Some(ButtonAction::Key(0x20)));
        assert_eq!(t.lookup(MouseButton::X1), Some(ButtonAction::Disabled));
        assert_eq!(
            t.lookup(MouseButton::Right),
            Some(ButtonAction::Button(MouseButton::Middle))
        );
        assert_eq!(t.lookup(MouseButton::Left), None);
    }

    #[test]
    fn unknown_action_falls_back_to_disabled() {
        let e = RemapEntry {
            source: MouseButton::Left,
            action: "bogus".into(),
            target: None,
            vk: None,
        };
        assert_eq!(e.to_action(), ButtonAction::Disabled);
    }
}
