# Changelog

All notable changes to **Win Mouse Fix** will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] — 2026-09-07

### Added

- **Smooth inertial scrolling** — Double-exponential (Holt) smoothing with Mac-matched momentum decay curve, subpixel accumulation, and a dedicated SendInput injection thread for zero-latency output
- **Button remapping** — Full RemapEngine with ClickCycleTracker; supports key combos, built-in actions (Task View, Show Desktop), and Shift/Ctrl modifier triggers
- **Click-drag gestures** — DragController with three modes: `move` (window drag), `scroll` (drag-to-scroll feeding the injection pipeline), `navigate` (back/forward). Shift and Ctrl modifiers dynamically switch scroll axis / precision mid-gesture
- **Pointer acceleration** — PointerAccel with configurable sensitivity range, integrated into the move pipeline via `send_mouse_move`
- **Modifier-key scroll combos** — Shift + scroll → horizontal, Ctrl + scroll → precision mode; implemented via ActiveModifiers + ModifiedScrollModification
- **Per-app profiles** — `[[profiles]]` table in config.toml with `match_exe` substring matching; profiles deep-merge over base config; foreground window polled every 500ms
- **Config hot-reload** — mtime polling on config.toml (1s interval via WM_TIMER); no restart needed
- **Device layer (HID++)** — Logitech HID++ 2.0 protocol (short/long report encode/decode), device enumeration via SetupAPI, battery level via 0x1000/0x1004 feature
- **Tray battery icon** — Dynamic HICON with GDI-rendered percentage overlay; updates every 5s; low-battery red warning; falls back to static icon on non-Logitech / error
- **DPI cross-monitor switching** — Per-monitor DPI via GetDpiForMonitor; automatic hardware DPI adjustment with formula `D_i = base_dpi × P_i / P_ref`; debounced on cursor monitor change
- **Phone trackpad server** — Embedded HTTP + hand-rolled WebSocket server (RFC 6455); LAN IP discovery; token-authenticated connections; QR code page; firewall rule setup
- **Phone trackpad PWA** — Full-screen trackpad web page with pointer events, multitouch gesture recognition (1-finger move/tap, 2-finger tap/scroll, 3-finger swipe), pinch-zoom disabled, iOS Safari compatibility
- **Voice input** — Web Speech API on phone → KEYEVENTF_UNICODE injection into PC; auto-start speech when input field is focused
- **Panic hook + crash log** — `std::panic::set_hook` writes stack info to `crash.log` next to the executable before aborting
- **Log rotation** — 2 MB × 3 rotating log files; readable timestamps; disabled by default
- **Exit confirmation dialog** — MessageBox YES/NO before quitting
- **Help dialog** — Tray menu → Help shows usage instructions in Chinese
- **G Hub integration** — Port polling (200ms interval, 15s timeout) to detect and wait for Logitech G Hub before connecting
- **Performance** — Hot paths: all log calls, mutex markers, and unnecessary lock acquisitions removed from scroll/wheel/move hot paths
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
