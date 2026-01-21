.PHONY: build release test clean target run-target profile help

# Default target
help:
	@echo "rsprof - Zero-instrumentation profiler for Rust"
	@echo ""
	@echo "Usage:"
	@echo "  make build        Build debug version"
	@echo "  make release      Build release version"
	@echo "  make test         Run tests"
	@echo "  make target       Build the example target app (profiling profile)"
	@echo "  make run-target   Run the example target app"
	@echo "  make profile      Profile the target app for 10s"
	@echo "  make clean        Clean build artifacts"
	@echo ""
	@echo "Quick start:"
	@echo "  make release target"
	@echo "  make run-target   # In terminal 1"
	@echo "  make profile      # In terminal 2"

# Build targets
build:
	cargo build

release:
	cargo build --release

test:
	cargo test

clean:
	cargo clean
	rm -f rsprof.*.db

# Example target app - profiling build with frame pointers
build-target:
	RUSTFLAGS="-C force-frame-pointers=yes" cargo build --profile profiling -p rsprof --example target_app

target: build-target
	./target/profiling/examples/target_app

run-target: target

# Profile the running target app
PROFILE_DURATION ?= 10s
PROFILE_OUTPUT ?= profile.db

profile: release
	@PID=$$(pgrep -x target_app 2>/dev/null); \
	if [ -z "$$PID" ]; then \
		echo "Error: target_app is not running."; \
		echo "Start it first with: make run-target"; \
		exit 1; \
	fi; \
	echo "Profiling target_app (PID $$PID) for $(PROFILE_DURATION)..."; \
	./target/release/rsprof --pid $$PID -o $(PROFILE_OUTPUT)

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
	@echo "Starting target_app in background..."
	@./target/profiling/examples/target_app & \
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
