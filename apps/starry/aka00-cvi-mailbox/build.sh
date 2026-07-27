#!/usr/bin/env bash
# 构建 mbmon（riscv64 musl 静态 PIE，StarryOS 用户态）→ mbmon
# 部署：拷到板子 rootfs /root/mbmon（或 SD 卡 p3 的 /root）。
set -euo pipefail
cd "$(dirname "$0")"

TARGET=riscv64gc-unknown-linux-musl
export CARGO_TARGET_RISCV64GC_UNKNOWN_LINUX_MUSL_LINKER="${RISCV_MUSL_GCC:-/home/yfblock/Env/riscv64-linux-musl-cross/bin/riscv64-linux-musl-gcc}"

if ! rustup target list --installed 2>/dev/null | grep -q "^${TARGET}$"; then
  rustup target add "$TARGET"
fi

cargo build --release --target "$TARGET"
BIN="target/${TARGET}/release/mbmon"
echo "built: $BIN"
file "$BIN" 2>/dev/null || true
echo "部署: 拷到板子 /root/mbmon"
