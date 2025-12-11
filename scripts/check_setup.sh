#!/bin/bash
# Quick script to check QEMU build status and provide guidance

cd "$(dirname "$0")/.."

echo "=== Koradar QEMU Status Check ==="
echo ""

if [ ! -d "qemu-build/bin" ]; then
    echo "❌ QEMU not built yet."
    echo "   Run: ./scripts/setup_qemu.sh"
    exit 1
fi

echo "✅ QEMU binaries found:"
ls -1 qemu-build/bin/qemu-* 2>/dev/null | sed 's|.*/||' | sed 's/^/   /'

echo ""
if [ -f "qemu-build/bin/qemu-x86_64" ]; then
    echo "✅ User mode emulation (qemu-x86_64) is available"
    echo "   You can trace binaries with: make trace BINARY=/path/to/binary"
else
    echo "❌ User mode emulation (qemu-x86_64) is NOT available"
    echo "   Run: ./setup_qemu.sh (it will detect and rebuild)"
fi

echo ""
if [ -f "qemu-build/bin/qemu-system-x86_64" ]; then
    echo "✅ System emulation (qemu-system-x86_64) is available"
else
    echo "❌ System emulation (qemu-system-x86_64) is NOT available"
fi

echo ""
echo "=== Tracer Plugin Status ==="
if [ -f "target/release/libkoradar_tracer.dylib" ] || [ -f "target/release/libkoradar_tracer.so" ]; then
    echo "✅ Tracer plugin built"
else
    echo "❌ Tracer plugin not built"
    echo "   Run: cargo build --release -p koradar-tracer"
fi

