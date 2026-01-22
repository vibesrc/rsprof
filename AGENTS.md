# Repository Guidelines

## Project Structure & Module Organization

- `crates/rsprof/`: main CLI/TUI application.
- `crates/rsprof-trace/`: instrumentation crate (allocator + shared memory events).
- `crates/rsprof/examples/`: example apps (`target_app`, `test_app`) for profiling demos.
- `docs/`: design notes and RFCs; `docs/media/` holds screenshots.
- Root `Cargo.toml`: workspace settings and shared build profiles.

## Build, Test, and Development Commands

- `cargo build`: debug build of all crates.
- `cargo build --release`: optimized build of rsprof.
- `make release`: same as `cargo build --release`.
- `make target`: build + run `target_app` (profiling profile + frame pointers).
- `make profile`: attach rsprof to a running `target_app`.
- `cargo test`: run test suite.
- `cargo fmt --all`: format all Rust code.
- `cargo clippy -p rsprof --all-targets -- -D warnings`: lint with warnings as errors.

## Coding Style & Naming Conventions

- Rust formatting via `cargo fmt` (rustfmt defaults).
- Linting via `cargo clippy` (warnings are treated as errors in CI).
- Use clear, short identifiers; prefer `snake_case` for functions/vars and `CamelCase` for types.

## Testing Guidelines

- Use `cargo test` for unit/integration tests.
- Examples (`crates/rsprof/examples/`) are not tests, but should compile cleanly.
- Keep tests small and deterministic; no external services required.

## Commit & Pull Request Guidelines

- Commit messages in this repo are short, imperative, and scoped (e.g., “Add include-internal and self-profile support”).
- PRs should describe the change, include reproduction steps or commands, and note any user‑visible behavior changes.

## Profiling Notes

- Accurate stack traces require frame pointers in the *target* app. For user apps:
  - Add a profiling profile in their `Cargo.toml` and build with:
    - `RUSTFLAGS="-C force-frame-pointers=yes" cargo build --profile profiling`
- `--include-internal` is required for rsprof‑ception so internal frames are recorded.
