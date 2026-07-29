#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
TARGET=riscv64gc-unknown-linux-musl
export CARGO_TARGET_RISCV64GC_UNKNOWN_LINUX_MUSL_LINKER="${RISCV_MUSL_GCC:-/home/yfblock/Env/riscv64-linux-musl-cross/bin/riscv64-linux-musl-gcc}"
rustup target list --installed 2>/dev/null | grep -q "^${TARGET}$" || rustup target add "$TARGET"
cargo build --release --target "$TARGET"
BIN="target/${TARGET}/release/rgb-check"
echo "built: $BIN"
file "$BIN" 2>/dev/null || true
