#!/usr/bin/env bash
# Run cargo inside the Ubuntu dev container.
#   scripts/dev.sh image            build the dev image (UBUNTU=22.04 for the old-LTS image)
#   scripts/dev.sh check            fmt --check, clippy -D warnings, test
#   scripts/dev.sh it               headless integration tests (Xvfb + D-Bus)
#   scripts/dev.sh shell            interactive shell
#   scripts/dev.sh <any command>    e.g. scripts/dev.sh cargo build --release
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
UBUNTU="${UBUNTU:-24.04}"
IMAGE="clipy-dev:${UBUNTU}"
VOL="clipy-${UBUNTU//./}"

run() {
    local tty=()
    [ -t 0 ] && tty=(-it)
    docker run --rm ${tty[@]+"${tty[@]}"} \
        -v "$ROOT":/src \
        -v "${VOL}-registry":/usr/local/cargo/registry \
        -v "${VOL}-target":/src/target \
        -e CARGO_TERM_COLOR=always \
        "$IMAGE" "$@"
}

case "${1:-check}" in
    image)
        docker build --build-arg UBUNTU="$UBUNTU" -t "$IMAGE" -f "$ROOT/docker/Dockerfile.dev" "$ROOT/docker"
        ;;
    check)
        run bash -c 'cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test'
        ;;
    it)
        run bash scripts/it-xvfb.sh
        ;;
    shell)
        run bash
        ;;
    *)
        run "$@"
        ;;
esac
