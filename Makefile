.PHONY: build release test clean example profile help profile-profile profile-profiler build-profiler

# Default target
help:
	@echo "rsprof - Zero-instrumentation profiler for Rust"
	@echo ""
	@echo "Usage:"
	@echo "  make build        Build debug version"
	@echo "  make release      Build release version"
	@echo "  make test         Run tests"
	@echo "  make example      Build and run the example app (profiling profile)"
	@echo "  make profile      Profile the example app for 10s (manual run)"
	@echo "  make profile-profile   Run rsprof in this terminal (attaches to running example_app)"
	@echo "  make profile-profiler  Run rsprof in this terminal (attaches to running rsprof)"
	@echo "  make clean        Clean build artifacts"
	@echo ""
	@echo "Quick start:"
	@echo "  make release example"
	@echo "  make example      # In terminal 1"
	@echo "  make profile      # In terminal 2"

# Build targets
build:
	cargo build

release:
	cargo build --release

build-profiler:
	RUSTFLAGS="-C force-frame-pointers=yes" cargo build --profile profiling -p rsprof --features self-profile

test:
	cargo test

clean:
	cargo clean
	rm -f rsprof.*.db

# Example app - profiling build with frame pointers
build-example:
	cd example && cargo build --profile profiling

example: build-example
	./example/target/profiling/example_app

# Profile the running example app
PROFILE_DURATION ?= 10s
PROFILE_OUTPUT ?= profile.db

profile: release
	@PID=$$(pgrep -x example_app 2>/dev/null); \
	if [ -z "$$PID" ]; then \
		echo "Error: example_app is not running."; \
		echo "Start it first with: make example"; \
		exit 1; \
	fi; \
	echo "Profiling example_app (PID $$PID) for $(PROFILE_DURATION)..."; \
	./target/release/rsprof --pid $$PID -o $(PROFILE_OUTPUT) --append

# View the last profile
top-cpu:
	@if [ ! -f $(PROFILE_OUTPUT) ]; then \
		echo "No profile found. Run 'make profile' first."; \
		exit 1; \
	fi; \
	./target/release/rsprof top cpu $(PROFILE_OUTPUT) --top 100

top-heap:
	@if [ ! -f $(PROFILE_OUTPUT) ]; then \
		echo "No profile found. Run 'make profile' first."; \
		exit 1; \
	fi; \
	./target/release/rsprof top heap $(PROFILE_OUTPUT) --top 100

top-both: top-cpu top-heap


top-json:
	@if [ ! -f $(PROFILE_OUTPUT) ]; then \
		echo "No profile found. Run 'make profile' first."; \
		exit 1; \
	fi; \
	./target/release/rsprof top cpu $(PROFILE_OUTPUT) --json

# Interactive TUI viewer
view:
	@if [ ! -f $(PROFILE_OUTPUT) ]; then \
		echo "No profile found. Run 'make profile' first."; \
		exit 1; \
	fi; \
	./target/release/rsprof view $(PROFILE_OUTPUT)

# Quick demo: build everything, run target in background, profile, show results
demo: release target
	@echo "Starting example_app in background..."
	@./example_app/target/profiling/example_app & \
	APP_PID=$$!; \
	sleep 2; \
	echo "Profiling for 5 seconds..."; \
	./target/release/rsprof --pid $$APP_PID --quiet --duration 5s -o demo.db; \
	kill $$APP_PID 2>/dev/null || true; \
	echo ""; \
	echo "=== Profile Results ==="; \
	./target/release/rsprof top cpu demo.db --top 15; \
	rm -f demo.db

# Install to ~/.cargo/bin
install: release
	cp target/release/rsprof ~/.cargo/bin/
	@echo "Installed rsprof to ~/.cargo/bin/"
