#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT"

command -v podman >/dev/null || {
  echo "error: podman is required" >&2
  exit 1
}

# Locate nFPM in PATH, GOBIN, or GOPATH/bin.
NFPM_BIN=$(command -v nfpm 2>/dev/null || true)
if [[ -z "$NFPM_BIN" ]] && command -v go >/dev/null 2>&1; then
  GO_BIN=$(go env GOBIN 2>/dev/null || true)
  if [[ -z "$GO_BIN" ]]; then
    GO_PATH=$(go env GOPATH 2>/dev/null || true)
    [[ -n "$GO_PATH" ]] && GO_BIN="$GO_PATH/bin"
  fi
  [[ -n "${GO_BIN:-}" && -x "$GO_BIN/nfpm" ]] && NFPM_BIN="$GO_BIN/nfpm"
fi
[[ -n "$NFPM_BIN" ]] || {
  echo "error: nfpm is required; install it with:" >&2
  echo "  go install github.com/goreleaser/nfpm/v2/cmd/nfpm@latest" >&2
  exit 1
}

MAINTAINER=${MAINTAINER:-pulpul-s}
VERSION=${VERSION:-$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n1)}
RELEASE=${RELEASE:-1}

case "${NFPM_ARCH:-$(uname -m)}" in
  x86_64|amd64) NFPM_ARCH=amd64 ;;
  aarch64|arm64) NFPM_ARCH=arm64 ;;
  *)
    echo "error: set NFPM_ARCH to an nFPM architecture name" >&2
    exit 1
    ;;
esac
export VERSION RELEASE NFPM_ARCH MAINTAINER

IMAGE=${DEBIAN_IMAGE:-docker.io/library/debian:12-slim}
CONTAINER_NAME="hwall-deb-build-$$-$RANDOM"
WORK_DIR=$(mktemp -d "${TMPDIR:-/tmp}/hwall-deb-package.XXXXXXXX")
STAGE_DIR="$WORK_DIR/stage"
TARGET_DIR="$WORK_DIR/cargo-target"
NFPM_WORK_DIR="$WORK_DIR/nfpm-work"
PACKAGE_DIR="$WORK_DIR/package"
FINAL_DIR="$ROOT/dist/deb"

mkdir -p \
  "$STAGE_DIR" "$TARGET_DIR" "$PACKAGE_DIR" \
  "$NFPM_WORK_DIR/packaging/nfpm" "$NFPM_WORK_DIR/dist"

remove_work_dir() {
  [[ -e "$WORK_DIR" ]] || return 0

  # Rootless container storage and bind mounts can contain subordinate-UID
  # files. Entering Podman's user namespace can remove those safely.
  podman unshare rm -rf -- "$WORK_DIR" >/dev/null 2>&1 || {
    chmod -R u+rwX "$WORK_DIR" >/dev/null 2>&1 || true
    rm -rf -- "$WORK_DIR" >/dev/null 2>&1 || true
  }
}

cleanup() {
  local status=$?
  trap - EXIT INT TERM HUP

  podman rm --force "$CONTAINER_NAME" >/dev/null 2>&1 || true

  # The requested default is zero retained build images. This deliberately
  # removes the Debian image even when it existed before this invocation.
  podman image rm --force "$IMAGE" >/dev/null 2>&1 || true

  remove_work_dir
  exit "$status"
}
trap cleanup EXIT INT TERM HUP

cp "$ROOT/packaging/nfpm/hwall-deb.yaml" \
  "$NFPM_WORK_DIR/packaging/nfpm/hwall-deb.yaml"
ln -s "$STAGE_DIR" "$NFPM_WORK_DIR/dist/deb-root"

podman run \
  --name "$CONTAINER_NAME" \
  --rm \
  --pull=always \
  --network=host \
  --security-opt label=disable \
  --volume "$ROOT:/src:ro" \
  --volume "$STAGE_DIR:/stage:rw" \
  --volume "$TARGET_DIR:/target:rw" \
  --workdir /src \
  --env CARGO_TARGET_DIR=/target \
  "$IMAGE" \
  bash -euxo pipefail -c '
    apt-get update
    DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      ca-certificates curl build-essential pkg-config libgtk-4-dev file binutils
    curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --profile minimal --default-toolchain 1.97.1
    . "$HOME/.cargo/env"
    make DESTDIR=/stage PREFIX=/usr CARGO_TARGET_DIR=/target install
    file /target/release/hwall /target/release/hwall-cli
    ldd /target/release/hwall
    ldd /target/release/hwall-cli
  '

(
  cd "$NFPM_WORK_DIR"
  "$NFPM_BIN" package \
    --config packaging/nfpm/hwall-deb.yaml \
    --packager deb \
    --target "$PACKAGE_DIR/"
)

PACKAGE_FILE=$(find "$PACKAGE_DIR" -maxdepth 1 -type f -name '*.deb' -print -quit)
[[ -n "$PACKAGE_FILE" ]] || {
  echo "error: nFPM did not produce a .deb package" >&2
  exit 1
}

rm -rf -- "$FINAL_DIR"
mkdir -p "$FINAL_DIR"
PACKAGE_NAME=$(basename -- "$PACKAGE_FILE")
mv -- "$PACKAGE_FILE" "$FINAL_DIR/$PACKAGE_NAME"

echo "Debian package written to: $FINAL_DIR/$PACKAGE_NAME"
echo "Cleanup complete: container, Debian image, and temporary build files removed."
