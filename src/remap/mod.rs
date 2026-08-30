//! Mouse button remapping.
//!
//! Architecture:
//! - `RemapEntry`           — one row in config.toml
//! - `RemapTable`          — built from config, keyed by trigger + modifiers
//! - `ClickCycleTracker`    — tracks button press → held → levelExpired/release
//! - `RemapEngine`          — matches active modifiers against table, produces effects
//! - `execute_effect`       — executes the matched effects

use std::collections::HashMap;
use parking_lot::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
    MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP,
    MOUSEINPUT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{XBUTTON1, XBUTTON2};

mod engine;
pub use engine::RemapEngine;

// ─── Button identifiers ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    X1,
    X2,
}

impl MouseButton {
    pub fn from_windows(button: u32) -> Option<Self> {
        match button {
            0 => Some(MouseButton::Left),
            1 => Some(MouseButton::Right),
            2 => Some(MouseButton::Middle),
            3 => Some(MouseButton::X1),
            4 => Some(MouseButton::X2),
            _ => None,
        }
    }
}

// ─── Triggers ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Trigger {
    Button {
        button: MouseButton,
        level: u8,
        duration: ClickDuration,
    },
    Scroll,
    Drag,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClickDuration {
    Click,
    Hold,
}

// ─── Modifiers ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModifierCondition {
    #[serde(default)]
    pub keyboard: u32,
    #[serde(default)]
    pub buttons: Vec<MouseButton>,
}

impl ModifierCondition {
    pub fn is_subset_of(&self, other: &ModifierCondition) -> bool {
        (other.keyboard & self.keyboard) == self.keyboard
            && self.buttons.iter().all(|b| other.buttons.contains(b))
    }

    pub fn is_empty(&self) -> bool {
        self.keyboard == 0 && self.buttons.is_empty()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActiveModifiers {
    pub keyboard: u32,
    pub buttons: Vec<MouseButton>,
}

impl ActiveModifiers {
    /// Returns true if `required` modifier condition is satisfied by self.
    /// i.e. all modifiers in `required` are also present in `self`.
    pub fn satisfies(&self, required: &ModifierCondition) -> bool {
        (self.keyboard & required.keyboard) == required.keyboard
            && required.buttons.iter().all(|b| self.buttons.contains(b))
    }
}

// ─── Effects ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", content = "data")]
pub enum Effect {
    SymbolicHotkey { keycode: u16, #[serde(default)] flags: u32 },
    NavigationSwipe { direction: SwipeDirection },
    MouseButtonClicks { button: MouseButton, #[serde(rename = "n_of_clicks")] n_of_clicks: u8 },
    SystemDefinedEvent { #[serde(rename = "event_type")] event_type: u32, #[serde(default)] flags: u32 },
    ModifiedScroll { modification: ModifiedScrollModification },
    ModifiedDrag { drag_type: ModifiedDragType, #[serde(default)] variant: Option<ModifiedDragVariant> },
    TaskView,      // Win+Tab — virtual desktop overview
    ShowDesktop,   // Win+D — minimize all windows / restore
    Disabled,
    PassThrough,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SwipeDirection {
    Forward,
    Back,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModifiedScrollModification {
    Precision,
    Fast,
    Horizontal,
    Zoom,
    FourFingerPinch,
    ThreeFingerSwipeHorizontal,
    Rotate,
    CommandTab,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModifiedDragType {
    TwoFingerSwipe,
    ThreeFingerSwipe,
    FourFingerSwipe,
    FakeDrag,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", content = "data")]
pub enum ModifiedDragVariant {
    ButtonNumber { button: MouseButton },
}

// ─── RemapEntry ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemapEntry {
    #[serde(default)]
    pub modifiers: ModifierCondition,
    pub trigger: Trigger,
    pub effect: Effect,
}

/// Legacy remap entry (source button → action). Used for simple button remapping.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LegacyRemapEntry {
    pub source: MouseButton,
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub target: Option<MouseButton>,
    #[serde(default)]
    pub vk: Option<u32>,
}

impl LegacyRemapEntry {
    pub fn to_action(&self) -> ButtonAction {
        match self.action.as_str() {
            "button" => ButtonAction::Button(self.target.unwrap_or(MouseButton::Left)),
            "key" => ButtonAction::Key(self.vk.unwrap_or(0)),
            _ => ButtonAction::Disabled,
        }
    }
}

// ─── Action phase ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionPhase {
    Start,
    End,
    Combined,
}

// ─── Runtime button tracking ─────────────────────────────────────────────────

struct ClickState {
    #[allow(dead_code)]
    button: MouseButton,
    down_at: Instant,
    click_count: u8,
    max_level: u8,
    held_fired: bool,
    level_expired: bool,
}

impl ClickState {
    /// Marks this click state as a modifier (button held → acts as modifier key).
    #[allow(dead_code)]
    fn set_modifier(&mut self) {
        // A button used as modifier doesn't expire via level timers;
        // it stays active until released.
    }

    fn is_modifier(&self) -> bool {
        // Currently a button is a modifier if it has no associated remap
        // at click_count=1 (i.e., it was set up for hold-as-modifier usage).
        // We use max_level = 0 to indicate modifier-only mode.
        self.max_level == 0
    }

    fn new(button: MouseButton) -> Self {
        Self {
            button,
            down_at: Instant::now(),
            click_count: 1,
            max_level: 1,
            held_fired: false,
            level_expired: false,
        }
    }

    fn hold_duration_ms(&self) -> u64 {
        self.down_at.elapsed().as_millis() as u64
    }
}

pub struct ClickCycleTracker {
    active: HashMap<MouseButton, ClickState>,
    release_callbacks: Mutex<HashMap<MouseButton, Vec<Box<dyn FnOnce() + Send + 'static>>>>,
    double_click_ms: u64,
    level_timers_ms: [u64; 3],
}

impl Default for ClickCycleTracker {
    fn default() -> Self {
        Self {
            active: HashMap::new(),
            release_callbacks: Mutex::new(HashMap::new()),
            double_click_ms: 500,
            level_timers_ms: [300, 200, 0],
        }
    }
}

impl ClickCycleTracker {
    pub fn new() -> Self {
        Self {
            active: HashMap::new(),
            release_callbacks: Mutex::new(HashMap::new()),
            double_click_ms: 500,
            level_timers_ms: [300, 200, 0],
        }
    }

    /// Set the max click level for a button (called by hooks.rs after engine lookup).
    pub fn set_max_level(&mut self, button: MouseButton, level: u8) {
        if let Some(state) = self.active.get_mut(&button) {
            state.max_level = level;
        }
    }

    /// Marks a button as a held modifier (e.g., right-click held = acts as Ctrl).
    /// When set, the button's max_level becomes 0 and it stays active indefinitely.
    #[allow(dead_code)]
    pub fn set_modifier(&mut self, button: MouseButton) {
        if let Some(state) = self.active.get_mut(&button) {
            state.set_modifier();
        }
    }

    /// Returns true if `button` is currently held as a modifier.
    #[allow(dead_code)]
    pub fn is_modifier_button(&self, button: MouseButton) -> bool {
        self.active.get(&button).map(|s| s.is_modifier()).unwrap_or(false)
    }

    /// Returns a list of all currently-held modifier buttons.
    pub fn held_modifier_buttons(&self) -> Vec<MouseButton> {
        self.active
            .iter()
            .filter(|(_, s)| s.is_modifier())
            .map(|(&btn, _)| btn)
            .collect()
    }

    pub fn on_button_down(&mut self, button: MouseButton) -> (u8, bool) {
        let is_new;
        let click_count;

        if let Some(state) = self.active.get_mut(&button) {
            if state.hold_duration_ms() < self.double_click_ms && !state.level_expired {
                state.click_count = state.click_count.saturating_add(1).min(3);
            } else {
                *state = ClickState::new(button);
            }
            is_new = false;
            click_count = state.click_count;
        } else {
            self.active.insert(button, ClickState::new(button));
            is_new = true;
            click_count = 1;
        }

        (click_count, is_new)
    }

    pub fn on_button_up(
        &mut self,
        button: MouseButton,
        engine: Option<&mut RemapEngine>,
        active_mods: &ActiveModifiers,
    ) -> Vec<(Effect, ActionPhase)> {
        let Some(state) = self.active.remove(&button) else {
            return Vec::new();
        };

        if let Some(callbacks) = self.release_callbacks.lock().remove(&button) {
            for cb in callbacks {
                cb();
            }
        }

        match engine {
            Some(engine) => engine.resolve_effects(button, state.click_count, state.held_fired, active_mods),
            None => Vec::new(),
        }
    }

    pub fn tick(&mut self, engine: &mut RemapEngine, active_mods: &ActiveModifiers) {
        let mut expired = Vec::new();

        for (button, state) in &mut self.active {
            if state.level_expired {
                continue;
            }
            let held = state.hold_duration_ms();
            let idx = state.click_count.saturating_sub(1) as usize;
            let threshold = self.level_timers_ms.get(idx).copied().unwrap_or(0);

            if held >= threshold && threshold > 0 {
                state.level_expired = true;
                expired.push(*button);
            }
        }

        for button in expired {
            if let Some(state) = self.active.get(&button) {
                let effects =
                    engine.resolve_effects(button, state.click_count, state.held_fired, active_mods);
                for (effect, phase) in effects {
                    execute_effect_phase(&effect, phase);
                }
                if let Some(s) = self.active.get_mut(&button) {
                    s.held_fired = true;
                }
            }
        }
    }

    #[allow(dead_code)]
    pub fn is_active(&self, button: MouseButton) -> bool {
        self.active.contains_key(&button)
    }

    #[allow(dead_code)]
    pub fn on_release(&mut self, button: MouseButton, cb: impl FnOnce() + Send + 'static) {
        self.release_callbacks
            .lock()
            .entry(button)
            .or_default()
            .push(Box::new(cb));
    }

    pub fn active_count(&self) -> usize {
        self.active.len()
    }

    #[allow(dead_code)]
    pub fn expire_all(&mut self) {
        self.active.clear();
        self.release_callbacks.lock().clear();
    }
}

// ─── Effect execution ─────────────────────────────────────────────────────────

pub fn execute_effect(effect: &Effect) {
    match effect {
        Effect::SymbolicHotkey { keycode, flags } => unsafe { send_symbolic_hotkey(*keycode, *flags) },
        Effect::NavigationSwipe { direction } => unsafe { send_navigation_swipe(*direction) },
        Effect::MouseButtonClicks { button, n_of_clicks } => {
            for _ in 0..*n_of_clicks {
                unsafe {
                    send_mouse_button(*button, true);
                    std::thread::sleep(Duration::from_millis(50));
                    send_mouse_button(*button, false);
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
        Effect::ModifiedDrag { drag_type, .. } => {
            match drag_type {
                ModifiedDragType::TwoFingerSwipe
                | ModifiedDragType::ThreeFingerSwipe
                | ModifiedDragType::FourFingerSwipe => {
                    // Swipe direction is determined and emitted at mousemove time
                    // from the WM_MOUSEMOVE handler in hooks.rs. This arm is a
                    // no-op here — the actual hotkey is sent in the low-level
                    // mouse hook based on drag axis and accumulated delta.
                }
                ModifiedDragType::FakeDrag => {}
            }
        }
        Effect::SystemDefinedEvent { event_type, flags } => unsafe { send_system_event(*event_type, *flags) },
        Effect::ModifiedScroll { .. } => {
            // ModifiedScroll is handled by the scroll engine directly, not here.
        }
        Effect::TaskView => unsafe { send_task_view() },
        Effect::ShowDesktop => unsafe { send_show_desktop() },
        Effect::Disabled => {}
        Effect::PassThrough => {}
    }
}

/// Execute an effect for a specific action phase. `Combined` fires the full
/// effect (e.g. a complete key press+release). `Start` (a hold began) presses
/// the key/modifier down and keeps it held; `End` (the trigger released)
/// releases it. This makes "hold a button = act as a modifier key" work
/// instead of dropping the release on the floor.
pub fn execute_effect_phase(effect: &Effect, phase: ActionPhase) {
    match phase {
        ActionPhase::Combined => execute_effect(effect),
        ActionPhase::Start => send_effect_down(effect),
        ActionPhase::End => send_effect_up(effect),
    }
}

fn send_effect_down(effect: &Effect) {
    if let Effect::SymbolicHotkey { keycode, flags } = effect {
        unsafe { send_symbolic_hotkey_down(*keycode, *flags) };
    }
}

fn send_effect_up(effect: &Effect) {
    if let Effect::SymbolicHotkey { keycode, flags } = effect {
        unsafe { send_symbolic_hotkey_up(*keycode, *flags) };
    }
}

#[derive(Clone, Copy)]
enum KeyState { Down, Up, Both }

unsafe fn symbolic_hotkey_inputs(keycode: u16, flags: u32, state: KeyState) -> Vec<INPUT> {
    let ctrl = (flags & 0x100) != 0;
    let alt  = (flags & 0x400) != 0;
    let shift = (flags & 0x200) != 0;
    let win  = (flags & 0x800) != 0;
    let key_flags = flags & 0x7FF;

    let mut inputs: Vec<INPUT> = Vec::with_capacity(8);
    let down = |inputs: &mut Vec<INPUT>, vk: u16| inputs.push(vk_input(vk, key_flags, true));
    let up = |inputs: &mut Vec<INPUT>, vk: u16| inputs.push(vk_input(vk, key_flags, false));

    match state {
        KeyState::Down => {
            if ctrl { down(&mut inputs, 0xA2); }
            if alt  { down(&mut inputs, 0xA4); }
            if shift { down(&mut inputs, 0xA0); }
            if win  { down(&mut inputs, 0x5B); }
            down(&mut inputs, keycode);
        }
        KeyState::Up => {
            up(&mut inputs, keycode);
            if win  { up(&mut inputs, 0x5B); }
            if shift { up(&mut inputs, 0xA0); }
            if alt  { up(&mut inputs, 0xA4); }
            if ctrl { up(&mut inputs, 0xA2); }
        }
        KeyState::Both => {
            if ctrl { down(&mut inputs, 0xA2); }
            if alt  { down(&mut inputs, 0xA4); }
            if shift { down(&mut inputs, 0xA0); }
            if win  { down(&mut inputs, 0x5B); }
            down(&mut inputs, keycode);
            up(&mut inputs, keycode);
            if win  { up(&mut inputs, 0x5B); }
            if shift { up(&mut inputs, 0xA0); }
            if alt  { up(&mut inputs, 0xA4); }
            if ctrl { up(&mut inputs, 0xA2); }
        }
    }
    inputs
}

unsafe fn send_symbolic_hotkey(keycode: u16, flags: u32) {
    let inputs = symbolic_hotkey_inputs(keycode, flags, KeyState::Both);
    SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32);
}

unsafe fn send_symbolic_hotkey_down(keycode: u16, flags: u32) {
    let inputs = symbolic_hotkey_inputs(keycode, flags, KeyState::Down);
    SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32);
}

unsafe fn send_symbolic_hotkey_up(keycode: u16, flags: u32) {
    let inputs = symbolic_hotkey_inputs(keycode, flags, KeyState::Up);
    SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32);
}

unsafe fn vk_input(vk: u16, flags: u32, down: bool) -> INPUT {
    let mut i = INPUT { r#type: INPUT_KEYBOARD, Anonymous: std::mem::zeroed() };
    i.Anonymous.ki = KEYBDINPUT {
        wVk: vk,
        wScan: 0,
        dwFlags: if down { flags } else { flags | KEYEVENTF_KEYUP },
        time: 0,
        dwExtraInfo: 0,
    };
    i
}

unsafe fn send_navigation_swipe(direction: SwipeDirection) {
    // Mirrors mac TouchSimulator postNavigationSwipeEventWithDirection:
    // maps to Windows VK_BROWSER_BACK / VK_BROWSER_FORWARD which work
    // in most browsers, file explorers, and many other apps.
    let (vk, flags) = match direction {
        SwipeDirection::Forward => (0xA7, 0x800u32), // VK_BROWSER_FORWARD
        SwipeDirection::Back => (0xA6, 0x800u32),   // VK_BROWSER_BACK
    };
    send_symbolic_hotkey(vk as u16, flags);
}
pub(crate) fn send_virtual_desktop_switch(direction: SwipeDirection) {
    // Switch virtual desktop: Win+Ctrl+Left/Right.
    // Mirrors mac's TouchSimulator postDockSwipeEventWithDelta for horizontal spaces swipe.
    match direction {
        SwipeDirection::Forward => crate::win::virtual_desktop::switch_virtual_desktop_right(),
        SwipeDirection::Back => crate::win::virtual_desktop::switch_virtual_desktop_left(),
    }
}

unsafe fn send_system_event(event_type: u32, _flags: u32) {
    match event_type {
        0 => send_symbolic_hotkey(0x09, 0x800),
        1 => send_symbolic_hotkey(0x09, 0x400),
        _ => {}
    }
}

/// Open Task View (Win+Tab) — virtual desktop overview / Timeline.
unsafe fn send_task_view() {
    let mut inputs: Vec<INPUT> = Vec::with_capacity(4);
    inputs.push(vk_input(0x5B, 0, true));  // VK_LWIN down
    inputs.push(vk_input(0x09, 0, true));  // VK_TAB down
    inputs.push(vk_input(0x09, 0, false)); // VK_TAB up
    inputs.push(vk_input(0x5B, 0, false)); // VK_LWIN up
    SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32);
}

/// Show Desktop / restore all (Win+D) — toggle minimize all windows.
unsafe fn send_show_desktop() {
    let mut inputs: Vec<INPUT> = Vec::with_capacity(4);
    inputs.push(vk_input(0x5B, 0, true));  // VK_LWIN down
    inputs.push(vk_input(0x44, 0, true));  // VK_D down
    inputs.push(vk_input(0x44, 0, false)); // VK_D up
    inputs.push(vk_input(0x5B, 0, false)); // VK_LWIN up
    SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32);
}

// ─── Mouse button synthesis ──────────────────────────────────────────────────

unsafe fn send_mouse_button(btn: MouseButton, down: bool) {
    let (flags, data) = match btn {
        MouseButton::Left => (if down { MOUSEEVENTF_LEFTDOWN } else { MOUSEEVENTF_LEFTUP }, 0),
        MouseButton::Right => (if down { MOUSEEVENTF_RIGHTDOWN } else { MOUSEEVENTF_RIGHTUP }, 0),
        MouseButton::Middle => (if down { MOUSEEVENTF_MIDDLEDOWN } else { MOUSEEVENTF_MIDDLEUP }, 0),
        MouseButton::X1 => (if down { MOUSEEVENTF_XDOWN } else { MOUSEEVENTF_XUP }, XBUTTON1),
        MouseButton::X2 => (if down { MOUSEEVENTF_XDOWN } else { MOUSEEVENTF_XUP }, XBUTTON2),
    };
    let mut input = INPUT { r#type: INPUT_MOUSE, ..std::mem::zeroed() };
    input.Anonymous.mi = MOUSEINPUT {
        dx: 0, dy: 0, mouseData: data as u32, dwFlags: flags, time: 0, dwExtraInfo: 0,
    };
    SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
}

// ─── Legacy simple remap (hooks.rs backward compatibility) ──────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonAction {
    Disabled,
    Button(MouseButton),
    Key(u32),
}

/// Synthesize the target event for a simple remap. `down` selects press vs release.
pub fn execute(action: ButtonAction, down: bool) {
    match action {
        ButtonAction::Disabled => {}
        ButtonAction::Button(b) => unsafe { send_mouse_button(b, down) },
        ButtonAction::Key(vk) => unsafe { send_key_simple(vk, down) },
    }
}

unsafe fn send_key_simple(vk: u32, down: bool) {
    let mut input = INPUT { r#type: INPUT_KEYBOARD, ..std::mem::zeroed() };
    input.Anonymous.ki = KEYBDINPUT {
        wVk: vk as u16, wScan: 0,
        dwFlags: if down { 0 } else { KEYEVENTF_KEYUP },
        time: 0, dwExtraInfo: 0,
    };
    SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
}

#[derive(Debug, Clone, Default)]
pub struct SimpleRemapTable {
    map: HashMap<MouseButton, ButtonAction>,
}

impl SimpleRemapTable {
    pub fn from_entries(entries: &[(MouseButton, ButtonAction)]) -> Self {
        let mut map = HashMap::new();
        for (src, action) in entries {
            map.insert(*src, *action);
        }
        Self { map }
    }

    pub fn lookup(&self, btn: MouseButton) -> Option<ButtonAction> {
        self.map.get(&btn).copied()
    }
}

// ─── RemapTable (complex, keyed by trigger + modifiers) ─────────────────────

#[derive(Debug, Clone, Default)]
pub struct RemapTable {
    entries: HashMap<u64, Vec<(Trigger, Effect)>>,
}

impl RemapTable {
    pub fn from_entries(entries: &[RemapEntry]) -> Self {
        let mut table = Self::default();
        for entry in entries {
            let key = modifier_key(&entry.modifiers);
            table.entries
                .entry(key)
                .or_default()
                .push((entry.trigger.clone(), entry.effect.clone()));
        }
        table
    }

    pub fn lookup(&self, trigger: &Trigger, active_mods: &ActiveModifiers) -> Vec<&Effect> {
        let mut results: Vec<&Effect> = Vec::new();
        let mut seen: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for key in modifier_keys_supersets(active_mods) {
            if let Some(entries) = self.entries.get(&key) {
                for (t, eff) in entries {
                    if t == trigger {
                        let ptr = eff as *const Effect as usize;
                        if seen.insert(ptr) {
                            results.push(eff);
                        }
                    }
                }
            }
        }
        results
    }
}

fn modifier_key(cond: &ModifierCondition) -> u64 {
    let btn_bits: u64 = cond
        .buttons
        .iter()
        .map(|b| match b {
            MouseButton::Left => 1 << 0,
            MouseButton::Right => 1 << 1,
            MouseButton::Middle => 1 << 2,
            MouseButton::X1 => 1 << 3,
            MouseButton::X2 => 1 << 4,
        })
        .fold(0u64, |acc, b| acc | b);
    ((cond.keyboard as u64) << 16) | btn_bits
}

fn modifier_keys_supersets(active: &ActiveModifiers) -> Vec<u64> {
    let mut keys = Vec::new();
    keys.push(modifier_key(&ModifierCondition {
        keyboard: active.keyboard,
        buttons: active.buttons.clone(),
    }));
    let mut kb = active.keyboard;
    while kb != 0 {
        keys.push(modifier_key(&ModifierCondition { keyboard: kb, buttons: vec![] }));
        kb &= kb - 1;
    }
    keys.push(0);
    keys
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifier_subset_empty() {
        let empty = ModifierCondition::default();
        let shift = ModifierCondition { keyboard: 0x200, ..Default::default() };
        assert!(empty.is_subset_of(&shift));
        assert!(!shift.is_subset_of(&empty));
    }

    #[test]
    fn active_modifiers_satisfies() {
        let required = ModifierCondition { keyboard: 0x200, ..Default::default() };
        let active = ActiveModifiers { keyboard: 0x200 | 0x100, buttons: vec![] };
        assert!(active.satisfies(&required));
    }

    #[test]
    fn click_cycle_single_click() {
        let tracker = ClickCycleTracker::new();
        assert!(!tracker.is_active(MouseButton::Right));
    }

    #[test]
    fn remap_table_lookup() {
        let entries = vec![RemapEntry {
            modifiers: ModifierCondition::default(),
            trigger: Trigger::Button { button: MouseButton::Right, level: 1, duration: ClickDuration::Click },
            effect: Effect::Disabled,
        }];
        let table = RemapTable::from_entries(&entries);
        let mods = ActiveModifiers::default();
        let results = table.lookup(
            &Trigger::Button { button: MouseButton::Right, level: 1, duration: ClickDuration::Click },
            &mods,
        );
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_active_modifiers_empty_satisfies_empty() {
        let active = ActiveModifiers::default();
        let required = ModifierCondition::default();
        assert!(active.satisfies(&required));
    }

    #[test]
    fn test_active_modifiers_ctrl_satisfies_ctrl() {
        // 0x100 = Ctrl bitmask (matches flags convention in symbolic_hotkey_inputs)
        let active = ActiveModifiers { keyboard: 0x100, buttons: vec![] };
        let required = ModifierCondition { keyboard: 0x100, buttons: vec![] };
        assert!(active.satisfies(&required));
    }

    #[test]
    fn test_active_modifiers_ctrl_does_not_satisfy_shift() {
        // 0x100 = Ctrl, 0x200 = Shift
        let active = ActiveModifiers { keyboard: 0x100, buttons: vec![] };
        let required = ModifierCondition { keyboard: 0x200, buttons: vec![] };
        assert!(!active.satisfies(&required));
    }
}
