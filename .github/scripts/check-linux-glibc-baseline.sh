#!/usr/bin/env bash
set -euo pipefail

archive=$1
expected_version=$2
out_dir=$3
expected_glibc=${4:-glibc 2.31}

rm -rf "$out_dir"
mkdir -p "$out_dir/unpacked" "$out_dir/cache"

actual_glibc=$(getconf GNU_LIBC_VERSION)
{
  printf 'distribution='
  . /etc/os-release
  printf '%s %s\n' "$NAME" "$VERSION_ID"
  printf 'expected_glibc=%s\n' "$expected_glibc"
  printf 'actual_glibc=%s\n' "$actual_glibc"
} > "$out_dir/environment.txt"
test "$actual_glibc" = "$expected_glibc"

tar -xzf "$archive" -C "$out_dir/unpacked"
cli=$(find "$out_dir/unpacked" -type f -name codestory-cli -print -quit)
test -n "$cli"
chmod +x "$cli"

run_probe() {
  local name=$1
  shift
  set +e
  "$@" > "$out_dir/$name.stdout.txt" 2> "$out_dir/$name.stderr.txt"
  local status=$?
  set -e
  printf '%s\n' "$status" > "$out_dir/$name.exit-code.txt"
  test "$status" -eq 0
}

run_probe version "$cli" --version
grep -F "$expected_version" "$out_dir/version.stdout.txt"

run_probe help "$cli" --help
grep -Eiq 'usage:' "$out_dir/help.stdout.txt"

initialize='{"jsonrpc":"2.0","id":"initialize","method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"glibc-baseline-proof","version":"1.0.0"}}}'
set +e
printf '%s\n' "$initialize" | CODESTORY_CACHE_ROOT="$out_dir/cache" timeout 30s \
  "$cli" serve --stdio --refresh none --project /workspace \
  > "$out_dir/stdio-initialize.stdout.txt" 2> "$out_dir/stdio-initialize.stderr.txt"
stdio_status=${PIPESTATUS[1]}
set -e
printf '%s\n' "$stdio_status" > "$out_dir/stdio-initialize.exit-code.txt"
test "$stdio_status" -eq 0
grep -Eq '"jsonrpc"[[:space:]]*:[[:space:]]*"2\.0"' "$out_dir/stdio-initialize.stdout.txt"
grep -Eq '"protocolVersion"[[:space:]]*:[[:space:]]*"2024-11-05"' "$out_dir/stdio-initialize.stdout.txt"
grep -Eq '"serverInfo"[[:space:]]*:' "$out_dir/stdio-initialize.stdout.txt"

printf 'status=passed\n' >> "$out_dir/environment.txt"
