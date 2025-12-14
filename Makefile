.PHONY: all setup build build-frontend build-server build-tracer build-tracer-linux run trace test-binary clean

# Detect OS
UNAME_S := $(shell uname -s)

# Default target
all: build

# Install dependencies (Trunk for frontend)
setup:
	@echo "Installing trunk..."
	cargo install trunk || true
	@echo "Adding WASM target..."
	rustup target add wasm32-unknown-unknown

# Build everything
# On macOS, also build the Linux tracer for Docker use
build: build-frontend build-server build-tracer
ifeq ($(UNAME_S),Darwin)
	@$(MAKE) build-tracer-linux
endif

build-frontend:
	@echo "Building Frontend..."
	cd frontend && trunk build --release

build-server:
	@echo "Building Server..."
	cargo build --release -p koradar-server

build-tracer:
	@echo "Building Tracer (Local)..."
	cargo build --release -p koradar-tracer

# Build tracer for Linux (used by Docker on macOS)
build-tracer-linux:
	@echo "Building Tracer for Linux (Docker)..."
	@if command -v docker >/dev/null 2>&1; then \
		docker run --rm --platform linux/amd64 \
			-v "$$(pwd):/workspace" \
			-w /workspace \
			rust:latest \
			bash -c "apt-get update && apt-get install -y libglib2.0-dev && rustup target add x86_64-unknown-linux-gnu && cargo build --release -p koradar-tracer --target x86_64-unknown-linux-gnu"; \
	else \
		echo "Docker not found, skipping Linux tracer build."; \
	fi

# Run the Server (Back-end + Front-end hosting)
# Access http://localhost:3000 after running this
run:
	@echo "Starting Server at http://localhost:3000 ..."
	@if [ -n "$(BINARY)" ]; then \
		cargo run --release -p koradar-server -- $(BINARY); \
	else \
		cargo run --release -p koradar-server; \
	fi

# Run QEMU with Tracer Plugin (Run this in a separate terminal)
# Usage: make trace [BINARY=/path/to/binary] [DOCKER=1]
# If BINARY is specified:
#   - On Linux: uses user-mode emulation (qemu-x86_64)
#   - On macOS: requires DOCKER=1 to run in Docker container
# Otherwise, starts system emulation monitor
trace: build-tracer
	@echo "Starting QEMU with Tracer..."
	@if [ -z "$(BINARY)" ]; then \
		echo "Error: BINARY is not specified."; \
		echo "Usage: make trace BINARY=/path/to/binary [DOCKER=1]"; \
		exit 1; \
	fi
	@echo "Tracing binary: $(BINARY)"
	@if [ "$(UNAME_S)" = "Darwin" ]; then \
		if [ "$(DOCKER)" != "1" ]; then \
			echo "Error: On macOS, linux-user mode requires Docker."; \
			echo "  Run: make trace BINARY=$(BINARY) DOCKER=1"; \
			echo "  Or use: ./scripts/trace_docker.sh $(BINARY)"; \
			exit 1; \
		fi; \
		./scripts/trace_docker.sh $(BINARY); \
	else \
		if [ ! -f "./qemu-build/bin/qemu-x86_64" ]; then \
			echo "Error: qemu-x86_64 not found. Please rebuild QEMU with user-mode support:"; \
			echo "  ./scripts/setup_qemu.sh"; \
			exit 1; \
		fi; \
		./qemu-build/bin/qemu-x86_64 \
			-plugin ./target/release/libkoradar_tracer.so \
			$(BINARY); \
	fi

# Create a simple test binary (requires Docker or cross-compiler)
test-binary:
	@./scripts/create_test_binary.sh
	@echo ""
	@echo "Test binary created at /tmp/koradar_test_hello"
	@echo "Run: make trace BINARY=/tmp/koradar_test_hello"

# Build and run the vulnerable stack example
example-vuln:
	@echo "Building vulnerable example..."
	@mkdir -p examples/vuln_stack/build
	@# Compile with -fno-stack-protector and -no-pie to make it easy to exploit
	@if command -v gcc >/dev/null 2>&1 && [ "$(UNAME_S)" = "Linux" ]; then \
		gcc -fno-stack-protector -no-pie examples/vuln_stack/vuln.c -o examples/vuln_stack/build/vuln; \
	elif command -v docker >/dev/null 2>&1; then \
		docker run --rm --platform linux/amd64 -v "$$(pwd)/examples/vuln_stack:/src" -w /src \
			gcc:latest gcc -static -fno-stack-protector -no-pie vuln.c -o build/vuln; \
	else \
		echo "Error: Cannot build. Need gcc (on Linux) or docker."; \
		exit 1; \
	fi
	@echo "Binary built at examples/vuln_stack/build/vuln"
	@echo "Generating payload..."
	@python3 examples/vuln_stack/exploit.py > examples/vuln_stack/build/payload
	@echo "Running trace..."
	@# Use trace_docker.sh for consistency on macOS
	@if [ "$(UNAME_S)" = "Darwin" ]; then \
		./scripts/trace_docker.sh examples/vuln_stack/build/vuln < examples/vuln_stack/build/payload; \
	else \
		./qemu-build/bin/qemu-x86_64 -plugin ./target/release/libkoradar_tracer.so examples/vuln_stack/build/vuln < examples/vuln_stack/build/payload; \
	fi

# Clean artifacts
clean:
	cargo clean
	cd frontend && trunk clean

