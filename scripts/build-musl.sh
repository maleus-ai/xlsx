#!/bin/sh
# Build the musl addon inside Alpine, for the architecture given (or the host's).
#
#   scripts/build-musl.sh          # whatever this machine is
#   scripts/build-musl.sh x64
#   scripts/build-musl.sh arm64
#
# Why a container rather than a cross toolchain: the generated applications
# deploy on `node:24-alpine`, so `x86_64-unknown-linux-musl` is not one target
# among five — it is the one that has to work, and it goes straight to npm.
# Building it against the libc it will run on removes a class of surprise; doing
# it from an image pinned by digest removes a supply chain. A compiler fetched
# over plain HTTPS from a third-party host, with no checksum, would be a shorter
# path into every application that reads an upload than any of the archives this
# reader refuses.
set -eu

# rust:alpine, pinned. Bump deliberately, never implicitly.
IMAGE="rust@sha256:a10e64dd139b7387337c7fbe8aca31b959b57b2fd4c8ae20a02cf1d6ea424dce"

ROOT=$(cd "$(dirname "$0")/.." && pwd)

ARCH=${1:-}
if [ -z "$ARCH" ]; then
  case $(uname -m) in
    x86_64) ARCH=x64 ;;
    aarch64 | arm64) ARCH=arm64 ;;
    *) echo "unsupported host architecture: $(uname -m)" >&2; exit 1 ;;
  esac
fi

case "$ARCH" in
  x64) PLATFORM=linux/amd64 ;;
  arm64) PLATFORM=linux/arm64 ;;
  *) echo "usage: $0 [x64|arm64]" >&2; exit 1 ;;
esac

OUT="$ROOT/crates/xlsx-node/xlsx.linux-$ARCH-musl.node"
TARGET_DIR="$ROOT/target-musl-$ARCH"

docker run --rm \
  --platform "$PLATFORM" \
  -v "$ROOT:/work" \
  -w /work \
  -e CARGO_TARGET_DIR="/work/target-musl-$ARCH" \
  -e HOST_UID="$(id -u)" \
  -e HOST_GID="$(id -g)" \
  -e TARGET_DIR="/work/target-musl-$ARCH" \
  "$IMAGE" \
  sh -c '
    set -eu
    apk add --no-cache musl-dev >/dev/null
    # A Node addon is loaded with dlopen, so it has to be a dynamic library. On
    # musl, Rust links statically by default and the result cannot be loaded.
    RUSTFLAGS="-C target-feature=-crt-static" cargo build -p xlsx-node --release
    # The build ran as root on a mounted volume. Handing the tree back means the
    # next `cargo clean` on the host is not a sudo.
    chown -R "$HOST_UID:$HOST_GID" "$TARGET_DIR"
  '

cp "$TARGET_DIR/release/libxlsx_node.so" "$OUT"
echo "built $OUT"
