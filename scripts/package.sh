#!/usr/bin/env bash
# Builds the packages to share, into dist/ (run through `make dist`):
#   clipy_<version>_<arch>.deb               sudo apt install ./clipy_<version>_<arch>.deb
#   clipy-<version>-linux-<machine>.tar.gz   unpack, then ./install.sh
#   SHA256SUMS and INSTALL.md
# The packages need the glibc the binary was linked against, so build on the oldest release
# you target. A build on Ubuntu 26.04 needs glibc 2.34, which Ubuntu 22.04 and Mint 21 have.
set -euo pipefail
umask 022

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

BIN=target/release/clipy
APP_ID=uz.osoncode.Clipy
UUID=clipy@osoncode.uz
ICON_SIZES=(16 32 128 256 512)
DIST=dist

field() { sed -n "s/^$1 = \"\(.*\)\"/\1/p" Cargo.toml | head -1; }
VERSION="$(field version)"
DESCRIPTION="$(field description)"
MAINTAINER="$(sed -n 's/^authors = \["\([^"]*\)".*/\1/p' Cargo.toml)"
ARCH="$(dpkg --print-architecture)"
MACHINE="$(uname -m)"

[ -x "$BIN" ] || { echo "Run 'make build' first." >&2; exit 1; }
grep -q "clipy_${VERSION}_$ARCH.deb" INSTALL.md ||
    { echo "INSTALL.md names another version; update it to $VERSION." >&2; exit 1; }

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# The newest glibc symbol version the binary needs. Weak references do not count: Rust's std
# links newer functions weakly and falls back when they are missing.
glibc_floor() {
    objdump -T "$BIN" | awk '/\*UND\*/ && $2 != "w"' | grep -o 'GLIBC_[0-9.]*' \
        | sed 's/^GLIBC_//' | sort -uV | tail -1
}

# Lays out the app's files under $1 the way they go under /usr: bin/ and share/.
stage() {
    local prefix=$1 ext="$1/share/gnome-shell/extensions/$UUID"
    install -Dm755 "$BIN" "$prefix/bin/clipy"
    install -Dm644 "resources/$APP_ID.desktop" "$prefix/share/applications/$APP_ID.desktop"
    for s in "${ICON_SIZES[@]}"; do
        install -Dm644 "resources/icons/app_$s.png" "$prefix/share/icons/hicolor/${s}x$s/apps/clipy.png"
    done
    install -d "$ext"
    install -m644 "gnome-extension/$UUID"/*.js "gnome-extension/$UUID/metadata.json" \
        "gnome-extension/$UUID/stylesheet.css" "$ext/"
}

build_deb() {
    local root="$WORK/deb" out="$DIST/clipy_${VERSION}_$ARCH.deb"
    stage "$root/usr"
    install -Dm644 -t "$root/usr/share/doc/clipy" LICENSE resources/icons/LICENSE-Clipy README.md
    install -d "$root/DEBIAN"
    # The pre-t64 package names: Ubuntu 24.04 and later provide them from the …t64 packages, so
    # the one package installs on 22.04 through 26.04. The AppIndicator library is loaded at
    # run time, so it is not in the binary's NEEDED list.
    cat >"$root/DEBIAN/control" <<EOF
Package: clipy
Version: $VERSION
Architecture: $ARCH
Maintainer: $MAINTAINER
Installed-Size: $(du -sk --exclude=DEBIAN "$root" | cut -f1)
Depends: libc6 (>= $(glibc_floor)), libgcc-s1, libgtk-3-0 (>= 3.22), libglib2.0-0 (>= 2.56), libdbus-1-3, libayatana-appindicator3-1
Recommends: xdotool
Section: utils
Priority: optional
Description: $DESCRIPTION
 Clipboard history and snippets in a panel menu and a menu at the mouse
 pointer. On GNOME Wayland, log out and back in after installing, then enable
 the bundled GNOME Shell extension: gnome-extensions enable $UUID
EOF
    # xz rather than dpkg's zstd default, which older Debian dpkg cannot read.
    dpkg-deb --root-owner-group -Zxz --build "$root" "$out" >/dev/null
    echo "$out"
}

build_tar() {
    local name="clipy-$VERSION-linux-$MACHINE"
    local dir="$WORK/$name" out="$DIST/$name.tar.gz"
    stage "$dir"
    install -m644 -t "$dir" README.md INSTALL.md LICENSE resources/icons/LICENSE-Clipy
    install -m755 scripts/dist-install.sh "$dir/install.sh"
    tar -C "$WORK" --owner=0 --group=0 --sort=name -czf "$out" "$name"
    echo "$out"
}

mkdir -p "$DIST"
built=("$(build_deb)" "$(build_tar)")
install -m644 INSTALL.md "$DIST/"
(cd "$DIST" && sha256sum "${built[@]##*/}" >SHA256SUMS)
printf '%s\n' "${built[@]}" "$DIST/SHA256SUMS" "$DIST/INSTALL.md"
