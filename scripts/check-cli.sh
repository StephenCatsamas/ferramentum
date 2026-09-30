#!/usr/bin/env bash
# Check a CLI without resolving other workspace members' sibling path dependencies.
set -euo pipefail

case "${1:-}" in
    kai-tool|ice-tool) check_package=$1 ;;
    *)
        echo "Usage: $0 {kai-tool|ice-tool} [cargo-command] [arguments...]" >&2
        exit 2
        ;;
esac
shift
check_action=${1:-test}
if [ "$#" -gt 0 ]; then shift; fi
check_repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
mkdir -p "$check_repo/target"
check_workspace=$(mktemp -d "$check_repo/target/check-$check_package.XXXXXX")
trap 'rm -rf "$check_workspace"' EXIT

cp -R "$check_repo/$check_package" "$check_workspace/$check_package"
# Retain workspace dependencies, patches, and profiles; narrow only the member list.
awk -v package="$check_package" '
    /^members = \[/ { print "members = [\"" package "\"]"; skip=1; next }
    skip && /^\]/ { skip=0; next }
    !skip { print }
' "$check_repo/Cargo.toml" > "$check_workspace/Cargo.toml"
if [ -f "$check_repo/Cargo.lock" ]; then
    cp "$check_repo/Cargo.lock" "$check_workspace/Cargo.lock"
fi
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$check_repo/target/${check_package%-tool}-standalone}"
cargo "$check_action" --manifest-path "$check_workspace/Cargo.toml" -p "$check_package" "$@"
