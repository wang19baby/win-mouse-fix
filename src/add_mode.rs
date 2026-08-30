//! AddMode — interactive remap learning.
//!
//! Flow:
//! 1. UI calls `AddMode::enable_add_mode()`.
//! 2. While active, every button/scroll/drag event that fires a remap
//!    is intercepted and its trigger info is stored in `ADD_MODE_PAYLOAD`.
//!    The event is then passed through unmodified.
//! 3. UI polls or is notified of the payload.
//! 4. UI calls `AddMode::get_payload()` to retrieve the captured trigger,
//!    then calls `AddMode::disable()` when done.

use crate::remap::{ActiveModifiers, Effect, MouseButton, RemapEntry, Trigger};
use std::sync::RwLock;

/// Payload captured during add mode.
#[derive(Debug, Clone)]
pub struct AddModePayload {
    /// Which button triggered this, if any.
    pub button: Option<MouseButton>,
    /// Click count at time of capture.
    pub click_count: u8,
    /// Whether the button was held down at time of capture.
    pub was_held: bool,
    /// Active modifiers at time of capture.
    pub active_mods: ActiveModifiers,
    /// Scroll/drag trigger was captured (vs button).
    pub scroll_captured: bool,
    /// Drag trigger was captured (vs button).
    pub drag_captured: bool,
}

impl Default for AddModePayload {
    fn default() -> Self {
        Self {
            button: None,
            click_count: 0,
            was_held: false,
            active_mods: ActiveModifiers { keyboard: 0, buttons: Vec::new() },
            scroll_captured: false,
            drag_captured: false,
        }
    }
}

/// Whether add mode is currently active.
static ADD_MODE_ACTIVE: RwLock<bool> = RwLock::new(false);

/// Captured payload during add mode.
static ADD_MODE_PAYLOAD: RwLock<AddModePayload> = RwLock::new(AddModePayload {
    button: None,
    click_count: 0,
    was_held: false,
    active_mods: ActiveModifiers { keyboard: 0, buttons: Vec::new() },
    scroll_captured: false,
    drag_captured: false,
});

/// Returns true if add mode is currently enabled.
pub fn is_active() -> bool {
    *ADD_MODE_ACTIVE.read().unwrap_or_else(|e| e.into_inner())
}

/// Enable add mode. Returns true if successful.
pub fn enable() -> bool {
    let mut guard = ADD_MODE_ACTIVE.write().unwrap_or_else(|e| e.into_inner());
    if *guard {
        return false; // already active
    }
    *guard = true;
    // Clear any stale payload.
    *ADD_MODE_PAYLOAD.write().unwrap_or_else(|e| e.into_inner()) =
        AddModePayload::default();
    true
}

/// Disable add mode. Returns the captured payload (if any).
pub fn disable() -> AddModePayload {
    {
        let mut guard = ADD_MODE_ACTIVE.write().unwrap_or_else(|e| e.into_inner());
        *guard = false;
    }
    ADD_MODE_PAYLOAD.write().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Consume a button event in add mode. Returns `true` if the event was
/// captured (caller should pass it through unmodified). Returns `false`
/// if add mode is not active and normal remapping should proceed.
pub fn on_button_event(
    button: MouseButton,
    click_count: u8,
    was_held: bool,
    active_mods: &ActiveModifiers,
) -> bool {
    if !is_active() {
        return false;
    }
    let mut payload = ADD_MODE_PAYLOAD.write().unwrap_or_else(|e| e.into_inner());
    *payload = AddModePayload {
        button: Some(button),
        click_count,
        was_held,
        active_mods: active_mods.clone(),
        scroll_captured: false,
        drag_captured: false,
    };
    true
}

/// Consume a scroll event in add mode.
pub fn on_scroll_event(active_mods: &ActiveModifiers) -> bool {
    if !is_active() {
        return false;
    }
    let mut payload = ADD_MODE_PAYLOAD.write().unwrap_or_else(|e| e.into_inner());
    *payload = AddModePayload {
        button: None,
        click_count: 0,
        was_held: false,
        active_mods: active_mods.clone(),
        scroll_captured: true,
        drag_captured: false,
    };
    true
}

/// Consume a drag event in add mode.
pub fn on_drag_event(active_mods: &ActiveModifiers) -> bool {
    if !is_active() {
        return false;
    }
    let mut payload = ADD_MODE_PAYLOAD.write().unwrap_or_else(|e| e.into_inner());
    *payload = AddModePayload {
        button: None,
        click_count: 0,
        was_held: false,
        active_mods: active_mods.clone(),
        scroll_captured: false,
        drag_captured: true,
    };
    true
}

/// Retrieve the current payload without disabling add mode.
pub fn get_payload() -> Option<AddModePayload> {
    let payload = ADD_MODE_PAYLOAD.read().unwrap_or_else(|e| e.into_inner());
    // Return None if nothing captured yet.
    if payload.button.is_none()
        && !payload.scroll_captured
        && !payload.drag_captured
    {
        None
    } else {
        Some(payload.clone())
    }
}

/// Build a `RemapEntry` from an add mode payload and a chosen effect.
/// Uses sensible defaults for level and duration.
pub fn build_remap_entry(
    payload: &AddModePayload,
    effect: Effect,
) -> RemapEntry {
    let trigger = if payload.scroll_captured {
        Trigger::Scroll
    } else if payload.drag_captured {
        Trigger::Drag
    } else {
        let (duration, level) = if payload.was_held {
            (crate::remap::ClickDuration::Hold, payload.click_count.max(1))
        } else {
            (crate::remap::ClickDuration::Click, payload.click_count.max(1))
        };
        Trigger::Button {
            button: payload.button.unwrap_or(MouseButton::Left),
            level,
            duration,
        }
    };

    RemapEntry {
        trigger,
        modifiers: crate::remap::ModifierCondition {
            keyboard: payload.active_mods.keyboard,
            buttons: payload.active_mods.buttons.clone(),
        },
        effect,
    }
}

/// Append a `RemapEntry` to `config.toml` as a new `[[buttons.advanced]]` section.
/// Returns `Ok(())` on success, or an error message on failure.
pub fn save_to_config(entry: &RemapEntry) -> Result<(), String> {
    use crate::config::config_path;

    let path = config_path();
    let mut content = std::fs::read_to_string(&path)
        .map_err(|e| format!("读取 config.toml 失败: {e}"))?;

    // Build TOML text for the new entry.
    let trigger_str = match &entry.trigger {
        Trigger::Button { button, level, duration } => {
            let btn = match button {
                MouseButton::Left => "left",
                MouseButton::Right => "right",
                MouseButton::Middle => "middle",
                MouseButton::X1 => "x1",
                MouseButton::X2 => "x2",
            };
            let dur = match duration {
                crate::remap::ClickDuration::Click => "click",
                crate::remap::ClickDuration::Hold => "hold",
            };
            format!("trigger = {{ type = \"button\", button = \"{}\", level = {}, duration = \"{}\" }}", btn, level, dur)
        }
        Trigger::Scroll => "trigger = { type = \"scroll\" }".to_string(),
        Trigger::Drag => "trigger = { type = \"drag\" }".to_string(),
    };

    let mod_str = if entry.modifiers.keyboard != 0 || !entry.modifiers.buttons.is_empty() {
        format!("\nmodifiers = {{ keyboard = 0x{:x} }}", entry.modifiers.keyboard)
    } else {
        String::new()
    };

    let effect_str = match &entry.effect {
        Effect::PassThrough => "effect = { type = \"pass_through\" }".to_string(),
        Effect::Disabled => "effect = { type = \"disabled\" }".to_string(),
        Effect::TaskView => "effect = { type = \"task_view\" }".to_string(),
        Effect::ShowDesktop => "effect = { type = \"show_desktop\" }".to_string(),
        Effect::NavigationSwipe { direction } => {
            let d = match direction {
                crate::remap::SwipeDirection::Back => "back",
                crate::remap::SwipeDirection::Forward => "forward",
            };
            format!("effect = {{ type = \"navigation_swipe\", data = {{ direction = \"{}\" }} }}", d)
        }
        Effect::SymbolicHotkey { keycode, flags } => {
            format!("effect = {{ type = \"symbolic_hotkey\", data = {{ keycode = {}, flags = {} }} }}", keycode, flags)
        }
        Effect::MouseButtonClicks { button, n_of_clicks } => {
            let btn = match button {
                MouseButton::Left => "left",
                MouseButton::Right => "right",
                MouseButton::Middle => "middle",
                MouseButton::X1 => "x1",
                MouseButton::X2 => "x2",
            };
            format!("effect = {{ type = \"mouse_button_clicks\", data = {{ button = \"{}\", n_of_clicks = {} }} }}", btn, n_of_clicks)
        }
        _ => "effect = { type = \"pass_through\" }".to_string(),
    };

    let new_section = format!("\n[[buttons.advanced]]\n{}\n{}\n{}\n", trigger_str, mod_str, effect_str);

    // Ensure trailing newline, then append.
    if !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(&new_section);

    std::fs::write(&path, &content)
        .map_err(|e| format!("写入 config.toml 失败: {e}"))?;

    crate::log::write(&format!("AddMode: appended entry to {}", path.display()));
    Ok(())
}
