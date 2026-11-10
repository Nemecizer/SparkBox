#!/bin/sh
# Opt-in Cargo runner for this Mac's slow Dropbox/syspolicyd test-binary startups.
# Copies only the executable; Cargo's test working directory and arguments stay intact.
set -eu
binary="$1"
shift
run_dir=$(mktemp -d /private/tmp/sparkbox-test-exe.XXXXXX)
copy="$run_dir/$(basename "$binary")"
trap 'rm -f "$copy"; rmdir "$run_dir"' 0
trap 'exit 130' INT
trap 'exit 143' HUP TERM
cp "$binary" "$copy"
"$copy" "$@"
