#!/bin/bash
# Quick test script to verify QEMU rebuild will work
# This simulates the first part of setup_qemu.sh

cd "$(dirname "$0")/.."
INSTALL_DIR="$(pwd)/qemu-build"
SOURCE_DIR="qemu-src"

echo "=== Testing QEMU Rebuild Logic ==="
echo ""

if [ ! -d "$SOURCE_DIR" ]; then
    echo "❌ ERROR: qemu-src directory not found"
    echo "   Run: git clone --depth 1 --branch master https://github.com/qemu/qemu.git qemu-src"
    exit 1
fi

if [ ! -f "$SOURCE_DIR/configure" ]; then
    echo "❌ ERROR: configure script not found in qemu-src"
    exit 1
fi

if [ -d "$INSTALL_DIR/bin" ] && [ ! -f "$INSTALL_DIR/bin/qemu-x86_64" ]; then
    echo "✅ Rebuild path will be triggered"
    echo ""
    echo "Testing configure command (dry run)..."
    cd "$SOURCE_DIR"
    
    # Test if configure accepts the target list
    if ./configure --help | grep -q "x86_64-linux-user"; then
        echo "✅ x86_64-linux-user target is supported"
    else
        echo "⚠️  WARNING: x86_64-linux-user may not be supported on this platform"
    fi
    
    echo ""
    echo "To actually rebuild, run: ./scripts/setup_qemu.sh"
    echo "This will take 10-30 minutes."
else
    if [ -f "$INSTALL_DIR/bin/qemu-x86_64" ]; then
        echo "✅ qemu-x86_64 already exists - no rebuild needed"
    else
        echo "ℹ️  Initial build path (qemu-build/bin doesn't exist yet)"
    fi
fi

