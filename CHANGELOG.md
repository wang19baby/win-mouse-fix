# Changelog

All notable changes to **Win Mouse Fix** will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased] — 2026-09-11

### Changed

- Scroll injection workers now block while idle and terminate when their last sender is dropped; configuration/profile changes no longer leak polling threads, and unavailable/overloaded workers fail open to native scrolling
- Foreground-profile polling skips process and disk work when no executable profile exists, and runtime-only configuration changes no longer reinstall input hooks
- Synthetic multi-click remaps no longer sleep inside the low-level hook, and Alt+Tab wheel navigation no longer spawns one thread per step
- Configuration writes use same-directory atomic replacement; tray feature toggles preserve profile overlays instead of flattening them into the base document
- Phone-trackpad startup now binds before returning, restores accepted sockets to blocking mode, honors the configured preferred port with bounded fallback, supports live config enable/disable, caps accepted connections at 32, and shuts down listeners plus every accepted socket
- Phone thumbnail requests are clamped to 1920×1080 and limited to two concurrent capture jobs
- Phone pairing tokens now use the Windows system CSPRNG; legacy time-seeded token files rotate once to the versioned secure format
- Optional drag, pointer-acceleration, and LAN-server settings ship disabled until explicitly enabled or fully validated
- Battery discovery now runs immediately off-thread and logs state transitions instead of every poll

### Fixed

- Removed a left-button “foreign injection” heuristic that classified ordinary physical clicks as synthetic and bypassed legitimate remaps/drag gestures
- Removed per-wheel and raw WebSocket payload logging, including bearer-token exposure
- Restored the tray entries for mapping recording and help; mapping capture now works even when remapping/scrolling are disabled, leaves the physical input untouched, and can be cancelled from the tray
- Hook, tray, remote-server, config-save, and immediate mapping-activation failures now produce actionable errors instead of silently continuing
- Remote authentication no longer sends the initial window list twice, disconnected clients leave the broadcast registry, per-client writes are serialized, window-event broadcasts use a bounded off-callback queue, and the bearer-bearing QR page is restricted to the host machine

## [0.1.0] — 2026-09-07

### Added

- **Smooth inertial scrolling** — Double-exponential (Holt) smoothing with Mac-matched momentum decay curve, subpixel accumulation, and a dedicated SendInput injection thread for zero-latency output
- **Button remapping** — Full RemapEngine with ClickCycleTracker; supports key combos, built-in actions (Task View, Show Desktop), and Shift/Ctrl modifier triggers
- **Click-drag gestures** — DragController with three modes: `move` (window drag), `scroll` (drag-to-scroll feeding the injection pipeline), `navigate` (back/forward). Shift and Ctrl modifiers dynamically switch scroll axis / precision mid-gesture
- **Pointer acceleration prototype** — Configurable curve and state controller with unit tests; runtime move-hook integration remained pending
- **Modifier-key scroll combos** — Shift + scroll → horizontal, Ctrl + scroll → precision mode; implemented via ActiveModifiers + ModifiedScrollModification
- **Per-app profiles** — `[[profiles]]` table in config.toml with `match_exe` substring matching; profiles deep-merge over base config; foreground window polled every 500ms
- **Config hot-reload** — mtime polling on config.toml (1s interval via WM_TIMER); no restart needed
- **Device layer (HID++)** — Logitech HID++ 2.0 protocol (short/long report encode/decode), device enumeration via SetupAPI, battery level via 0x1000/0x1004 feature
- **Device telemetry primitives** — Battery/DPI query and icon/switching code added; tray refresh, cross-monitor polling, and hardware write validation remained pending
- **Phone trackpad server** — Embedded HTTP + hand-rolled WebSocket server (RFC 6455); LAN IP discovery; token-authenticated connections; QR code page; firewall rule setup
- **Phone trackpad PWA** — Full-screen trackpad web page with pointer events, multitouch gesture recognition (1-finger move/tap, 2-finger tap/scroll, 3-finger swipe), pinch-zoom disabled, iOS Safari compatibility
- **Voice input** — Web Speech API on phone → KEYEVENTF_UNICODE injection into PC; auto-start speech when input field is focused
- **Panic hook + crash log** — `std::panic::set_hook` writes stack info to `crash.log` next to the executable before aborting
- **Log rotation** — 2 MB × 3 rotating log files; readable timestamps; disabled by default
- **Exit confirmation dialog** — MessageBox YES/NO before quitting
- **Help dialog** — Tray menu → Help shows usage instructions in Chinese
- **G Hub integration** — Local port availability check and cached device queries; the app intentionally does not auto-start G Hub
- **Performance groundwork** — Bounded scroll channel and atomic hook flags; later hot-path/lifecycle corrections are recorded under Unreleased
- **Compilation** — 70 compiler warnings → 0 warnings; opt-level 3 + LTO + panic=abort in release

### Changed

- `config.toml` defaults: all features enabled (`scroll.enabled`, `buttons.enabled`, `drag.enabled`, `accel.enabled`); phone trackpad disabled by default (`remote.enabled = false`)
- Battery poll interval: 20s (bg thread) / 5s (tray icon refresh)
- Profile switch poll: 500ms
- DPI switch poll: 250ms

### Fixed

- Scroll injector no longer drops events during high-frequency wheel bursts
- LLMHF_INJECTED guard prevents feedback loops when SendInput reinjects its own events
- Config hot-reload correctly re-applies hooks after file change

---

## [0.0.0] — 2026-08-01

### Added

- Initial scaffold: main.rs entry, config.rs, log.rs, win/tray.rs, win/hooks.rs, win/message_loop.rs
- Cargo.toml with all dependencies pinned
