#!/usr/bin/env sh
set -eu

find_root() {
    dir=${1:-"$PWD"}

    while [ "$dir" != "/" ]; do
        if [ -f "$dir/Cargo.toml" ]; then
            printf '%s\n' "$dir"
            return 0
        fi

        dir=$(dirname "$dir")
    done

    return 1
}

root=$(find_root "$PWD" || true)
if [ -z "${root:-}" ]; then
    echo "resteyes rustfmt hook skipped: could not find Cargo.toml" >&2
    exit 0
fi

cd "$root"

if command -v make >/dev/null 2>&1; then
    make fmt
elif command -v cargo >/dev/null 2>&1; then
    cargo fmt --all
elif command -v nix >/dev/null 2>&1; then
    nix develop --command cargo fmt --all
else
    echo "resteyes rustfmt hook skipped: make, cargo, and nix are unavailable" >&2
fi
