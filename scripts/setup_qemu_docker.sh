#!/bin/bash
# Docker-based QEMU setup for macOS users
# This builds and runs QEMU in a Linux container to enable linux-user mode

set -e
cd "$(dirname "$0")/.."

echo "=== Koradar Docker QEMU Setup (for macOS) ==="
echo ""

if ! command -v docker >/dev/null 2>&1; then
    echo "❌ ERROR: Docker is required for this setup"
    echo "   Install Docker Desktop from https://www.docker.com/products/docker-desktop"
    exit 1
fi

# Check if Docker is running
if ! docker info >/dev/null 2>&1; then
    echo "❌ ERROR: Docker is not running"
    echo "   Please start Docker Desktop"
    exit 1
fi

INSTALL_DIR="$(pwd)/qemu-build-docker"
SOURCE_DIR="qemu-src-docker"

echo "This will build QEMU with linux-user support in a Docker container."
echo "The binaries will be available at: $INSTALL_DIR"
echo ""

# Create a Dockerfile for building QEMU
cat > Dockerfile.qemu << 'DOCKER_EOF'
FROM ubuntu:22.04

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update && apt-get install -y \
    build-essential \
    git \
    ninja-build \
    python3 \
    python3-pip \
    python3-tomli \
    pkg-config \
    libglib2.0-dev \
    libpixman-1-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /qemu-build

CMD ["/bin/bash"]
DOCKER_EOF

echo "Building QEMU in Docker container (this will take 10-30 minutes)..."
echo ""

# Build QEMU in Docker (force x86_64 platform)
docker build --platform linux/amd64 -f Dockerfile.qemu -t koradar-qemu-builder .

# Ensure output directory exists
mkdir -p qemu-build-docker

# Run the build (force x86_64 platform)
# We mount the current directory to /host so we can copy artifacts back
# We try to use local qemu-src if available (mounted to /qemu-src), otherwise clone inside container
if [ -d "qemu-src" ]; then
    MOUNT_SRC="-v $(pwd)/qemu-src:/qemu-src"
else
    MOUNT_SRC=""
fi

docker run --rm --platform linux/amd64 -v "$(pwd):/host" $MOUNT_SRC koradar-qemu-builder bash -c "
    if [ ! -d /qemu-src ] || [ -z \"\$(ls -A /qemu-src)\" ]; then
        echo 'Cloning QEMU...'
        git clone --depth 1 --branch master https://github.com/qemu/qemu.git /qemu-src
    fi
    cd /qemu-src
    ./configure --prefix=/qemu-build/install --target-list=x86_64-softmmu,x86_64-linux-user --enable-plugins --enable-debug-tcg --disable-werror --disable-docs
    make -j\$(nproc)
    make install
    cp -r /qemu-build/install/* /host/qemu-build-docker/
"

echo ""
echo "⚠️  NOTE: QEMU binaries built in Docker are Linux binaries and cannot run directly on macOS."
echo ""
echo "To trace Linux binaries on macOS, you need to run QEMU inside Docker."
echo "See README.md for Docker-based tracing instructions."
echo ""
echo "Alternatively, use system emulation mode (more complex but works on macOS)."

