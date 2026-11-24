#!/usr/bin/env bash
# Generates large fixture files for virtualization and performance checks.
#
# Two files come out of one run: large.csv for the grid, and large.json for the
# JSON viewer. The JSON file is compact, one record per line, so it is a large
# file on disk that expands to several lines per record once it is formatted.
# The output lands in fixtures/generated/, which is not committed. Run this
# before benchmarking a view or before a QA pass that needs a big file.
set -euo pipefail

rows="${1:-200000}"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out_dir="$root/fixtures/generated"
out="$out_dir/large.csv"
json="$out_dir/large.json"

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

{
  printf '[\n'
  for ((i = 1; i <= rows; i++)); do
    if ((i % 3 == 0)); then active="true"; else active="false"; fi
    if ((i < rows)); then separator=","; else separator=""; fi
    printf '  {"id":%d,"name":"user_%d","handle":"user_%d@%s","team":%d,"commits":%d,"active":%s,"tags":["alpha","beta"]}%s\n' \
      "$i" "$i" "$i" "$domain" "$((i % 12))" "$((i * 7 % 5000))" "$active" "$separator"
  done
  printf ']\n'
} >"$json"

printf 'wrote %s rows to %s (%s) and %s (%s)\n' \
  "$rows" "$out" "$(du -h "$out" | cut -f1)" "$json" "$(du -h "$json" | cut -f1)"
