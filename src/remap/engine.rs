//! Remap engine: matches active modifiers against the remap table
//! and resolves effects for a given trigger.
//!
//! Mirrors mac-mouse-fix's `RemapSwizzler.swift` + `RemapsAnalyzer.swift`.

use super::{
    ActionPhase, ActiveModifiers, ClickDuration, Effect, MouseButton, RemapEntry, RemapTable,
    Trigger,
};
use parking_lot::Mutex;
use std::collections::HashMap;

/// The remap engine — holds the compiled remap table and resolves effects.
#[derive(Debug)]
pub struct RemapEngine {
    table: RemapTable,
    /// Entries in declaration order (for priority when same modifier subset).
    entries: Vec<RemapEntry>,
    /// Cache: (keyboard_mask << 16 | button_bits) → Vec<Effect> for Trigger::Scroll.
    scroll_cache: Mutex<HashMap<u64, Vec<Effect>>>,
    /// Cache: same key → Vec<Effect> for Trigger::Drag.
    #[allow(dead_code)]
    drag_cache: Mutex<HashMap<u64, Vec<Effect>>>,
}

impl RemapEngine {
    pub fn new(entries: Vec<RemapEntry>) -> Self {
        let table = RemapTable::from_entries(&entries);
        Self {
            table,
            entries,
            scroll_cache: Mutex::new(HashMap::new()),
            drag_cache: Mutex::new(HashMap::new()),
        }
    }

    /// Reload with new entries.
    #[allow(dead_code)]
    pub fn reload(&mut self, entries: Vec<RemapEntry>) {
        self.table = RemapTable::from_entries(&entries);
        self.entries = entries;
        self.scroll_cache.lock().clear();
        self.drag_cache.lock().clear();
    }

    /// Cache key from active modifiers: (keyboard << 16) | button_bits.
    fn modifier_cache_key(active_mods: &ActiveModifiers) -> u64 {
        let kb = active_mods.keyboard as u64;
        let btn_bits = active_mods.buttons.iter().fold(0u64, |acc, b| {
            acc | (match b {
                MouseButton::Left => 1u64 << 0,
                MouseButton::Right => 1u64 << 1,
                MouseButton::Middle => 1u64 << 2,
                MouseButton::X1 => 1u64 << 3,
                MouseButton::X2 => 1u64 << 4,
            })
        });
        (kb << 16) | btn_bits
    }

    /// Resolve effects for a scroll event with the given active modifiers.
    pub fn resolve_scroll_effects(&self, active_mods: &ActiveModifiers) -> Vec<Effect> {
        let key = Self::modifier_cache_key(active_mods);
        if let Some(cached) = self.scroll_cache.lock().get(&key) {
            return cached.clone();
        }
        let results = self.table.lookup(&Trigger::Scroll, active_mods);
        let owned: Vec<_> = results.into_iter().cloned().collect();
        self.scroll_cache.lock().insert(key, owned.clone());
        owned
    }

    /// Resolve effects for a drag event.
    #[allow(dead_code)]
    pub fn resolve_drag_effects(&self, active_mods: &ActiveModifiers) -> Vec<Effect> {
        let key = Self::modifier_cache_key(active_mods);
        if let Some(cached) = self.drag_cache.lock().get(&key) {
            return cached.clone();
        }
        let results = self.table.lookup(&Trigger::Drag, active_mods);
        let owned: Vec<_> = results.into_iter().cloned().collect();
        self.drag_cache.lock().insert(key, owned.clone());
        owned
    }

    /// Resolve effects for a button at a given click count and hold state.
    /// Returns (effect, phase) pairs to execute.
    pub fn resolve_effects(
        &self,
        button: MouseButton,
        click_count: u8,
        already_held: bool,
        active_mods: &ActiveModifiers,
    ) -> Vec<(Effect, ActionPhase)> {
        let mut results = Vec::new();

        // Try each entry; find first matching with best modifier specificity.
        for entry in &self.entries {
            // Check trigger match.
            let trigger_level = match &entry.trigger {
                Trigger::Button {
                    button: b,
                    level,
                    duration,
                } if *b == button => Some((*level, *duration)),
                _ => None,
            };

            let Some((level, duration)) = trigger_level else {
                continue;
            };

            // Check modifier match — entry.modifiers must be subset of active_mods.
            if !active_mods.satisfies(&entry.modifiers) {
                continue;
            }

            // Match click count (level).
            if click_count < level {
                continue;
            }

            // Determine action phase.
            let phase = if already_held {
                ActionPhase::End
            } else if matches!(duration, ClickDuration::Hold) {
                ActionPhase::Start
            } else {
                ActionPhase::Combined
            };

            results.push((entry.effect.clone(), phase));
        }

        results
    }

    /// Get max click level for a button (for ClickCycle configuration).
    pub fn max_level_for_button(&self, button: MouseButton) -> u8 {
        let mut max = 0u8;
        for entry in &self.entries {
            if let Trigger::Button {
                button: b, level, ..
            } = &entry.trigger
            {
                if *b == button {
                    max = max.max(*level);
                }
            }
        }
        max
    }

    /// Returns true if any entry modifies scroll.
    #[allow(dead_code)]
    pub fn modifies_scroll(&self) -> bool {
        self.entries
            .iter()
            .any(|e| matches!(e.trigger, Trigger::Scroll))
    }

    /// Returns true if any entry modifies pointing.
    #[allow(dead_code)]
    pub fn modifies_pointing(&self) -> bool {
        self.entries
            .iter()
            .any(|e| matches!(e.trigger, Trigger::Drag))
    }
}
