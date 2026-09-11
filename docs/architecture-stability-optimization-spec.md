# Architecture, stability, performance, and UX optimization spec

## Assurance level

Tier 3: the affected paths include global input-hook timing, worker-thread lifetime, and an authenticated LAN input server. This is an autonomous run; spec approval was not obtained before implementation, so the final evidence must state that limitation.

## Baseline

- Source branch: `audit/stability-ux-20260911`, created from the clean `main` worktree.
- Baseline command: `cargo test --all-targets`.
- Baseline result: 192 passed across 5 suites, 5 ignored, 0 failed.
- No dependency crate may be added. Enabling an additional API feature on the existing `windows-sys` dependency is allowed only if needed for Windows system randomness.

## Failure model

1. Dropping the smooth-scroll sender currently leaves a detached worker waking every 8 ms forever. Repeated config/profile application can therefore accumulate threads and CPU wakeups, and an obsolete worker can continue coasting after the feature is disabled.
2. Every physical wheel tick currently formats, timestamps, flushes, and metadata-checks a log record on the injector thread. This can add input latency and creates rapid log rotation.
3. Foreground application polling currently reloads the file, tears down hooks, and starts a new injector whenever the executable changes, even with no executable profiles or when the effective config is unchanged.
4. The phone-trackpad start path returns before readiness but the tray immediately asks for connection info, creating a false “startup failed” dialog. Its stop path drops no listener owned by the accept loop, so the port and remote input can remain live after the UI says it stopped.
5. Authenticated socket clones remain registered after normal disconnect and are not closed by server stop. Concurrent or stale writers can retain handles and continue status traffic.
6. Remote diagnostics currently write raw HTTP headers and input payloads, including bearer material or typed text, to a hard-coded developer path. This violates the documented local/private behavior and adds per-frame file I/O.
7. The pairing token claims 256-bit strength but is produced by a time/PID-seeded xorshift state. A LAN input authorization token must use the Windows system RNG or fail closed.
8. Add-mode reports that a restart is required even though hot reload exists, leaving the user uncertain about whether a new mapping is active.
9. The mouse hook classifies every unmarked left-button event as a foreign synthetic event. Normal physical left clicks therefore bypass configured remaps and the shipped left-button drag trigger.
10. Configuration equality ignores remap tables and the window-switcher flag, so an idempotence guard can incorrectly skip real mapping changes. Tray toggles also save the active profile overlay as the base config, flattening profile-specific values.
11. Config writes and AddMode appends replace the live TOML with ordinary writes, allowing the reload timer or a failed write to observe/truncate partial content.
12. Mapping recorder/help commands exist but are absent from the `TPM_RETURNCMD` dispatch menu; capture is also gated by already-enabled scroll/remap features and incorrectly swallows the input it promises to pass through.
13. Hook and tray startup failures are logged but leave an invisible or nonfunctional process running.
14. Pre-authentication sockets are not registered for shutdown and connection threads have no upper bound, so stop can leave a late-authenticating input path and unauthenticated LAN traffic can exhaust threads.
15. WebSocket responses and broadcasts write through independent cloned handles without a shared serializer, so frames can interleave. The unauthenticated `/qr` HTTP route also exposes the bearer token to any LAN peer.
16. Smooth mode swallows a physical wheel event even if the injector never started, disconnected, or rejected the event, turning a background failure into complete loss of scrolling.
17. Battery polling waits a full interval before the first read and logs every routine attempt/result, delaying phone status while creating repetitive log I/O.
18. The production listener is nonblocking; on Windows its accepted sockets can remain nonblocking, causing the long-lived WebSocket reader to interpret `WouldBlock` as disconnect immediately after handshake.
19. Window-event callbacks synchronously write to every authenticated phone client. One slow LAN client can therefore stall the callback, and concurrent control/broadcast writes can interleave frames.
20. Mouse-button click effects sleep for 100 ms per synthetic click inside `WH_MOUSE_LL`, while each window-switcher wheel step spawns another thread. Both risk hook removal, latency, thread bursts, and out-of-order selection.
21. Authenticated `thumb_request` messages spawn unbounded capture threads and accept attacker-sized bitmap dimensions, allowing a stale or compromised client to exhaust threads/GDI memory.

## Executable acceptance scenarios

### Scroll worker lifecycle

- Given a smooth-scroll worker whose final sender is dropped, the worker exits promptly without another event; a regression test must observe completion within a bounded timeout.
- Given a live but idle worker with no pending coast, it blocks waiting for input rather than polling every 8 ms.
- Given the existing wheel/coast test vectors, emitted distances, direction handling, subpixel behavior, and configured curve behavior remain unchanged.
- Production wheel handling performs no synchronous per-wheel log write or formatting.
- A physical wheel event is swallowed only after the injector accepts it; a missing, disconnected, or saturated worker fails open to native scrolling.

### Hook callback latency

- Mouse-button click effects enqueue ordered `SendInput` down/up events without sleeping in the low-level hook.
- Window-switcher wheel steps execute short ordered `SendInput` sequences inline instead of spawning one thread per wheel event; only the intentional 20 ms switcher-entry sequence uses a named worker.

### Profile application

- Given no enabled executable profiles, the 500 ms profile timer performs no foreground-process lookup and no config/hook reapplication.
- Given an executable transition that resolves to the already-active config, hooks and the injector are not restarted.
- Given a transition to a different effective profile, the new effective config is still applied.

### Remote lifecycle and privacy

- A successful `start_server` call has bound a listener and published connection info before returning, so the tray can immediately open the QR page; the configured preferred port is tried first with bounded fallback.
- A failed start returns a concrete error and leaves no “running” state.
- Stopping invalidates the exact listener generation, closes every accepted socket (including pre-authentication clients), clears connection info, and lets the bound port be reused within a bounded timeout. Connection readers poll cancellation at one-second intervals because Windows does not reliably interrupt a synchronous receive through a duplicated socket.
- Accepted sockets are explicitly restored to blocking mode, connection workers are capped at 32, and a 40-connection pre-authentication flood remains capped and shuts down within two seconds.
- A normally disconnected authenticated client is removed from the active-client registry.
- All post-authentication WebSocket writes for a client share one mutex, preventing broadcast/control-frame interleaving.
- Window/status broadcasts enter a generation-owned 256-item nonblocking queue; system callbacks never perform socket writes and overload drops best-effort updates instead of blocking input/window handling.
- The bearer-bearing `/qr` page is served only when the peer is the host machine; other LAN peers receive HTTP 403.
- Thumbnail capture is limited to two concurrent jobs, dimensions are clamped to 1920×1080, overload receives a null best-effort update, and an old generation cannot broadcast a completed capture into a restarted server.
- WebSocket authentication, status, input dispatch, HTTP assets, ping/pong, and rejection behavior continue to satisfy existing tests. Authentication sends one initial window-list snapshot, not duplicate snapshots.
- Pairing tokens remain 64 lowercase hexadecimal characters and are sourced from `BCryptGenRandom`; RNG failure prevents server startup instead of falling back to predictable bytes.
- No production or debug path writes raw request headers, authentication data, or input payloads to a hard-coded file. The literal developer `ws_debug.log` path is absent from production source.

### Configuration and tray UX

- Physical unmarked left-button input remains eligible for remap and drag processing; only `LLMHF_INJECTED` or the private marker bypasses the hook.
- Full config equality detects window-switcher, legacy-remap, and advanced-remap changes.
- Non-input-only config changes update runtime state without reinstalling low-level hooks, while changed scroll/button/drag settings still rebuild the input pipeline.
- Base config toggles never serialize an active per-app overlay into global settings.
- Config replacement and AddMode appends are same-directory atomic operations; a staging failure preserves the previous valid file.
- Mapping recording and help are reachable from the tray. Recording works when features are disabled, captures the first button/scroll trigger, passes the physical input through, can be explicitly cancelled from the tray, saves atomically, and reports immediate activation failures accurately.
- Battery discovery remains off the hook thread, populates the cache immediately, clamps a zero interval, and logs only state changes or worker-start failure.
- Tray creation, hook installation, remote startup, toggle persistence, and mapping activation failures produce a visible or logged concrete error rather than silently continuing.

## Invariants

- Existing config schema remains compatible. The shipped sample changes only risky optional enable flags (`drag`, unfinished `accel`, and LAN `remote`) to explicit opt-in.
- No wheel curve, remap matching rule, gesture threshold, SendInput marker, or public network message shape is changed except removal of the redundant duplicate initial window-list frame and restoring physical left-button eligibility.
- Low-level hook callbacks never block on file or network I/O introduced by this change.
- No new background loop is unbounded without an explicit stop condition.
- Existing ignored hardware/interactive tests stay ignored; no assertion is weakened or deleted.

## Planned files

- `src/scroll/injector.rs`
- `src/win/hooks.rs`, `src/win/window_switcher.rs`, `src/win/tray.rs`
- `src/remote.rs`, `src/main.rs`
- `src/config.rs`, `src/add_mode.rs`, `src/remap/mod.rs`
- `src/device/battery.rs`
- `config.toml`
- `Cargo.toml` for the existing `windows-sys` cryptography feature
- Existing milestone/user documentation where verified claims are stale
- `docs/architecture-stability-optimization-evidence.md`

## Verification plan

1. Observe focused regression tests fail against the pre-fix behavior where practical, then pass after the fix.
2. Run focused scroll, profile/config, remote lifecycle, WebSocket, and tray/add-mode tests.
3. Run the real binary briefly with an isolated temporary runtime directory/config; verify tray/message-loop startup, log behavior, and clean process termination without enabling remote access or exercising physical input.
4. Run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test --all-targets` after the final source edit.
5. Run ignored loopback WebSocket tests that are deterministic on this workstation; record hardware/interactive checks that cannot be automated.
6. Perform manual mutation controls for the worker-disconnect and remote-stop defenses, restoring each mutation and rerunning its focused test.
