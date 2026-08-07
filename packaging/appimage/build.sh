#!/usr/bin/env bash

# build the appimage on debian 12 to retain glibc compatibility with older systems.
#
# we build a newer gtk than the minimum required gtk 4.8, because gtk 4.8 can cause
# problems with kde plasma. gtk 4.22 and its required dependencies are built locally.
#
# fontconfig is also built because debian 12 ships an older version that can be
# incompatible with fontconfig configuration files found on newer distributions.

set -euo pipefail

ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$ROOT"

: "${HWALL_VERSION:?HWALL_VERSION is required}"

case "$(uname -m)" in
  x86_64) APPIMAGE_ARCH=x86_64 ;;
  aarch64|arm64) APPIMAGE_ARCH=aarch64 ;;
  *)
    echo "Unsupported AppImage architecture: $(uname -m)" >&2
    exit 1
    ;;
esac

WORK_DIR=${RUNNER_TEMP:-/tmp}/hwall-appimage
PREFIX=$WORK_DIR/runtime
SOURCES=$WORK_DIR/sources
BUILDS=$WORK_DIR/builds
DOWNLOADS=$WORK_DIR/downloads
APPDIR=$WORK_DIR/HWall.AppDir
TOOLS=$WORK_DIR/tools
TARGET=$WORK_DIR/target
OUTPUT=$ROOT/dist/appimage
JOBS=${JOBS:-$(nproc)}

GLIB_VERSION=2.84.4
GLIB_SHA256=8a9ea10943c36fc117e253f80c91e477b673525ae45762942858aef57631bb90
WAYLAND_VERSION=1.24.0
WAYLAND_SHA256=82892487a01ad67b334eca83b54317a7c86a03a89cfadacfef5211f11a5d0536
WAYLAND_PROTOCOLS_VERSION=1.48
WAYLAND_PROTOCOLS_SHA256=398036ac0eb6484982ddbde7ff86848d753231f9cdeeae983f06b52946625aa1
FONTCONFIG_VERSION=2.18.2
FONTCONFIG_SHA256=cf8e6576ef0484c15079bdaf77cd9c51c464df5365814ada4d3ee7331ea31eb5
CAIRO_VERSION=1.18.4
CAIRO_SHA256=445ed8208a6e4823de1226a74ca319d3600e83f6369f99b14265006599c32ccb
HARFBUZZ_VERSION=10.4.0
HARFBUZZ_SHA256=480b6d25014169300669aa1fc39fb356c142d5028324ea52b3a27648b9beaad8
PANGO_VERSION=1.56.4
PANGO_SHA256=17065e2fcc5f5a5bdbffc884c956bfc7c451a96e8c4fb2f8ad837c6413cb5a01
GTK_VERSION=4.22.4
GTK_SHA256=51bd9f60c7d23a665a556c7364c21fb2e4e282566b3e7e092455e8f910330893

rm -rf "$WORK_DIR" "$OUTPUT"
mkdir -p "$PREFIX" "$SOURCES" "$BUILDS" "$DOWNLOADS" "$APPDIR/usr/lib" "$TOOLS" "$OUTPUT"

export PATH="$PREFIX/bin:$PATH"
export PKG_CONFIG_PATH="$PREFIX/lib/pkgconfig:$PREFIX/share/pkgconfig"
export CMAKE_PREFIX_PATH="$PREFIX"
export LD_LIBRARY_PATH="$PREFIX/lib"
export CPPFLAGS="-I$PREFIX/include"
export LDFLAGS="-L$PREFIX/lib -Wl,-rpath-link,$PREFIX/lib"
export CARGO_TARGET_DIR="$TARGET"

fetch() {
  local name=$1 url=$2 sha256=$3
  curl --fail --location --retry 4 --output "$DOWNLOADS/$name" "$url"
  printf '%s  %s\n' "$sha256" "$DOWNLOADS/$name" | sha256sum --check --status
}

extract() {
  local archive=$1 directory=$2
  tar -xf "$DOWNLOADS/$archive" -C "$SOURCES"
  test -d "$SOURCES/$directory"
}

build_meson() {
  local name=$1 source=$2
  shift 2
  meson setup "$BUILDS/$name" "$source" \
    --prefix="$PREFIX" \
    --libdir=lib \
    --buildtype=release \
    --wrap-mode=nofallback \
    -Ddefault_library=shared \
    "$@"
  meson compile -C "$BUILDS/$name" -j "$JOBS"
  meson install -C "$BUILDS/$name"
}

install_licenses() {
  local component=$1 source=$2 destination="$APPDIR/usr/share/licenses/hwall-appimage/$1"
  local license relative

  mkdir -p "$destination"
  while IFS= read -r -d '' license; do
    relative=${license#"$source"/}
    install -Dm644 "$license" "$destination/$relative"
  done < <(
    find "$source" -maxdepth 2 -type f \
      \( -iname 'copying*' -o -iname 'license*' \) -print0
  )
}

fetch "glib-$GLIB_VERSION.tar.xz" \
  "https://download.gnome.org/sources/glib/2.84/glib-$GLIB_VERSION.tar.xz" \
  "$GLIB_SHA256"
fetch "wayland-$WAYLAND_VERSION.tar.xz" \
  "https://gitlab.freedesktop.org/wayland/wayland/-/releases/$WAYLAND_VERSION/downloads/wayland-$WAYLAND_VERSION.tar.xz" \
  "$WAYLAND_SHA256"
fetch "wayland-protocols-$WAYLAND_PROTOCOLS_VERSION.tar.xz" \
  "https://gitlab.freedesktop.org/wayland/wayland-protocols/-/releases/$WAYLAND_PROTOCOLS_VERSION/downloads/wayland-protocols-$WAYLAND_PROTOCOLS_VERSION.tar.xz" \
  "$WAYLAND_PROTOCOLS_SHA256"
fetch "fontconfig-$FONTCONFIG_VERSION.tar.xz" \
  "https://gitlab.freedesktop.org/api/v4/projects/890/packages/generic/fontconfig/$FONTCONFIG_VERSION/fontconfig-$FONTCONFIG_VERSION.tar.xz" \
  "$FONTCONFIG_SHA256"
fetch "cairo-$CAIRO_VERSION.tar.xz" \
  "https://cairographics.org/releases/cairo-$CAIRO_VERSION.tar.xz" \
  "$CAIRO_SHA256"
fetch "harfbuzz-$HARFBUZZ_VERSION.tar.xz" \
  "https://github.com/harfbuzz/harfbuzz/releases/download/$HARFBUZZ_VERSION/harfbuzz-$HARFBUZZ_VERSION.tar.xz" \
  "$HARFBUZZ_SHA256"
fetch "pango-$PANGO_VERSION.tar.xz" \
  "https://download.gnome.org/sources/pango/1.56/pango-$PANGO_VERSION.tar.xz" \
  "$PANGO_SHA256"
fetch "gtk-$GTK_VERSION.tar.xz" \
  "https://download.gnome.org/sources/gtk/4.22/gtk-$GTK_VERSION.tar.xz" \
  "$GTK_SHA256"

extract "glib-$GLIB_VERSION.tar.xz" "glib-$GLIB_VERSION"
build_meson glib "$SOURCES/glib-$GLIB_VERSION" \
  -Dtests=false \
  -Dinstalled_tests=false \
  -Dintrospection=disabled \
  -Ddocumentation=false \
  -Dman-pages=disabled \
  -Dnls=disabled \
  -Dselinux=disabled \
  -Dsysprof=disabled

extract "wayland-$WAYLAND_VERSION.tar.xz" "wayland-$WAYLAND_VERSION"
build_meson wayland "$SOURCES/wayland-$WAYLAND_VERSION" \
  -Ddocumentation=false \
  -Dtests=false \
  -Ddtd_validation=false

extract "wayland-protocols-$WAYLAND_PROTOCOLS_VERSION.tar.xz" \
  "wayland-protocols-$WAYLAND_PROTOCOLS_VERSION"
build_meson wayland-protocols "$SOURCES/wayland-protocols-$WAYLAND_PROTOCOLS_VERSION" \
  -Dtests=false

extract "fontconfig-$FONTCONFIG_VERSION.tar.xz" "fontconfig-$FONTCONFIG_VERSION"
build_meson fontconfig "$SOURCES/fontconfig-$FONTCONFIG_VERSION" \
  -Ddoc=disabled \
  -Dtests=disabled \
  -Dtools=disabled \
  -Dcache-build=disabled \
  -Dxml-backend=expat \
  -Dfontations=disabled \
  -Dnls=disabled

extract "cairo-$CAIRO_VERSION.tar.xz" "cairo-$CAIRO_VERSION"
build_meson cairo "$SOURCES/cairo-$CAIRO_VERSION" \
  -Dtests=disabled \
  -Dgtk2-utils=disabled \
  -Dglib=enabled \
  -Dfontconfig=enabled \
  -Dspectre=disabled \
  -Dsymbol-lookup=disabled

extract "harfbuzz-$HARFBUZZ_VERSION.tar.xz" "harfbuzz-$HARFBUZZ_VERSION"
build_meson harfbuzz "$SOURCES/harfbuzz-$HARFBUZZ_VERSION" \
  -Dtests=disabled \
  -Ddocs=disabled \
  -Dbenchmark=disabled \
  -Dicu=disabled \
  -Dgraphite=disabled \
  -Dglib=enabled \
  -Dgobject=enabled \
  -Dfreetype=enabled \
  -Dcairo=enabled \
  -Dintrospection=disabled

extract "pango-$PANGO_VERSION.tar.xz" "pango-$PANGO_VERSION"
build_meson pango "$SOURCES/pango-$PANGO_VERSION" \
  -Dintrospection=disabled \
  -Ddocumentation=false \
  -Dbuild-testsuite=false \
  -Dbuild-examples=false \
  -Dfontconfig=enabled \
  -Dfreetype=enabled \
  -Dcairo=enabled

extract "gtk-$GTK_VERSION.tar.xz" "gtk-$GTK_VERSION"
build_meson gtk "$SOURCES/gtk-$GTK_VERSION" \
  -Dwayland-backend=true \
  -Dx11-backend=true \
  -Dbroadway-backend=false \
  -Dprint-cups=disabled \
  -Dprint-cpdb=disabled \
  -Dcolord=disabled \
  -Dmedia-gstreamer=disabled \
  -Dvulkan=disabled \
  -Dcloudproviders=disabled \
  -Dtracker=disabled \
  -Dsysprof=disabled \
  -Dintrospection=disabled \
  -Ddocumentation=false \
  -Dman-pages=false \
  -Dbuild-demos=false \
  -Dbuild-tests=false \
  -Dbuild-testsuite=false \
  -Dbuild-examples=false

test "$(pkg-config --modversion gtk4)" = "$GTK_VERSION"
test "$(pkg-config --modversion fontconfig)" = "$FONTCONFIG_VERSION"
printf 'GTK %s, Fontconfig %s\n' "$GTK_VERSION" "$FONTCONFIG_VERSION"

make DESTDIR="$APPDIR" PREFIX=/usr install-gui
install -Dm644 LICENSE "$APPDIR/usr/share/licenses/hwall/LICENSE"

stage_runtime() {
  cp -a "$PREFIX/lib/." "$APPDIR/usr/lib/"
  cp -a "$PREFIX/share/." "$APPDIR/usr/share/"
  if test -d "$PREFIX/etc"; then
    mkdir -p "$APPDIR/usr/etc"
    cp -a "$PREFIX/etc/." "$APPDIR/usr/etc/"
  fi
  rm -rf \
    "$APPDIR/usr/lib/pkgconfig" \
    "$APPDIR/usr/share/aclocal" \
    "$APPDIR/usr/share/doc" \
    "$APPDIR/usr/share/man" \
    "$APPDIR/usr/share/pkgconfig" \
    "$APPDIR/usr/share/wayland" \
    "$APPDIR/usr/share/wayland-protocols"
  find "$APPDIR/usr/lib" -type f \( -name '*.a' -o -name '*.la' \) -delete
}

stage_runtime
for component in \
  "glib-$GLIB_VERSION" \
  "wayland-$WAYLAND_VERSION" \
  "wayland-protocols-$WAYLAND_PROTOCOLS_VERSION" \
  "fontconfig-$FONTCONFIG_VERSION" \
  "cairo-$CAIRO_VERSION" \
  "harfbuzz-$HARFBUZZ_VERSION" \
  "pango-$PANGO_VERSION" \
  "gtk-$GTK_VERSION"; do
  install_licenses "$component" "$SOURCES/$component"
done

mkdir -p "$APPDIR/usr/share/icons"
cp -a /usr/share/icons/Adwaita "$APPDIR/usr/share/icons/"

svg_loader=$(find /usr/lib -type f \
  \( -name 'libpixbufloader-svg.so' -o -name 'libpixbufloader_svg.so' \) \
  -path '*/gdk-pixbuf-2.0/*/loaders/*' -print -quit)
test -n "$svg_loader"

linuxdeploy="$TOOLS/linuxdeploy.AppImage"
appimagetool="$TOOLS/appimagetool.AppImage"
curl --fail --location --retry 4 \
  --output "$linuxdeploy" \
  "https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-$APPIMAGE_ARCH.AppImage"
curl --fail --location --retry 4 \
  --output "$appimagetool" \
  "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-$APPIMAGE_ARCH.AppImage"
chmod +x "$linuxdeploy" "$appimagetool"

APPIMAGE_EXTRACT_AND_RUN=1 LD_LIBRARY_PATH="$PREFIX/lib" "$linuxdeploy" \
  --appdir "$APPDIR" \
  --executable "$APPDIR/usr/bin/hwall" \
  --library "$svg_loader" \
  --desktop-file packaging/io.github.hwall.HWall.desktop \
  --icon-file packaging/icons/hicolor/scalable/apps/io.github.hwall.HWall.svg

stage_runtime
"$PREFIX/bin/glib-compile-schemas" "$APPDIR/usr/share/glib-2.0/schemas"

staged_svg_loader=$(find "$APPDIR/usr/lib" -type f \
  \( -name 'libpixbufloader-svg.so' -o -name 'libpixbufloader_svg.so' \) \
  -print -quit)
test -n "$staged_svg_loader"
mkdir -p "$APPDIR/usr/lib/gdk-pixbuf-2.0"
query_loaders=$(pkg-config --variable=gdk_pixbuf_query_loaders gdk-pixbuf-2.0)
if ! test -x "$query_loaders"; then
  echo "gdk-pixbuf-query-loaders not found: $query_loaders" >&2
  exit 1
fi
GDK_PIXBUF_MODULEDIR= LD_LIBRARY_PATH="$APPDIR/usr/lib" \
  "$query_loaders" "$staged_svg_loader" \
  | sed "s|$APPDIR|@APPDIR@|g" \
  > "$APPDIR/usr/lib/gdk-pixbuf-2.0/loaders.cache.in"

rm -f "$APPDIR/AppRun"
cat > "$APPDIR/AppRun" <<'APPRUN'
#!/bin/sh
set -eu

APPDIR=${APPDIR:-$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)}
export APPDIR
export LD_LIBRARY_PATH="$APPDIR/usr/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export XDG_DATA_DIRS="$APPDIR/usr/share${XDG_DATA_DIRS:+:$XDG_DATA_DIRS}"

if [ -r /etc/fonts/fonts.conf ]; then
  export FONTCONFIG_PATH=/etc/fonts
  export FONTCONFIG_FILE=/etc/fonts/fonts.conf
else
  export FONTCONFIG_PATH="$APPDIR/usr/etc/fonts"
  export FONTCONFIG_FILE="$APPDIR/usr/etc/fonts/fonts.conf"
fi

cache=${XDG_RUNTIME_DIR:-${TMPDIR:-/tmp}}/hwall-gdk-pixbuf-$(id -u).cache
cache_tmp=$cache.$$
umask 077
sed "s|@APPDIR@|$APPDIR|g" \
  "$APPDIR/usr/lib/gdk-pixbuf-2.0/loaders.cache.in" > "$cache_tmp"
mv -f "$cache_tmp" "$cache"
export GDK_PIXBUF_MODULE_FILE="$cache"

exec "$APPDIR/usr/bin/hwall" "$@"
APPRUN
chmod +x "$APPDIR/AppRun"

ldd_output=$(LD_LIBRARY_PATH="$APPDIR/usr/lib" ldd "$APPDIR/usr/bin/hwall")
printf '%s\n' "$ldd_output"
grep -Eq "$APPDIR/usr/(bin/\.\./)?lib/libgtk-4\.so\.1" <<<"$ldd_output"
grep -Eq "$APPDIR/usr/(bin/\.\./)?lib/libfontconfig\.so\.1" <<<"$ldd_output"
! grep -q 'not found' <<<"$ldd_output"

output_file="$OUTPUT/HWall-$HWALL_VERSION-$APPIMAGE_ARCH.AppImage"
env -u LD_LIBRARY_PATH \
  ARCH="$APPIMAGE_ARCH" \
  VERSION="$HWALL_VERSION" \
  APPIMAGE_EXTRACT_AND_RUN=1 \
  "$appimagetool" "$APPDIR" "$output_file"
chmod +x "$output_file"
file "$output_file"
