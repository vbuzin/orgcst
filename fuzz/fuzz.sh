#!/bin/sh
# libFuzzer for orgcst. A missing nightly is a failed run, not a skip.
set -eu

CORE=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
cd "$CORE"

if ! cargo +nightly -V >/dev/null 2>&1; then
    printf '%s\n' 'rustup toolchain install nightly' >&2
    exit 1
fi

if ! cargo +nightly fuzz --help >/dev/null 2>&1; then
    printf '%s\n' 'cargo install cargo-fuzz' >&2
    exit 1
fi

secs=${1:-60}
for target in parse apply roundtrip; do
    cargo +nightly fuzz run "$target" -- -max_total_time="$secs"
done
