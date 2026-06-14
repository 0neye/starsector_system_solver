#!/bin/sh
# Thin shim: the installer now lives in the binary itself.
# Runs `system_solver install` from this unpacked release archive.
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
BINARY="$SCRIPT_DIR/system_solver"

if [ ! -x "$BINARY" ]; then
    echo "system_solver not found next to this script; run it from inside the unpacked release archive." >&2
    exit 1
fi

exec "$BINARY" install "$@"
