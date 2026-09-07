# Security Policy

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| 0.1.x   | :white_check_mark: |

## Reporting a Vulnerability

**Do not open a public issue for security vulnerabilities.**

If you find a security issue, please report it by emailing `security@aiotdl.com`.

Response timeline:
- Acknowledgment: within 48 hours
- Initial assessment: within 7 days
- Fix timeline: varies by severity; critical issues addressed as quickly as possible

Please include:
- Description of the vulnerability
- Steps to reproduce
- Potential impact
- Whether you're comfortable with public credit for the report

---

## What This Tool Does with Your Data

Win Mouse Fix is a **local-only** input processing tool. It:

- **Never connects to the internet** (except the optional phone-trackpad LAN server, which you explicitly enable)
- **Never transmits keystrokes or mouse data** to any remote server
- **Never collects telemetry or analytics**
- **Never updates automatically** (you must download new versions manually)
- All processing happens on your machine, in memory

### Input Data

The tool intercepts mouse wheel, button, and movement events via Windows low-level hooks (`WH_MOUSE_LL`, `WH_KEYBOARD_LL`). This interception is necessary for smooth scrolling, remapping, and acceleration. The raw event data is processed locally and then either:
- Forwarded (possibly modified) to the OS via `SendInput`
- Used to trigger internal actions (key combos, navigation gestures)

No event data is stored, logged, or transmitted beyond the immediate processing pipeline.

### Phone Trackpad

When `[remote].enabled = true`, the app starts a local HTTP/WebSocket server on your LAN IP. This lets your phone act as a trackpad. The server:
- Binds to your LAN IP only (not 0.0.0.0)
- Requires a 256-bit token (shown as a QR code) for authentication
- Token is stored in `token.txt` next to the executable and changes on each app start
- Is intended for use on trusted home networks only

**Do not enable the phone trackpad on untrusted networks.**

### Permissions

The app requires **Administrator** privileges because:
- `WH_MOUSE_LL` and `WH_KEYBOARD_LL` global hooks require elevated privileges on Windows
- The phone trackpad server may need to add a firewall rule

The admin privilege request is for hook installation only. The app does not make any other system changes.

---

## Security Best Practices for Users

1. **Keep `config.toml` private** — it may contain custom key bindings
2. **Don't run untrusted configs** — only load config files from sources you trust
3. **Disable phone trackpad when not in use** — set `remote.enabled = false` in config.toml
4. **Download from official releases only** — verify the SHA-256 hash of any downloaded binary

---

## Known Limitations

- The app runs with administrator privileges. Only install builds from sources you trust.
- The phone trackpad server has no TLS (it runs on a LAN IP only). This is a deliberate tradeoff to avoid certificate management complexity.
- The app does not currently support code signing (Windows SmartScreen may show a warning on first run).

---

## Credit

Vulnerability reporters will be credited in the release notes (unless they request anonymity).
