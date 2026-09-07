# Contributing to Win Mouse Fix

Thank you for your interest in contributing!

---

## Development Setup

### Prerequisites

- **Rust 1.75+** — Install via [rustup](https://rustup.rs)
- **Windows 10/11 (64-bit)** — The project uses `windows-sys` and Win32 APIs, so cross-compilation is not supported
- **Administrator privileges** — Required at runtime for global input hooks; also needed to run some tests

### Build

```bash
# Clone
git clone https://github.com/wang19baby/win-mouse-fix.git
cd win-mouse-fix

# Build (debug)
cargo build

# Build (release)
cargo build --release

# Run (requires admin)
./target/debug/win-mouse-fix.exe
# or
./target/release/win-mouse-fix.exe
```

### Run Tests

```bash
cargo test
```

Some tests are integration-style and depend on Windows APIs. They run in the same environment as the build.

### Code Quality

```bash
# Format
cargo fmt

# Lint
cargo clippy
```

We target zero warnings on the `main` branch.

---

## Branch Model

```
main          — stable, always releasable
dev           — integration branch for the next release
feat/<name>   — feature branches, PR → dev
```

Work on feature branches and open a Pull Request against `dev`. Direct pushes to `main` are restricted.

---

## Pull Request Guidelines

1. **Keep PRs focused.** One feature or fix per PR; don't bundle unrelated changes.
2. **Tests for new behavior.** New observable behavior should have a unit test. Integration tests are optional (they require a live Windows session).
3. **Pass CI.** All tests must be green and `cargo clippy` must be clean before merge.
4. **Describe the change.** The PR description should explain *why* the change is needed, not just *what* it does.
5. **Breaking config changes.** If you change the format of `config.toml`, bump the major version and update the CHANGELOG.

---

## Issue Guidelines

### Bug Reports

Please include:

- Windows version (e.g., Windows 11 23H2)
- Mouse model (if relevant)
- Steps to reproduce
- Expected behavior vs. actual behavior
- Log output (enable `log_path` in config.toml and attach the log file)

### Feature Requests

Describe the use case: *why* do you want this feature? What problem does it solve? If you have a reference implementation (macOS tool, another Windows app), link it.

---

## Code Conventions

- **Error handling:** Use `Result` for fallible operations; log errors with context; avoid silently swallowing errors.
- **Concurrency:** All shared state goes through `parking_lot` mutexes or `RwLock`s; hook callbacks must return in < 1ms so no blocking inside them.
- **Naming:** `snake_case` for functions/variables, `CamelCase` for types, `SCREAMING_SNAKE_CASE` for constants.
- **Comments:** Explain *why*, not *what*. Complex scroll/accel math should have a comment linking to the reference (e.g. Mac Mouse Fix source line).
- **No new runtime dependencies** without discussion — adding a dependency is a long-term maintenance commitment.

---

## Hot Paths

The scroll injection and hook callback paths are performance-critical. Keep them free of:

- Allocations (use stack buffers, fixed-size arrays, or pre-allocated scratch space)
- Lock acquisitions (shared state should be `RwLock` with read-preferring semantics)
- Logging (use compile-time `#[cfg(test)]` profiling only)
- Dynamic dispatch (no trait objects on hot paths)

---

## Security

This tool processes keyboard and mouse input. If you find a security vulnerability:

**Do not open a public GitHub Issue.** Instead, email the maintainer directly (see the SECURITY.md for contact information).

General guidelines:
- No network access without user consent (the phone trackpad explicitly requires token auth)
- No data exfiltration
- No persistent state outside the config file and log file

---

## License

By contributing, you agree that your contributions will be licensed under the MIT License.
