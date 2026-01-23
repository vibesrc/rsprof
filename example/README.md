# rsprof example app

This is a standalone Cargo app that shows how to instrument a real Rust binary
with `rsprof-trace` and run `rsprof` against it.

## Build (with profiling profile + frame pointers)

```bash
cd example_app
cargo build --profile profiling
```

## Run and profile

```bash
# Terminal 1: run the app
./target/profiling/example_app &

# Terminal 2: attach rsprof to the running app
rsprof -p $(pgrep example_app)
```

Notes:
- Frame pointers are forced via `example_app/.cargo/config.toml`.
- `rsprof-trace::profiler!()` enables CPU + heap profiling.
