#!/usr/bin/env bash
# Generates a large delimited file for virtualization and performance checks.
#
# The output lands in fixtures/generated/, which is not committed. Run this
# before benchmarking the grid or before a QA pass that needs a big file.
set -euo pipefail

rows="${1:-200000}"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out_dir="$root/fixtures/generated"
out="$out_dir/large.csv"

# Reserved test domain, so the fixture never contains a routable address.
domain="example.test"

mkdir -p "$out_dir"

{
  printf 'id,name,handle,team,commits,active\n'
  for ((i = 1; i <= rows; i++)); do
    if ((i % 3 == 0)); then active="true"; else active="false"; fi
    printf '%d,user_%d,user_%d@%s,team_%d,%d,%s\n' \
      "$i" "$i" "$i" "$domain" "$((i % 12))" "$((i * 7 % 5000))" "$active"
  done
} >"$out"

printf 'wrote %s rows to %s (%s)\n' "$rows" "$out" "$(du -h "$out" | cut -f1)"
