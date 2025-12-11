#!/bin/bash
# Don't use set -e here, we want to handle errors gracefully
set -u  # Only fail on undefined variables

cd "$(dirname "$0")/.."

# Configuration
QEMU_VERSION="master"
INSTALL_DIR="$(pwd)/qemu-build"
SOURCE_DIR="qemu-src"

echo "=== Koradar QEMU Setup ==="

# Check for required tools
if ! command -v ninja &> /dev/null; then
    echo "Ninja not found. Attempting to install via pip..."
    pip3 install ninja || { echo "Failed to install ninja. Please install it manually."; exit 1; }
fi

# Ideally check for other python deps, but pip install is safe to re-run
echo "Installing Python build dependencies..."
pip3 install --user sphinx sphinx_rtd_theme distlib || true

if [ -d "$INSTALL_DIR/bin" ]; then
    # Check if user mode emulation is available
    if [ -f "$INSTALL_DIR/bin/qemu-x86_64" ]; then
        echo "QEMU seems to be already installed in $INSTALL_DIR (with user mode)"
        echo "To reinstall, remove the directory and run this script again."
        exit 0
    else
        echo "QEMU is installed but missing user mode emulation."
        echo "Rebuilding with user mode support..."
        # Continue to rebuild
    fi
fi

if [ ! -d "$SOURCE_DIR" ]; then
    echo "Cloning QEMU source ($QEMU_VERSION)..."
    git clone --depth 1 --branch "$QEMU_VERSION" https://github.com/qemu/qemu.git "$SOURCE_DIR"
fi

cd "$SOURCE_DIR"

# If user mode is missing, we need to reconfigure and rebuild
if [ -d "$INSTALL_DIR/bin" ] && [ ! -f "$INSTALL_DIR/bin/qemu-x86_64" ]; then
    # Check if we're on macOS - linux-user is not supported on macOS
    if [[ "$OSTYPE" == "darwin"* ]]; then
        echo "⚠️  WARNING: QEMU linux-user mode is not supported on macOS."
        echo "   Linux binaries cannot be traced directly on macOS."
        echo ""
        echo "   Options:"
        echo "   1. Use Docker to run QEMU in a Linux container"
        echo "   2. Use system emulation mode (more complex setup)"
        echo "   3. Build QEMU on a Linux machine"
        echo ""
        echo "   For now, only system emulation (qemu-system-x86_64) is available."
        exit 1
    fi
    
    echo "Reconfiguring QEMU to include user mode emulation..."
    if ! ./configure \
        --prefix="$INSTALL_DIR" \
        --target-list=x86_64-softmmu,x86_64-linux-user \
        --enable-plugins \
        --enable-debug-tcg \
        --disable-werror \
        --disable-docs; then
        echo "ERROR: QEMU configure failed. Check qemu-src/config.log for details."
        exit 1
    fi
    echo "Rebuilding QEMU (this may take a while)..."
    if ! make -j$(nproc); then
        echo "ERROR: QEMU build failed. Check the output above for details."
        exit 1
    fi
    if ! make install; then
        echo "ERROR: QEMU install failed. Check the output above for details."
        exit 1
    fi
    if [ -f "$INSTALL_DIR/bin/qemu-x86_64" ]; then
        echo "=== QEMU Rebuild Complete ==="
        echo "✅ qemu-x86_64 is now available"
        exit 0
    else
        echo "ERROR: qemu-x86_64 was not installed despite successful build."
        exit 1
    fi
fi

echo "Configuring QEMU..."
# We only need system emulation for x86_64, aarch64, and user emulation for common targets
# Plugins are essential for Koradar
# Added --enable-slirp=git (or system) if needed, but let's stick to defaults first.
# Using --disable-docs to avoid sphinx issues if installation failed.
if ! ./configure \
    --prefix="$INSTALL_DIR" \
    --target-list=x86_64-softmmu,x86_64-linux-user \
    --enable-plugins \
    --enable-debug-tcg \
    --disable-werror \
    --disable-docs; then
    echo "ERROR: QEMU configure failed. Check qemu-src/config.log for details."
    exit 1
fi

echo "Building QEMU (this may take a while)..."
if ! make -j$(nproc); then
    echo "ERROR: QEMU build failed. Check the output above for details."
    exit 1
fi

if ! make install; then
    echo "ERROR: QEMU install failed. Check the output above for details."
    exit 1
fi

echo "=== QEMU Setup Complete ==="
echo "Binaries are in $INSTALL_DIR/bin"
if [ -f "$INSTALL_DIR/bin/qemu-x86_64" ]; then
    echo "✅ qemu-x86_64 (user mode) is available"
else
    echo "⚠️  WARNING: qemu-x86_64 was not built. User mode emulation unavailable."
fi
if [ -f "$INSTALL_DIR/bin/qemu-system-x86_64" ]; then
    echo "✅ qemu-system-x86_64 (system mode) is available"
fi
