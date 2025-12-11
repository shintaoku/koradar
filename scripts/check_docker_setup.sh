#!/bin/bash
# Quick test to verify Docker tracing setup
# This checks if all components are ready for Docker-based tracing

cd "$(dirname "$0")/.."

echo "=== Koradar Docker Tracing Readiness Check ==="
echo ""

# Check Docker
if ! command -v docker >/dev/null 2>&1; then
    echo "❌ Docker not found"
    exit 1
fi

if ! docker info >/dev/null 2>&1; then
    echo "❌ Docker is not running"
    exit 1
fi
echo "✅ Docker is available and running"

# Check test binary
if [ -f "/tmp/koradar_test_hello" ]; then
    ARCH=$(file /tmp/koradar_test_hello | grep -o "x86-64\|ARM\|aarch64")
    if echo "$ARCH" | grep -q "x86-64"; then
        echo "✅ Test binary exists and is x86_64: /tmp/koradar_test_hello"
    else
        echo "⚠️  Test binary exists but is $ARCH (should be x86-64)"
        echo "   Run: make test-binary (will recreate with correct architecture)"
    fi
else
    echo "⚠️  Test binary not found: /tmp/koradar_test_hello"
    echo "   Run: make test-binary"
fi

# Check QEMU in Docker
if [ -f "qemu-build-docker/bin/qemu-x86_64" ]; then
    echo "✅ QEMU (Docker) is built: qemu-build-docker/bin/qemu-x86_64"
else
    echo "⚠️  QEMU (Docker) not found"
    echo "   Run: ./scripts/setup_qemu_docker.sh (takes 10-30 minutes)"
fi

# Check tracer plugin for Linux
if [ -f "target/x86_64-unknown-linux-gnu/release/libkoradar_tracer.so" ]; then
    echo "✅ Tracer plugin (Linux) is built"
else
    echo "⚠️  Tracer plugin (Linux) not built yet"
    echo "   Will be built automatically when running: ./scripts/trace_docker.sh"
fi

# Check server
if pgrep -f "koradar-server" >/dev/null 2>&1; then
    echo "✅ Server is running"
else
    echo "⚠️  Server is not running"
    echo "   Run in another terminal: make run"
fi

echo ""
echo "=== Ready to trace? ==="
if [ -f "/tmp/koradar_test_hello" ] && [ -f "qemu-build-docker/bin/qemu-x86_64" ]; then
    echo "✅ Yes! Run: ./scripts/trace_docker.sh /tmp/koradar_test_hello"
else
    echo "❌ Not yet. Complete the missing steps above."
fi

