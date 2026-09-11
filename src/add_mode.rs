//! AddMode — interactive remap learning.
//!
//! 1. The tray calls [`enable`].
//! 2. The first button-down or scroll event is recorded and passed through.
//! 3. The tray polls [`get_payload`] and asks the user to choose an effect.
//! 4. The tray calls [`disable`] after saving or cancellation.

use crate::remap::{ActiveModifiers, Effect, MouseButton, RemapEntry, Trigger};
use std::sync::atomic::{AtomicU8, Ordering};
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
            active_mods: ActiveModifiers {
                keyboard: 0,
                buttons: Vec::new(),
            },
            scroll_captured: false,
            drag_captured: false,
        }
    }
}

/// AddMode lifecycle: 0 = inactive, 1 = clearing stale payload, 2 = active.
static ADD_MODE_STATE: AtomicU8 = AtomicU8::new(0);

/// Captured payload during add mode.
static ADD_MODE_PAYLOAD: RwLock<AddModePayload> = RwLock::new(AddModePayload {
    button: None,
    click_count: 0,
    was_held: false,
    active_mods: ActiveModifiers {
        keyboard: 0,
        buttons: Vec::new(),
    },
    scroll_captured: false,
    drag_captured: false,
});

/// Returns true if add mode is currently enabled.
pub fn is_active() -> bool {
    ADD_MODE_STATE.load(Ordering::Acquire) == 2
}

/// Enable add mode. Returns true if successful.
pub fn enable() -> bool {
    if ADD_MODE_STATE
        .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return false;
    }
    // Publish the active state only after stale data is cleared.
    *ADD_MODE_PAYLOAD.write().unwrap_or_else(|e| e.into_inner()) = AddModePayload::default();
    ADD_MODE_STATE.store(2, Ordering::Release);
    true
}

/// Disable add mode. Returns the captured payload (if any).
pub fn disable() -> AddModePayload {
    ADD_MODE_STATE.store(0, Ordering::Release);
    ADD_MODE_PAYLOAD
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
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
    if payload.button.is_some() || payload.scroll_captured || payload.drag_captured {
        return true;
    }
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
    if payload.button.is_some() || payload.scroll_captured || payload.drag_captured {
        return true;
    }
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
#[allow(dead_code)]
pub fn on_drag_event(active_mods: &ActiveModifiers) -> bool {
    if !is_active() {
        return false;
    }
    let mut payload = ADD_MODE_PAYLOAD.write().unwrap_or_else(|e| e.into_inner());
    if payload.button.is_some() || payload.scroll_captured || payload.drag_captured {
        return true;
    }
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
    if payload.button.is_none() && !payload.scroll_captured && !payload.drag_captured {
        None
    } else {
        Some(payload.clone())
    }
}

/// Build a `RemapEntry` from an add mode payload and a chosen effect.
/// Uses sensible defaults for level and duration.
pub fn build_remap_entry(payload: &AddModePayload, effect: Effect) -> RemapEntry {
    let trigger = if payload.scroll_captured {
        Trigger::Scroll
    } else if payload.drag_captured {
        Trigger::Drag
    } else {
        let (duration, level) = if payload.was_held {
            (
                crate::remap::ClickDuration::Hold,
                payload.click_count.max(1),
            )
        } else {
            (
                crate::remap::ClickDuration::Click,
                payload.click_count.max(1),
            )
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

fn remove_empty_advanced_assignment(content: &str) -> String {
    let mut output = String::with_capacity(content.len());
    let mut in_buttons = false;
    for segment in content.split_inclusive('\n') {
        let line = segment.trim_end_matches(['\r', '\n']);
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_buttons = trimmed == "[buttons]";
        }
        let is_empty_advanced = in_buttons
            && trimmed
                .split('#')
                .next()
                .and_then(|statement| statement.split_once('='))
                .is_some_and(|(key, value)| key.trim() == "advanced" && value.trim() == "[]");
        if !is_empty_advanced {
            output.push_str(segment);
        }
    }
    output
}

/// Append a `RemapEntry` to `config.toml` as a new `[[buttons.advanced]]` section.
/// Returns `Ok(())` on success, or an error message on failure.
pub fn save_to_config(entry: &RemapEntry) -> Result<(), String> {
    use crate::config::config_path;

    let path = config_path();
    let mut content =
        std::fs::read_to_string(&path).map_err(|e| format!("读取 config.toml 失败: {e}"))?;
    // Serde emits `advanced = []` for an empty vector. Remove that assignment
    // before appending the first array-of-tables entry; otherwise TOML treats
    // `[[buttons.advanced]]` as a duplicate key.
    content = remove_empty_advanced_assignment(&content);

    // Build TOML text for the new entry.
    let trigger_str = match &entry.trigger {
        Trigger::Button {
            button,
            level,
            duration,
        } => {
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
            format!(
                "trigger = {{ type = \"button\", button = \"{}\", level = {}, duration = \"{}\" }}",
                btn, level, dur
            )
        }
        Trigger::Scroll => "trigger = { type = \"scroll\" }".to_string(),
        Trigger::Drag => "trigger = { type = \"drag\" }".to_string(),
    };

    let mod_str = if entry.modifiers.is_empty() {
        String::new()
    } else {
        let buttons = entry
            .modifiers
            .buttons
            .iter()
            .map(|button| match button {
                MouseButton::Left => "\"left\"",
                MouseButton::Right => "\"right\"",
                MouseButton::Middle => "\"middle\"",
                MouseButton::X1 => "\"x1\"",
                MouseButton::X2 => "\"x2\"",
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "\nmodifiers = {{ keyboard = 0x{:x}, buttons = [{}] }}",
            entry.modifiers.keyboard, buttons
        )
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
            format!(
                "effect = {{ type = \"navigation_swipe\", data = {{ direction = \"{}\" }} }}",
                d
            )
        }
        Effect::SymbolicHotkey { keycode, flags } => {
            format!(
                "effect = {{ type = \"symbolic_hotkey\", data = {{ keycode = {}, flags = {} }} }}",
                keycode, flags
            )
        }
        Effect::MouseButtonClicks {
            button,
            n_of_clicks,
        } => {
            let btn = match button {
                MouseButton::Left => "left",
                MouseButton::Right => "right",
                MouseButton::Middle => "middle",
                MouseButton::X1 => "x1",
                MouseButton::X2 => "x2",
            };
            format!("effect = {{ type = \"mouse_button_clicks\", data = {{ button = \"{}\", n_of_clicks = {} }} }}", btn, n_of_clicks)
        }
        _ => return Err("所选效果尚不支持写入 config.toml".to_string()),
    };

    let new_section = format!(
        "\n[[buttons.advanced]]\n{}\n{}\n{}\n",
        trigger_str, mod_str, effect_str
    );

    // Ensure trailing newline, then append.
    if !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(&new_section);
    toml::from_str::<crate::config::Config>(&content)
        .map_err(|e| format!("生成的映射配置无效,原文件未修改: {e}"))?;

    crate::config::atomic_write(&path, &content)
        .map_err(|e| format!("写入 config.toml 失败: {e}"))?;

    crate::log::write(&format!("AddMode: appended entry to {}", path.display()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn no_modifiers() -> ActiveModifiers {
        ActiveModifiers {
            keyboard: 0,
            buttons: Vec::new(),
        }
    }

    #[test]
    fn first_recorded_trigger_wins_until_disabled() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _ = disable();
        assert!(enable());
        assert!(is_active());

        assert!(on_button_event(MouseButton::X1, 1, false, &no_modifiers()));
        assert!(on_scroll_event(&no_modifiers()));

        let captured = get_payload().expect("first trigger must be available");
        assert_eq!(captured.button, Some(MouseButton::X1));
        assert!(!captured.scroll_captured);

        let disabled = disable();
        assert_eq!(disabled.button, Some(MouseButton::X1));
        assert!(!is_active());
    }

    #[test]
    fn recorded_button_defaults_to_click_not_hold() {
        let payload = AddModePayload {
            button: Some(MouseButton::Middle),
            click_count: 1,
            was_held: false,
            active_mods: no_modifiers(),
            scroll_captured: false,
            drag_captured: false,
        };
        let entry = build_remap_entry(&payload, Effect::PassThrough);
        assert!(matches!(
            entry.trigger,
            Trigger::Button {
                duration: crate::remap::ClickDuration::Click,
                ..
            }
        ));
    }

    #[test]
    fn recorded_mapping_roundtrips_button_modifiers() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let path = crate::config::config_path();
        let original = std::fs::read(&path).ok();
        let base = toml::to_string_pretty(&crate::config::Config::default()).unwrap();
        crate::config::atomic_write(&path, &base).unwrap();

        let entry = RemapEntry {
            modifiers: crate::remap::ModifierCondition {
                keyboard: 0x200,
                buttons: vec![MouseButton::X1],
            },
            trigger: Trigger::Button {
                button: MouseButton::Middle,
                level: 1,
                duration: crate::remap::ClickDuration::Click,
            },
            effect: Effect::TaskView,
        };
        let save_result = save_to_config(&entry);
        let parsed = std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|text| {
                toml::from_str::<crate::config::Config>(&text).map_err(|e| e.to_string())
            });

        match original {
            Some(contents) => crate::config::atomic_write(
                &path,
                std::str::from_utf8(&contents).expect("test config must be UTF-8"),
            )
            .unwrap(),
            None => {
                let _ = std::fs::remove_file(&path);
            }
        }

        save_result.unwrap();
        let parsed = parsed.unwrap();
        assert_eq!(parsed.buttons.advanced.last(), Some(&entry));
    }
}
