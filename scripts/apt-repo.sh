#!/usr/bin/env bash
# Builds a signed APT repository from .deb packages, for GitHub Pages (run by the release workflow):
#   scripts/apt-repo.sh OUT_DIR URL DEB...
# URL is where OUT_DIR will be served. It signs with SIGNING_KEY (a key ID or fingerprint), or
# with the first secret key in the keyring. OUT_DIR gets:
#   dists/stable/main/binary-<arch>/Packages   the index, signed as dists/stable/InRelease
#   pool/main/c/clipy/*.deb
#   clipy.sources   the apt source with the public key in it: the one file users need, as
#                   sudo wget -qO /etc/apt/sources.list.d/clipy.sources URL/clipy.sources
#   clipy.gpg       the public key on its own
set -euo pipefail
umask 022

[ $# -ge 3 ] || { echo "usage: $0 OUT_DIR URL DEB..." >&2; exit 2; }
OUT=$1
URL=${2%/}
shift 2

SUITE=stable
COMPONENT=main
POOL=pool/$COMPONENT/c/clipy
KEY=${SIGNING_KEY:-$(gpg --list-secret-keys --with-colons | awk -F: '$1 == "fpr" { print $10; exit }')}

[ -n "$KEY" ] || { echo "No secret key to sign with." >&2; exit 1; }
[ -z "$(ls -A "$OUT" 2>/dev/null)" ] || { echo "$OUT is not empty." >&2; exit 1; }

mkdir -p "$OUT/$POOL"
install -m644 "$@" "$OUT/$POOL/"
cd "$OUT"

ARCHS=$(for deb in "$POOL"/*.deb; do dpkg-deb -f "$deb" Architecture; done | sort -u | xargs)

for arch in $ARCHS; do
    dir=dists/$SUITE/$COMPONENT/binary-$arch
    mkdir -p "$dir"
    dpkg-scanpackages --arch "$arch" --multiversion pool >"$dir/Packages" 2>/dev/null
    gzip -9kn "$dir/Packages"
done

# apt-ftparchive would write this too, but it is not on every machine (apt-utils), and apt needs
# only these fields and the SHA256 list.
{
    cat <<EOF
Origin: Clipy
Label: Clipy
Suite: $SUITE
Codename: $SUITE
Date: $(LC_ALL=C date -u '+%a, %d %b %Y %H:%M:%S UTC')
Architectures: $ARCHS
Components: $COMPONENT
Description: Clipy for Linux
SHA256:
EOF
    (cd "dists/$SUITE" && find "$COMPONENT" -type f | sort | while read -r f; do
        printf ' %s %8d %s\n' "$(sha256sum "$f" | cut -d' ' -f1)" "$(stat -c%s "$f")" "$f"
    done)
} >"dists/$SUITE/Release"

sign() { gpg --batch --yes --local-user "$KEY" --digest-algo SHA512 "$@"; }
sign --clearsign -o "dists/$SUITE/InRelease" "dists/$SUITE/Release"
sign --detach-sign --armor -o "dists/$SUITE/Release.gpg" "dists/$SUITE/Release"

gpg --export --export-options export-minimal "$KEY" >clipy.gpg
# Signed-By can hold the key itself (apt 2.4 and later, so Ubuntu 22.04 on), which saves users
# a second download. Its lines are indented, and an empty line is written as " .".
{
    cat <<EOF
Types: deb
URIs: $URL
Suites: $SUITE
Components: $COMPONENT
Architectures: $ARCHS
Signed-By:
EOF
    gpg --armor --export --export-options export-minimal "$KEY" | sed 's/^$/./; s/^/ /'
} >clipy.sources

find . -type f | sort
