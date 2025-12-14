#!/bin/bash
# Docker-based tracing setup for macOS
# This runs QEMU and the tracer plugin inside a Docker container

set -e
cd "$(dirname "$0")/.."

BINARY="$1"
if [ -z "$BINARY" ]; then
    echo "Usage: $0 <binary_path>"
    echo "Example: $0 /tmp/koradar_test_hello"
    exit 1
fi

if [ ! -f "$BINARY" ]; then
    echo "Error: Binary not found: $BINARY"
    exit 1
fi

if ! command -v docker >/dev/null 2>&1; then
    echo "Error: Docker is required"
    exit 1
fi

echo "=== Koradar Docker Tracing (macOS) ==="
echo "Binary: $BINARY"
echo ""

# Check if QEMU is built in Docker
if [ ! -d "qemu-build-docker/bin" ] || [ ! -f "qemu-build-docker/bin/qemu-x86_64" ]; then
    echo "QEMU not found in qemu-build-docker. Building..."
    ./scripts/setup_qemu_docker.sh
fi

# Build tracer plugin for Linux (in Docker) if not already built
if [ ! -f "target/x86_64-unknown-linux-gnu/release/libkoradar_tracer.so" ]; then
    echo "Building tracer plugin for Linux (x86_64)..."
    docker run --rm --platform linux/amd64 \
        -v "$(pwd):/workspace" \
        -w /workspace \
        rust:latest \
        bash -c "rustup target add x86_64-unknown-linux-gnu && cargo build --release -p koradar-tracer --target x86_64-unknown-linux-gnu"
fi

# Check if QEMU is available
if [ ! -f "qemu-build-docker/bin/qemu-x86_64" ]; then
    echo "Error: QEMU not found. Run ./scripts/setup_qemu_docker.sh first"
    exit 1
fi

# Run QEMU with tracer in Docker
echo "Running QEMU with tracer in Docker (x86_64)..."
echo "Note: Make sure the server is running (make run) in another terminal"
echo "      Server should be listening on 0.0.0.0:3001"
echo ""
docker run --rm -i --platform linux/amd64 \
    -v "$(pwd)/qemu-build-docker/bin:/qemu-bin:ro" \
    -v "$(pwd)/target/x86_64-unknown-linux-gnu/release:/plugins:ro" \
    -v "$(realpath $BINARY):/binary:ro" \
    -v /tmp:/tmp \
    --network host \
    koradar-qemu-builder \
    /qemu-bin/qemu-x86_64 \
    -plugin /plugins/libkoradar_tracer.so \
    /binary

