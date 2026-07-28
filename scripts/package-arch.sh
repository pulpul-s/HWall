#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT"

command -v podman >/dev/null || {
  echo "error: podman is required" >&2
  exit 1
}

MAINTAINER=${MAINTAINER:-pulpul-s}
VERSION=${VERSION:-$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n1)}
RELEASE=${RELEASE:-1}
RUST_TOOLCHAIN=${RUST_TOOLCHAIN:-$(sed -n 's/^channel = "\([^"]*\)"/\1/p' rust-toolchain.toml | head -n1)}

[[ -n "$VERSION" ]] || {
  echo "error: could not determine VERSION from Cargo.toml" >&2
  exit 1
}
[[ -n "$RUST_TOOLCHAIN" ]] || {
  echo "error: could not determine Rust toolchain from rust-toolchain.toml" >&2
  exit 1
}

case "${ARCH_PACKAGE_ARCH:-$(uname -m)}" in
  x86_64|amd64) ARCH_PACKAGE_ARCH=x86_64 ;;
  aarch64|arm64) ARCH_PACKAGE_ARCH=aarch64 ;;
  *)
    echo "error: unsupported architecture; set ARCH_PACKAGE_ARCH explicitly" >&2
    exit 1
    ;;
esac

IMAGE=${ARCH_IMAGE:-docker.io/library/archlinux:base}
CONTAINER_NAME="hwall-arch-build-$$-$RANDOM"
WORK_DIR=$(mktemp -d "${TMPDIR:-/tmp}/hwall-arch-package.XXXXXXXX")
STAGE_DIR="$WORK_DIR/stage"
TARGET_DIR="$WORK_DIR/cargo-target"
PACKAGE_WORK_DIR="$WORK_DIR/package-work"
PACKAGE_DIR="$WORK_DIR/package"
FINAL_DIR="$ROOT/dist/arch"

mkdir -p \
  "$STAGE_DIR" "$TARGET_DIR" "$PACKAGE_DIR" \
  "$PACKAGE_WORK_DIR/payload"

remove_work_dir() {
  [[ -e "$WORK_DIR" ]] || return 0

  # Files written by a rootless container can use subordinate UID mappings.
  # podman unshare removes them from the matching user namespace.
  podman unshare rm -rf -- "$WORK_DIR" >/dev/null 2>&1 || {
    chmod -R u+rwX "$WORK_DIR" >/dev/null 2>&1 || true
    rm -rf -- "$WORK_DIR" >/dev/null 2>&1 || true
  }
}

cleanup() {
  local status=$?
  trap - EXIT INT TERM HUP

  podman rm --force "$CONTAINER_NAME" >/dev/null 2>&1 || true

  # Keep no Arch base image after the build. This also removes the image when
  # it existed before this invocation, matching the zero-retained-image policy.
  podman image rm --force "$IMAGE" >/dev/null 2>&1 || true

  remove_work_dir
  exit "$status"
}
trap cleanup EXIT INT TERM HUP

sed \
  -e "s/@VERSION@/$VERSION/g" \
  -e "s/@RELEASE@/$RELEASE/g" \
  -e "s/@ARCH@/$ARCH_PACKAGE_ARCH/g" \
  "$ROOT/packaging/arch/PKGBUILD.in" > "$PACKAGE_WORK_DIR/PKGBUILD"

podman run \
  --name "$CONTAINER_NAME" \
  --rm \
  --pull=always \
  --network=host \
  --security-opt label=disable \
  --volume "$ROOT:/src:ro" \
  --volume "$STAGE_DIR:/stage:rw" \
  --volume "$TARGET_DIR:/target:rw" \
  --volume "$PACKAGE_WORK_DIR:/package-work:rw" \
  --volume "$PACKAGE_DIR:/out:rw" \
  --workdir /src \
  --env "HWALL_MAINTAINER=$MAINTAINER" \
  --env "HWALL_RUST_TOOLCHAIN=$RUST_TOOLCHAIN" \
  "$IMAGE" \
  bash -euxo pipefail -c '
    pacman -Syu --noconfirm --needed \
      base-devel rustup gtk4 pkgconf file binutils

    useradd --create-home --uid 1000 builder
    chown -R builder:builder /stage /target /package-work

    runuser -u builder -- env \
      HOME=/home/builder \
      CARGO_HOME=/home/builder/.cargo \
      RUSTUP_HOME=/home/builder/.rustup \
      CARGO_TARGET_DIR=/target \
      PACKAGER="$HWALL_MAINTAINER" \
      HWALL_RUST_TOOLCHAIN="$HWALL_RUST_TOOLCHAIN" \
      bash -euxo pipefail -c '\''
        rustup toolchain install "$HWALL_RUST_TOOLCHAIN" --profile minimal
        cd /src
        make DESTDIR=/stage PREFIX=/usr CARGO_TARGET_DIR=/target install

        file /target/release/hwall /target/release/hwall-cli
        ldd /target/release/hwall
        ldd /target/release/hwall-cli

        cp -a /stage/. /package-work/payload/
        cd /package-work
        makepkg --cleanbuild --force --noconfirm
      '\''

    cp /package-work/*.pkg.tar.zst /out/
    chown 0:0 /out/*.pkg.tar.zst
  '

PACKAGE_FILE="$PACKAGE_DIR/hwall-$VERSION-$RELEASE-$ARCH_PACKAGE_ARCH.pkg.tar.zst"
[[ -f "$PACKAGE_FILE" ]] || {
  echo "error: makepkg did not produce the expected Arch package:" >&2
  echo "  $PACKAGE_FILE" >&2
  exit 1
}

rm -rf -- "$FINAL_DIR"
mkdir -p "$FINAL_DIR"
PACKAGE_NAME=$(basename -- "$PACKAGE_FILE")
mv -- "$PACKAGE_FILE" "$FINAL_DIR/$PACKAGE_NAME"

printf 'Arch package written to: %s\n' "$FINAL_DIR/$PACKAGE_NAME"
printf 'Cleanup complete: container, Arch image, Rust toolchain, and temporary build files removed.\n'
