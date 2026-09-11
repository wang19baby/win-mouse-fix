# Win Mouse Fix

**Make your $10 mouse better than an Apple Trackpad.**

A Windows mouse enhancement tool written in Rust + native Win32 APIs — no Electron or .NET. The working runtime centers on smooth inertial scrolling, button remapping, drag gestures, per-app profiles, and an opt-in LAN phone trackpad.

Inspired by [Mac Mouse Fix](https://github.com/noah-nuebling/mac-mouse-fix), which defines the reference experience for this project on macOS.

---

## Features

### Smooth Inertial Scrolling
Double-exponential smoothing on wheel events with Mac-matched physics: momentum decay, subpixel precision, and zero-latency injection via a dedicated thread.

### Button Remapping
Remap any mouse button to keys, combos, or built-in actions (Task View, Show Desktop, etc.). Supports Shift/Ctrl modifier triggers and a recording mode accessible from the tray menu.

### Click-Drag Gestures
Hold a button and drag to trigger window move, scroll, or navigation gestures. Modifier keys (Shift/Ctrl) dynamically adjust behavior mid-gesture.

### Pointer Acceleration (engine preview)
The acceleration curve and state controller are implemented and unit-tested. Runtime hook integration is not enabled yet, so `accel.enabled` currently has no effect.

### Modifier-key Scroll Combos
- **Shift + scroll** → horizontal scroll
- **Ctrl + scroll** → precision mode (slow, cursor-centered)

### Per-App Profiles
Profiles auto-apply based on the foreground window's executable name. Each profile is a partial config that deep-merges over the base.

### Device Integration (experimental, Logitech)
- Battery and hardware-DPI query primitives are implemented via HID++
- Tray battery refresh and cross-monitor hardware-DPI switching remain disabled pending physical-device validation

### Phone Trackpad (preview)
Turn your phone into a wireless trackpad over LAN. The server is opt-in and token-authenticated:

| Gesture | Action |
|---|---|
| 1-finger move | Move cursor |
| 1-finger tap | Left click |
| 2-finger tap | Right click |
| 2-finger drag | Scroll |
| 3-finger swipe up | Task View |
| 3-finger swipe left/right | Switch desktop |
| Voice button | Voice-to-text input |

The web interface includes an installable manifest. Offline Service Worker support and broad iOS/Android device validation are still pending.

---

## Requirements

- **Windows 10/11** (64-bit)
- Standard desktop session for normal hooks; run elevated only when input must reach elevated applications
- For experimental device features: a supported Logitech mouse and receiver

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

# 3. Run normally; elevate only to affect elevated applications
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
enabled = false
button = "left"
mode = "move"   # "move" | "scroll" | "navigate"

[accel]
enabled = false  # engine exists; runtime hook integration is pending
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

1. Right-click the tray icon and select **手机妙控板**
2. Accept the one-time firewall prompt if the phone cannot reach the PC
3. Scan the displayed QR code with the phone camera
4. The tray toggle is persisted; `remote.enabled = true` in `config.toml` is an equivalent opt-in and hot-reloads without an app restart

> **iOS Safari:** tap the Share button → **Add to Home Screen** for true full-screen mode.

---

## Architecture

```
┌──────────────────────────────────────────────────────────────┐
│ WH_MOUSE_LL / WH_KEYBOARD_LL  (main-thread message loop)    │
└───────────────┬───────────────────┬──────────────────────────┘
                │ wheel             │ button / drag
                ▼                   ▼
┌──────────────────────────┐  ┌───────────────────────────────┐
│ bounded sync channel     │  │ RemapEngine / DragController  │
│ → scroll-injector thread │  │ → bounded Win32 actions       │
└───────────────┬──────────┘  └──────────────┬────────────────┘
                └──────────────┬──────────────┘
                               ▼
                    SendInput / window actions

LAN HTTP/WebSocket listener → capped connection workers → the same injection primitives
window/status callbacks → bounded broadcast queue → serialized per-client WebSocket writes
config/profile timers → atomic config replacement and idempotent hook reconfiguration
device polling worker → battery cache (experimental)
```

- Low-level hook callbacks avoid file/network I/O and blocking channel sends
- Scroll smoothing and LAN/device I/O run off the hook thread
- Remap and gesture decisions remain in the callback and must stay bounded
- The pointer-acceleration engine is not yet connected to the move callback

---

## Privacy

The desktop process has no telemetry, analytics, or remote crash-reporting service:

- Mouse and keyboard events are processed locally
- The optional phone trackpad binds to a LAN address and requires a 256-bit bearer token
- Pairing tokens and raw input payloads are not written to application logs
- The preview server uses plain HTTP/WebSocket, not TLS; enable it only on a trusted private LAN because a network observer can read its traffic
- Browser speech recognition is controlled by the phone browser and may use that browser vendor's online speech service

Run the desktop process elevated only if it must intercept or inject input into elevated applications.

---

## Related Projects

- [Mac Mouse Fix](https://github.com/noah-nuebling/mac-mouse-fix) — the inspiration for this project
- [OpenLogi](https://github.com/C-D-H-Dev/OpenLogi) — Logitech HID++ reference implementation in Rust

---

## License

MIT — see [LICENSE](LICENSE).
