#!/usr/bin/env bash

set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE="${1:-/tmp/sak-bench-$(date +%s)}"
SIZE_MIB="${SIZE_MIB:-256}"
FILES="${FILES:-64}"
PASSWORD="${PASSWORD:-test}"
SNAPSHOT_PATH="${SNAPSHOT_PATH:-fixture}"
HOST="${HOST:-bench-host}"
LABEL="${LABEL:-bench}"
TAG="${TAG:-bench}"

SAK_BIN="$ROOT/target/release/sak"
RUSTIC_BIN="$ROOT/refs/rustic/target/release/rustic"

if [[ ! -x "$SAK_BIN" ]]; then
  printf 'missing %s\n' "$SAK_BIN" >&2
  printf 'build it with: cargo build --release\n' >&2
  exit 1
fi

if [[ ! -x "$RUSTIC_BIN" ]]; then
  printf 'missing %s\n' "$RUSTIC_BIN" >&2
  printf 'build it with: cargo build --release --manifest-path refs/rustic/Cargo.toml\n' >&2
  exit 1
fi

SOURCE="$WORKSPACE/source"
SAK_REPO="$WORKSPACE/sak-repo"
RUSTIC_REPO="$WORKSPACE/rustic-repo"

mkdir --parents "$SOURCE"

base_bytes=$((SIZE_MIB * 1024 * 1024 / FILES))
remainder=$((SIZE_MIB * 1024 * 1024 % FILES))

for i in $(seq 0 $((FILES - 1))); do
  section=$(printf '%02d' $((i % 8)))
  shelf=$(printf '%02d' $(((i / 8) % 8)))
  dir="$SOURCE/section-$section/shelf-$shelf"
  file="$dir/item-$(printf '%04d' "$i").bin"
  size="$base_bytes"
  if (( i < remainder )); then
    size=$((size + 1))
  fi

  mkdir --parents "$dir"
  head --bytes "$size" /dev/urandom > "$file"
done

measure() {
  local start end
  start=$(date +%s%N)
  "$@" >&2
  end=$(date +%s%N)
  printf '%s\n' $((end - start))
}

format_ns() {
  local ns="$1"
  printf '%s.%03ds' "$((ns / 1000000000))" "$(((ns / 1000000) % 1000))"
}

sak_first_ns=$(measure "$SAK_BIN" import \
  --repo "$SAK_REPO" \
  --source "$SOURCE" \
  --snapshot-path "$SNAPSHOT_PATH" \
  --host "$HOST" \
  --label "$LABEL" \
  --tag "$TAG" \
  --password "$PASSWORD")

sak_second_ns=$(measure "$SAK_BIN" import \
  --repo "$SAK_REPO" \
  --source "$SOURCE" \
  --snapshot-path "$SNAPSHOT_PATH" \
  --host "$HOST" \
  --label "$LABEL" \
  --tag "$TAG" \
  --password "$PASSWORD")

rustic_first_ns=$(measure "$RUSTIC_BIN" \
  --repository "$RUSTIC_REPO" \
  --password "$PASSWORD" \
  --no-cache \
  backup \
  --init \
  --as-path "$SNAPSHOT_PATH" \
  --host "$HOST" \
  --label "$LABEL" \
  --tag "$TAG" \
  "$SOURCE")

rustic_second_ns=$(measure "$RUSTIC_BIN" \
  --repository "$RUSTIC_REPO" \
  --password "$PASSWORD" \
  --no-cache \
  backup \
  --as-path "$SNAPSHOT_PATH" \
  --host "$HOST" \
  --label "$LABEL" \
  --tag "$TAG" \
  "$SOURCE")

printf 'workspace: %s\n' "$WORKSPACE"
printf 'source: %s\n' "$SOURCE"
printf 'sak repo: %s\n' "$SAK_REPO"
printf 'rustic repo: %s\n' "$RUSTIC_REPO"
printf 'sak first: %s\n' "$(format_ns "$sak_first_ns")"
printf 'sak second: %s\n' "$(format_ns "$sak_second_ns")"
printf 'rustic first: %s\n' "$(format_ns "$rustic_first_ns")"
printf 'rustic second: %s\n' "$(format_ns "$rustic_second_ns")"
