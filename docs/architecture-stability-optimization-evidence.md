# Architecture, stability, performance, and UX optimization evidence

## Conclusion

The optimization scope defined in `docs/architecture-stability-optimization-spec.md` is implemented and its six verification steps are now complete.

This does **not** mean the entire product roadmap is complete. Pointer acceleration is still not connected to `WM_MOUSEMOVE`; the standalone GUI is not exposed; Logitech battery/DPI behavior, physical mouse feel, firewall behavior, and phone-browser compatibility still require real hardware or device validation.

Assurance level: Tier 3. This was an autonomous implementation run; the specification did not receive independent pre-implementation approval.

## Baseline

- Branch: `audit/stability-ux-20260911`, created from the clean `main` worktree.
- Baseline command: `cargo test --all-targets`.
- Baseline: 192 passed, 5 ignored, 0 failed (`artifact://4`).
- No dependency crate was added. `Cargo.toml` only enables the cryptography API feature on the existing `windows-sys` dependency.

## Implementation coverage

| Failure model | Implemented change | Executable evidence |
|---|---|---|
| Scroll worker polled forever after disconnect | `src/scroll/injector.rs` blocks on `recv()` while idle, uses timed ticks only while coast is pending/active, and returns on every disconnect path. The channel was already bounded before this change. | `worker_exits_when_last_sender_is_dropped`; scroll injector suite |
| Per-wheel logging added latency and log churn | Removed physical-wheel/coast and window-switcher per-step logging. | Static hot-path inspection; full regression suite |
| Profile timer repeatedly did process/disk/hook work | `poll_foreground_profile` exits before process lookup when no enabled executable profile exists; unchanged effective input config avoids hook reinstall. | `hook_restart_filter_ignores_remote_only_changes` |
| Scroll was swallowed when the injector was unavailable | Hook swallows only after `try_send` succeeds; missing, full, or disconnected worker falls through to native input. | `smooth_scroll_only_swallows_an_accepted_event` |
| Ordinary left input was classified as foreign injection | Bypass now requires `LLMHF_INJECTED` or the private marker. | `injection_filter_keeps_unmarked_physical_events` |
| Config equality omitted button mappings | `Config`, `ButtonsConfig`, and remap entry types use complete derived equality. | `config_equality_detects_every_button_mapping_change` |
| Tray toggles flattened an active profile | Toggles load and save the base document, then resolve the active profile for runtime use. | Focused config/hook tests and source inspection |
| Config writes could be observed partially | Same-directory staging, `sync_all`, and `MoveFileExW(MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH)`. AddMode uses the same writer. | `atomic_config_replace_is_complete_and_failure_safe` |
| Mapping recorder/help were unreachable or misleading | Both tray entries are present. Recording works while features are disabled, captures the first trigger, passes physical input through, activates immediately, and can be cancelled. | AddMode suite, including `first_recorded_trigger_wins_until_disabled` |
| First AddMode append could duplicate `advanced = []` | Empty serialized assignment is removed before appending the first `[[buttons.advanced]]` table; final TOML is parsed before atomic replacement. | `recorded_mapping_roundtrips_button_modifiers` |
| Startup failures left an invisible/nonfunctional process | Tray and hook failures are visible and terminate startup; remote failure is visible while local mouse features remain available. | Source inspection and real startup smoke |
| Remote start returned before readiness | Listener binding and state publication complete before `start_server` returns; preferred port has bounded fallback. | `start_server_returns_ready_state_immediately` |
| Remote stop did not own every accepted connection | Generation token, accepted-socket registry, cancellation polling, and shutdown cover pre-auth and authenticated sockets. | `stop_server_closes_unauthenticated_connections`, `listener_stop_releases_bound_port` |
| Accepted sockets inherited nonblocking mode on Windows | Every accepted socket is restored to blocking mode before its worker starts; one-second read timeouts poll cancellation without changing the 30-second liveness-probe cadence. | 40-connection pressure test |
| Unauthenticated connection threads were unbounded | Atomic reservation caps accepted connections at 32. | `unauthenticated_connection_reservations_are_bounded`, `server_caps_and_closes_an_unauthenticated_connection_flood` |
| WebSocket writers could interleave | Authenticated client writes share one per-client mutex. | Remote suite and source inspection |
| Window callbacks performed socket writes | Status/window messages enter a generation-owned 256-item nonblocking queue; a dedicated broadcaster performs network I/O. | `outbound_broadcast_queue_is_bounded_and_fail_open` |
| Bearer generation was predictable and diagnostics leaked input/token material | Token uses `BCryptGenRandom`, is stored as versioned 64-character lowercase hex, and fails closed; raw header/input developer logging was removed. | `secure_token_has_expected_wire_format`, `persisted_token_requires_secure_format_version` |
| `/qr` exposed the bearer page to LAN peers | Route is limited to loopback or the listener host IP; other peers receive HTTP 403. | `bearer_qr_is_only_available_to_the_host_machine` |
| Thumbnail requests allowed unbounded threads and dimensions | Two concurrent jobs maximum, 1920×1080 clamp, null overload response, and generation check before broadcast. | `thumbnail_jobs_have_a_strict_concurrency_cap`, `thumbnail_dimensions_are_clamped_before_capture` |
| Synthetic multi-click slept inside `WH_MOUSE_LL` | Ordered `SendInput` down/up events are emitted without the 50 ms sleeps. | Static hot-path inspection; full remap regression suite |
| Alt+Tab wheel navigation spawned one thread per step | Short Tab/Shift+Tab/Alt-up/Escape sequences execute inline and in order; only the intentional 20 ms entry sequence uses one named worker. | Four window-switcher state tests and static thread inventory |
| Battery status was delayed and logged on every poll | Named background worker reads immediately, clamps a zero interval, then logs only state transitions/start failure. It never starts G Hub. | Source inspection and real startup smoke |

## Verification-plan results

### 1. Pre-fix RED and post-fix GREEN

- Scroll disconnect regression failed before the fix (`artifact://7`) and passed after it (`artifact://13`).
- Coast cancellation retained stale timing before the fix (`artifact://11`) and passed after reset logic (`artifact://13`).
- The connection-flood test exposed inherited nonblocking accepted sockets and delayed cancellation during development (`artifact://80`, `artifact://82`, `artifact://84`, `artifact://86`); it passed after both source fixes (`artifact://89`).

### 2. Focused suites

- Scroll injector: 9 passed (`artifact://62`).
- Hook behavior: 7 passed (`artifact://60`).
- Configuration: 18 passed (`artifact://66`).
- AddMode: 3 passed (`artifact://70`).
- Window switcher: 4 passed (`artifact://95`).
- Remote module before the final dimension-clamp case: 16 passed (`artifact://99`). The final all-target gate below includes that case.

### 3. Real binary smoke in an isolated runtime directory

- Built an isolated debug target and launched the actual `win-mouse-fix.exe` under the supervised process manager.
- Started with generated safe defaults: scroll off, buttons off, drag/acceleration/LAN off.
- Observed runtime log:

```text
Win Mouse Fix starting...
tray icon created
hooks installed (scroll.enabled=false, smooth=true, buttons.enabled=false)
Win Mouse Fix exited.
```

- Found the real message-only `WinMouseFixClass` window and posted `WM_CLOSE`.
- Process exited with code 0.
- A default `config.toml` was created beside the isolated executable; no `token.txt` was created, confirming the LAN server stayed disabled.
- The isolated smoke target was removed afterward.
- This proves startup, tray/message-window creation, hook installation, message-loop operation, logging, and orderly shutdown. It does not prove physical mouse feel or visually inspect the tray menu.

### 4. Final quality gates after the last source edit

```text
cargo fmt --all -- --check
# exit 0, no diff

cargo clippy --all-targets -- -D warnings
# exit 0, 0 warnings

cargo test --all-targets -- --test-threads=1
# 211 passed, 0 failed, 5 ignored; artifact://137
```

The release binary lock was resolved afterward: the old process had exited, the final release build was rebuilt directly into `target/release` (31 s, no lock), and the shipped binary is now the final version of this specification — SHA-256 `7ede274f38d97e7a63c99e95a99623ea79018f9899b24f534fff53060c43c3a0`, size 1,723,392 bytes, PE32+ GUI. The final binary was then launched as the resident process and re-verified in the live environment: `tray icon created`, `hooks installed (scroll.enabled=true, smooth=true, buttons.enabled=true)`, LAN trackpad ready on the published LAN address, and an orderly shutdown test (`remote: trackpad server stopped` → `Win Mouse Fix exited.` via WM_CLOSE to the tray window) passed before the final resident start. The previously isolated target used for the locked-build verification (`artifact://140`) was temporary and remains removed.

### 5. Deterministic ignored loopback checks

Run individually against the final WebSocket code:

- `ws_handshake_auth_status_and_reject`: passed (`artifact://127`).
- `ws_handshake_declines_permessage_deflate`: passed (`artifact://128`).
- `ws_handshake_accept_preserves_key_case`: passed (`artifact://129`).
- `ws_bad_token_clean_reject_not_1006`: passed (`artifact://130`).

The fifth ignored test, `ws_phone_trackpad_dispatch_e2e`, intentionally was not run because it mutates real Win32 global input state through `SendInput`.

### 6. Manual mutation controls

- Mutated the idle scroll disconnect branch from `return` to `continue`.
  - Expected failure: `worker_exits_when_last_sender_is_dropped` failed in 0.12 s (`artifact://116`).
  - Restored source: focused test passed (`artifact://118`).
- Mutated remote generation cancellation from `stop.store(true, ...)` to `false`.
  - Expected failure: `stop_server_closes_unauthenticated_connections` retained one connection and failed after 2.03 s (`artifact://120`).
  - Restored source: focused test passed (`artifact://122`).

These controls demonstrate that the two lifecycle tests fail when their defenses are removed rather than passing vacuously.

## Remaining product-level validation

These are roadmap gaps, not unfinished items in this optimization specification:

- Real mouse validation for scroll feel, left-button drag/remap, multi-click timing, and Alt+Tab navigation.
- CPU/wakeup and latency measurement on target hardware.
- Supported Logitech hardware validation for battery and HID++ DPI writes; tray/DPI timers remain disabled.
- Real firewall/UIPI behavior and multi-NIC/VPN address selection.
- iOS Safari and Android PWA compatibility, reconnect behavior, and multi-device use.
- The LAN transport is plain HTTP/WebSocket. The feature remains explicit opt-in and is suitable only for a trusted private LAN.
- Pointer-acceleration runtime integration, standalone GUI exposure, and offline Service Worker remain roadmap work.
