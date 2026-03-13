#!/usr/bin/env bash
set -euo pipefail

# Build mcp-server-bridge for aarch64 with older glibc compatibility using Ubuntu 20.04.
# Output: .aarch64-machine/bin/mcp-server-bridge-aarch64

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if ! command -v docker >/dev/null 2>&1; then
  echo "Docker is required to run this script." >&2
  exit 1
fi

cd "${ROOT_DIR}"

IMAGE="ubuntu:20.04"

cat <<'EOF'
[build-aarch64] Using Docker image: ubuntu:20.04
[build-aarch64] Building aarch64 binary with glibc 2.31 compatibility
EOF

docker run --rm \
  -e DEBIAN_FRONTEND=noninteractive \
  -v "${ROOT_DIR}":/work -w /work \
  "${IMAGE}" bash -lc '
    apt-get update && \
    apt-get install -y --no-install-recommends build-essential curl pkg-config ca-certificates gcc-aarch64-linux-gnu libc6-dev-arm64-cross && \
    curl https://sh.rustup.rs -sSf | sh -s -- -y && \
    . $HOME/.cargo/env && \
    rustup target add aarch64-unknown-linux-gnu && \
    CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
    CXX_aarch64_unknown_linux_gnu=aarch64-linux-gnu-g++ \
    AR_aarch64_unknown_linux_gnu=aarch64-linux-gnu-ar \
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
    RUSTFLAGS="-C linker=aarch64-linux-gnu-gcc" \
    PKG_CONFIG_ALLOW_CROSS=1 \
    PKG_CONFIG_PATH=/usr/aarch64-linux-gnu/lib/pkgconfig \
    PKG_CONFIG_SYSROOT_DIR=/usr/aarch64-linux-gnu \
    cargo build --release --target aarch64-unknown-linux-gnu
  '

mkdir -p .aarch64-machine/bin
cp target/aarch64-unknown-linux-gnu/release/mcp-server-bridge .aarch64-machine/bin/mcp-server-bridge-aarch64
echo "[build-aarch64] Done: .aarch64-machine/bin/mcp-server-bridge-aarch64"
