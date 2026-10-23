#!/bin/sh
# One Org oracle: the pinned Docker image. A local Emacs is not consulted.
set -eu

ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
IMAGE=org-parity:10814296c776
TMP="$ROOT/parity/.tmp"
PIN="$ROOT/parity/ORG_VERSION"
BIN="$ROOT/parity/target/debug/org-parity"

record=0
verbose=0
mode=corpus
for arg in "$@"; do
    case "$arg" in
        --record-version) record=1 ;;
        --verbose) verbose=1 ;;
        priorities) mode=priorities ;;
        syntax-probe) mode=syntax-probe ;;
        *)
            printf 'unknown argument: %s\n' "$arg" >&2
            exit 1
            ;;
    esac
done

if ! docker info >/dev/null 2>&1; then
    printf '%s\n' \
        'docker daemon is not running' \
        'Start Docker (or Colima), then re-run parity/parity.sh' \
        'A local Emacs is not the oracle'
    exit 2
fi

if ! docker image inspect "$IMAGE" >/dev/null 2>&1; then
    docker build -t "$IMAGE" -f "$ROOT/parity/Dockerfile" "$ROOT/parity"
fi

cargo build --manifest-path "$ROOT/parity/Cargo.toml" >/dev/null

mkdir -p "$TMP"

# Paths inside the read-only mount. JSON comes back on stdout.
run_emacs() {
    docker run --rm -v "$ROOT":/work:ro -w /work --entrypoint emacs \
        "$IMAGE" \
        -Q --batch --load /work/parity/emacs/dump.el -- "$@"
}

rel() {
    case "$1" in
        "$ROOT"/*) printf '%s\n' "${1#"$ROOT"/}" ;;
        *) printf '%s\n' "$1" ;;
    esac
}

if [ "$mode" = "syntax-probe" ]; then
    run_emacs --syntax-probe
    exit 0
fi

if [ "$record" -eq 1 ]; then
    printf '\n' > "$TMP/version.org"
    printf '%s\n' '{"inline_task_min_level":null,"list_alphabetical":null,"todo":null,"link_types":null,"user_entities":null}' \
        > "$TMP/version.sidecar.json"
    run_emacs "$(rel "$TMP/version.org")" "$(rel "$TMP/version.sidecar.json")" \
        > "$TMP/version.json"
    "$BIN" record-version --emacs "$TMP/version.json" --out "$PIN"
fi

if [ "$mode" = "priorities" ]; then
    run_emacs --priorities /work/parity/corpus/priorities.json > "$TMP/priorities.json"
    exec "$BIN" priorities \
        --emacs "$TMP/priorities.json" \
        --table "$ROOT/parity/corpus/priorities.json" \
        --pin "$PIN"
fi

"$BIN" plan --repo "$ROOT" --out "$TMP/plan" > "$TMP/plan.tsv"

status=0
while IFS=$(printf '\t') read -r kind label org sidecar; do
    [ -n "$kind" ] || continue
    out="$TMP/emacs.json"
    err="$TMP/emacs.err"
    if ! run_emacs "$(rel "$org")" "$(rel "$sidecar")" > "$out" 2> "$err"; then
        printf '%s: emacs failed\n' "$label" >&2
        cat "$err" >&2
        status=1
        if [ "$kind" = "coding" ]; then
            break
        fi
        continue
    fi
    set -- \
        diff \
        --org "$org" \
        --sidecar "$sidecar" \
        --emacs "$out" \
        --label "$label" \
        --pin "$PIN"
    if [ "$kind" = "differ" ]; then
        set -- "$@" --expect differ
    else
        set -- "$@" --expect equal
    fi
    if [ "$kind" = "coding" ]; then
        set -- "$@" --coding
    fi
    if [ "$verbose" -eq 1 ]; then
        set -- "$@" --verbose
    fi
    if ! "$BIN" "$@"; then
        status=1
        if [ "$kind" = "coding" ]; then
            break
        fi
    fi
done < "$TMP/plan.tsv"

exit "$status"
