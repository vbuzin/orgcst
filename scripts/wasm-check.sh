#!/bin/sh
# Host vs wasm32-wasip1 fingerprint. A missing tool is a failed run.
# The compile-only wasm32-unknown-unknown library check is not a substitute.
set -eu

ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
MANIFEST="$ROOT/Cargo.toml"
TARGET="$ROOT/target"

if ! command -v wasmtime >/dev/null 2>&1; then
    printf '%s\n' \
        'wasmtime not found on PATH.' \
        'Install it, for example: brew install wasmtime' \
        'The compile-only check (cargo check --target wasm32-unknown-unknown) is not a substitute.' \
        >&2
    exit 1
fi

if ! rustup target list --installed 2>/dev/null | grep -qx 'wasm32-wasip1'; then
    printf '%s\n' 'rustup target add wasm32-wasip1' >&2
    exit 1
fi

cargo build --manifest-path "$MANIFEST" --example wasm_behaviour
cargo build \
    --manifest-path "$MANIFEST" \
    \
    --example wasm_behaviour \
    --target wasm32-wasip1

host="$TARGET/debug/examples/wasm_behaviour"
wasm="$TARGET/wasm32-wasip1/debug/examples/wasm_behaviour.wasm"

host_out=$(mktemp)
wasm_out=$(mktemp)
trap 'rm -f "$host_out" "$wasm_out"' EXIT

"$host" >"$host_out"
wasmtime "$wasm" >"$wasm_out"
if ! diff -u "$host_out" "$wasm_out" >&2; then
    printf '%s\n' 'native and wasm32-wasip1 fingerprints differ' >&2
    exit 1
fi
