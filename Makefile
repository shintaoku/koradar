.PHONY: all setup build build-frontend build-server build-tracer run trace test-binary clean

# Default target
all: build

# Install dependencies (Trunk for frontend)
setup:
	@echo "Installing trunk..."
	cargo install trunk || true
	@echo "Adding WASM target..."
	rustup target add wasm32-unknown-unknown

# Build everything
build: build-frontend build-server build-tracer

build-frontend:
	@echo "Building Frontend..."
	cd frontend && trunk build --release

build-server:
	@echo "Building Server..."
	cargo build --release -p koradar-server

build-tracer:
	@echo "Building Tracer..."
	cargo build --release -p koradar-tracer

# Run the Server (Back-end + Front-end hosting)
# Access http://localhost:3000 after running this
run: build-frontend build-server
	@echo "Starting Server at http://localhost:3000 ..."
	cargo run --release -p koradar-server

# Run QEMU with Tracer Plugin (Run this in a separate terminal)
# Usage: make trace [BINARY=/path/to/binary] [DOCKER=1]
# If BINARY is specified:
#   - On Linux: uses user-mode emulation (qemu-x86_64)
#   - On macOS: requires DOCKER=1 to run in Docker container
# Otherwise, starts system emulation monitor
trace: build-tracer
	@echo "Starting QEMU with Tracer..."
	@if [ -z "$(BINARY)" ]; then \
		echo "═══════════════════════════════════════════════════════════"; \
		echo "⚠️  No binary specified - Starting QEMU system emulation"; \
		echo "═══════════════════════════════════════════════════════════"; \
		echo ""; \
		echo "This will show 'No bootable device' - this is EXPECTED behavior."; \
		echo "System emulation mode requires a kernel/disk image to boot."; \
		echo ""; \
		echo "To actually trace a Linux binary:"; \
		if [[ "$OSTYPE" == "darwin"* ]]; then \
			echo "  ./scripts/trace_docker.sh /path/to/binary"; \
			echo "  (macOS requires Docker for linux-user mode)"; \
		else \
			echo "  make trace BINARY=/path/to/binary"; \
		fi; \
		echo ""; \
		echo "To exit: Press Ctrl+C or type 'quit' in the QEMU monitor."; \
		echo "═══════════════════════════════════════════════════════════"; \
		echo ""; \
		./qemu-build/bin/qemu-system-x86_64 \
			-plugin ./target/release/libkoradar_tracer.dylib \
			-monitor stdio \
			-display none \
			-no-reboot; \
	else \
		echo "Tracing binary: $(BINARY)"; \
		if [[ "$OSTYPE" == "darwin"* ]]; then \
			if [ "$(DOCKER)" != "1" ]; then \
				echo "Error: On macOS, linux-user mode requires Docker."; \
				echo "  Run: make trace BINARY=$(BINARY) DOCKER=1"; \
				echo "  Or use: ./scripts/setup_qemu_docker.sh first"; \
				exit 1; \
			fi; \
			echo "Running QEMU in Docker container..."; \
			docker run --rm -it \
				-v "$(pwd)/target/release:/plugins:ro" \
				-v "$(pwd)/qemu-build-docker/bin:/qemu-bin:ro" \
				-v "$(BINARY):/binary:ro" \
				-v /tmp:/tmp \
				ubuntu:22.04 \
				/qemu-bin/qemu-x86_64 \
				-plugin /plugins/libkoradar_tracer.so \
				/binary; \
		else \
			if [ ! -f "./qemu-build/bin/qemu-x86_64" ]; then \
				echo "Error: qemu-x86_64 not found. Please rebuild QEMU with user-mode support:"; \
				echo "  ./scripts/setup_qemu.sh"; \
				exit 1; \
			fi; \
			./qemu-build/bin/qemu-x86_64 \
				-plugin ./target/release/libkoradar_tracer.so \
				$(BINARY); \
		fi; \
	fi

# Create a simple test binary (requires Docker or cross-compiler)
test-binary:
	@./scripts/create_test_binary.sh
	@echo ""
	@echo "Test binary created at /tmp/koradar_test_hello"
	@echo "Run: make trace BINARY=/tmp/koradar_test_hello"

# Clean artifacts
clean:
	cargo clean
	cd frontend && trunk clean

