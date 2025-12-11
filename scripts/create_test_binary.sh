#!/bin/bash
# Create a simple test binary for Koradar tracing
# This script creates a minimal Linux x86_64 binary

set -e
cd "$(dirname "$0")/.."

OUTPUT="/tmp/koradar_test_hello"
TMPDIR=$(mktemp -d)
trap "rm -rf $TMPDIR" EXIT

cat > "$TMPDIR/hello.c" << 'EOF'
#include <stdio.h>
#include <unistd.h>

int main() {
    printf("Hello, Koradar!\n");
    int x = 42;
    int y = x + 1;
    printf("x = %d, y = %d\n", x, y);
    return y;
}
EOF

echo "Creating test binary..."

# Try Docker first (most reliable on macOS)
if command -v docker >/dev/null 2>&1; then
    echo "Using Docker to compile (x86_64)..."
    # Use --platform to force x86_64 architecture
    docker run --rm --platform linux/amd64 \
        -v "$TMPDIR:/work" \
        -v "$(dirname $OUTPUT):$(dirname $OUTPUT)" \
        -w /work \
        ubuntu:20.04 \
        bash -c "apt-get update -qq >/dev/null 2>&1 && \
                 apt-get install -y -qq gcc >/dev/null 2>&1 && \
                 gcc -static -o $OUTPUT hello.c && \
                 chmod +x $OUTPUT"
    if [ -f "$OUTPUT" ]; then
        echo "✓ Created: $OUTPUT"
        file "$OUTPUT"
        exit 0
    fi
fi

# Try cross-compiler
if command -v x86_64-linux-gnu-gcc >/dev/null 2>&1; then
    echo "Using cross-compiler..."
    x86_64-linux-gnu-gcc -static -o "$OUTPUT" "$TMPDIR/hello.c"
    if [ -f "$OUTPUT" ]; then
        echo "✓ Created: $OUTPUT"
        file "$OUTPUT"
        exit 0
    fi
fi

echo "Error: Cannot create test binary."
echo "Please install Docker or a cross-compiler (x86_64-linux-gnu-gcc)"
echo "Or provide your own Linux x86_64 binary."
exit 1
