# Win Mouse Fix

**Make your $10 mouse better than an Apple Trackpad.**

A Windows mouse enhancement tool written in Rust + native Win32 APIs — no Electron, no .NET, no bloat. Adds smooth inertial scrolling, button remapping, pointer acceleration, and more.

Inspired by [Mac Mouse Fix](https://github.com/benoitj/Trackpad++), which defines the gold standard for this kind of tool on macOS.

---

## Features

### Smooth Inertial Scrolling
Double-exponential smoothing on wheel events with Mac-matched physics: momentum decay, subpixel precision, and zero-latency injection via a dedicated thread.

### Button Remapping
Remap any mouse button to keys, combos, or built-in actions (Task View, Show Desktop, etc.). Supports Shift/Ctrl modifier triggers and a recording mode accessible from the tray menu.

### Click-Drag Gestures
Hold a button and drag to trigger window move, scroll, or navigation gestures. Modifier keys (Shift/Ctrl) dynamically adjust behavior mid-gesture.

### Pointer Acceleration
Subtle, predictable acceleration curve. Configurable sensitivity range.

### Modifier-key Scroll Combos
- **Shift + scroll** → horizontal scroll
- **Ctrl + scroll** → precision mode (slow, cursor-centered)

### Per-App Profiles
Profiles auto-apply based on the foreground window's executable name. Each profile is a partial config that deep-merges over the base.

### Device Integration (Logitech)
- Battery level in tray icon (updates every 5s)
- Hardware DPI query via HID++
- Automatic DPI adjustment across monitors of different densities

### Phone Trackpad (PWA)
Turn your phone into a wireless trackpad over LAN. Scan a QR code, connect, and your phone becomes a full multitouch input device:

| Gesture | Action |
|---|---|
| 1-finger move | Move cursor |
| 1-finger tap | Left click |
| 2-finger tap | Right click |
| 2-finger drag | Scroll |
| 3-finger swipe up | Task View |
| 3-finger swipe left/right | Switch desktop |
| 🎤 button | Voice-to-text input |

Built as a zero-install PWA. Works on iOS Safari (add to Home Screen for best experience).

---

## Requirements

- **Windows 10/11** (64-bit)
- **Administrator privileges** — required for global input hooks
- For device features: **Logitech mouse** connected via Unifying/Bolt receiver or Bluetooth

---

## Installation

### Option 1: Download Release (Recommended)

Download the latest `.zip` from [GitHub Releases](https://github.com/wang19baby/win-mouse-fix/releases), extract, and run `win-mouse-fix.exe`.

> **Note:** Windows Smart Screen may show a warning on first run because the binary is not code-signed. Click "More info → Run anyway" — the tool is open source and the behavior is fully auditable.

### Option 2: Build from Source

```bash
# 1. Install Rust (rustup.msi from rustup.rs)
# 2. Clone and build
git clone https://github.com/wang19baby/win-mouse-fix.git
cd win-mouse-fix
cargo build --release

# 3. Run (requires admin)
./target/release/win-mouse-fix.exe
```

### Option 3: via Cargo

```bash
cargo install --git https://github.com/wang19baby/win-mouse-fix.git
```

---

## Configuration

All settings live in `config.toml` next to the executable. The file is created with defaults on first run.

### Quick Reference

```toml
[scroll]
enabled = true
smooth = true
speed = 1.0
invert = false

[buttons]
enabled = true

[drag]
enabled = true
button = "left"
mode = "move"   # "move" | "scroll" | "navigate"

[accel]
enabled = true
sensitivity = 1000.0

[touch]
gain = 6.7
scroll_gain = 5.5
tap_ms = 220

[remote]
enabled = false   # set to true to enable phone trackpad
port = 18765
```

### Per-App Profiles

```toml
[[profiles]]
match_exe = "code.exe"
config = { scroll = { speed = 1.5 } }

[[profiles]]
match_exe = "firefox.exe"
config = { scroll = { smooth = false } }
```

---

## Phone Trackpad Setup

1. Enable `[remote]` in `config.toml` (set `enabled = true`)
2. Restart the app
3. Right-click the tray icon → **Phone Trackpad** → scan the QR code with your phone camera
4. Your phone opens a full-screen trackpad — no app install needed

> **iOS Safari:** tap the Share button → **Add to Home Screen** for true full-screen mode.

---

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│  WH_MOUSE_LL / WH_KEYBOARD_LL  (hooks.rs)                  │
│  Low-level input interception — must return fast             │
└──────────────┬──────────────────────────────────────────────┘
               │
               ▼
┌──────────────────────────────────────────────────────────────┐
│  Scroll pipeline    Button pipeline    Accel pipeline        │
│  wheel_tracker ──►  RemapEngine ──►  PointerAccel ──►      │
│  smoother ───────►  ClickCycle ──►                          │
│  inject thread ◄───  execute_effect ──►                     │
└──────────────┬──────────────────┬─────────────────────────┘
               │                  │
               ▼                  ▼
┌─────────────────────────────┬──────────────────────────────┐
│  SendInput injection thread │  Device layer (Logitech HID++)│
│  (wheel / button / move)   │  battery / dpi               │
└─────────────────────────────┴──────────────────────────────┘
```

- Hook callbacks run on a system thread and must return in < 1ms
- All heavy computation (smoothing, remapping, acceleration) happens on worker threads
- The device layer (HID++) is independent and only active for Logitech hardware

---

## Privacy

**Win Mouse Fix never connects to the internet.** All processing is local:

- No telemetry, no analytics, no crash reporting services
- Your keystrokes and mouse data are never collected or transmitted
- The phone trackpad server binds to your LAN IP only and requires a token to connect

The app does request administrator privileges to install global input hooks. This is unavoidable for the functionality it provides.

---

## Related Projects

- [Mac Mouse Fix](https://github.com/benoitj/Trackpad++) — the inspiration for this project
- [OpenLogi](https://github.com/C-D-H-Dev/OpenLogi) — Logitech HID++ reference implementation in Rust

---

## License

MIT — see [LICENSE](LICENSE).
