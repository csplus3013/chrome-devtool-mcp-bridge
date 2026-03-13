#!/usr/bin/env bash
set -euo pipefail

# 1. FIX: ROOT_DIR is now correctly the project root
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "${ROOT_DIR}"

IMAGE="ubuntu:20.04"

# 2. ADD CACHE: Create local folders to store the toolchain and crates
mkdir -p .docker_cargo_cache/registry
mkdir -p .docker_cargo_cache/rustup

echo "[build-aarch64] Starting build in $ROOT_DIR"

docker run --rm -it \
  -v "${ROOT_DIR}":/work -w /work \
  -v "${ROOT_DIR}/.docker_cargo_cache/registry":/root/.cargo/registry \
  -v "${ROOT_DIR}/.docker_cargo_cache/rustup":/root/.rustup \
  -e DEBIAN_FRONTEND=noninteractive \
  "${IMAGE}" bash -c '
    # 3. SILENT INSTALL: Update and install tools without prompts
    apt-get update -qq && \
    apt-get install -y -qq build-essential curl pkg-config ca-certificates gcc-aarch64-linux-gnu < /dev/null && \
    
    # Only install Rust if it is not already in the cache
    if [ ! -f "/root/.cargo/bin/rustup" ]; then
        curl https://sh.rustup.rs -sSf | sh -s -- -y --default-toolchain stable
    fi
    
    source /root/.cargo/env
    rustup target add aarch64-unknown-linux-gnu
    
    # 4. CROSS-COMPILE
    export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc
    export CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc
    
    cargo build --release --target aarch64-unknown-linux-gnu
  '

# 5. WRAP UP
if [ -f "target/aarch64-unknown-linux-gnu/release/mcp-server-bridge" ]; then
    cp target/aarch64-unknown-linux-gnu/release/mcp-server-bridge ./mcp-server-bridge-aarch64
    tar -cvzf mcp-server-bridge_aarch64_bin.tar mcp-server-bridge-aarch64
    echo "[build-aarch64] Success! Binary: ./mcp-server-bridge-aarch64"
else
    echo "Build failed: Binary not found."
    exit 1
fi
