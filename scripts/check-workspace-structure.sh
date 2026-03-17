#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

fail() {
  echo "workspace-structure: $*" >&2
  exit 1
}

CARGO_BIN="${CARGO_BIN:-cargo}"
if ! command -v "$CARGO_BIN" >/dev/null 2>&1 || [[ ! -x "$(command -v "$CARGO_BIN" 2>/dev/null)" ]]; then
  if command -v cargo.exe >/dev/null 2>&1; then
    CARGO_BIN="cargo.exe"
  else
    fail "cargo was not found on PATH"
  fi
fi

RG_BIN="${RG_BIN:-rg}"
if ! command -v "$RG_BIN" >/dev/null 2>&1 || [[ ! -x "$(command -v "$RG_BIN" 2>/dev/null)" ]]; then
  if command -v rg.exe >/dev/null 2>&1; then
    RG_BIN="rg.exe"
  else
    fail "rg was not found on PATH"
  fi
fi

tmp_file="$(mktemp)"
trap 'rm -f "$tmp_file"' EXIT

"$CARGO_BIN" metadata --no-deps --format-version 1 >"$tmp_file"

retired_members=(
  "\"crates/contracts\""
  "\"crates/runtime\""
  "\"crates/core\""
  "\"crates/modules/music\""
  "\"crates/providers/music-songbird\""
)

for member in "${retired_members[@]}"; do
  if grep -Fq "$member" Cargo.toml; then
    fail "retired workspace member still present in Cargo.toml: $member"
  fi
done

retired_packages=(
  "\"name\":\"dynamo-contracts\""
  "\"name\":\"dynamo-runtime\""
  "\"name\":\"dynamo-core\""
  "\"name\":\"dynamo-module-music\""
  "\"name\":\"dynamo-provider-music-songbird\""
)

for package in "${retired_packages[@]}"; do
  if grep -Fq "$package" "$tmp_file"; then
    fail "retired package still present in cargo metadata: $package"
  fi
done

active_targets=(
  "Cargo.toml"
  "README.md"
  ".github/workflows"
  "crates"
  "scripts"
  "docs/dev-smoke-checklist.md"
  "docs/workspace-architecture.md"
)

patterns=(
  "dynamo-core"
  "dynamo-contracts"
  "(?<![A-Za-z0-9_-])dynamo-runtime(?![A-Za-z0-9_-])"
  "dynamo-module-music"
  "crates/modules/music"
  "dynamo-provider-music-songbird"
  "crates/providers/music-songbird"
  "songbird"
  "DAVE"
  "lavalink"
)

for pattern in "${patterns[@]}"; do
  if "$RG_BIN" -n -P "$pattern" "${active_targets[@]}" \
    -g '!scripts/check-workspace-structure.sh' \
    -g '!scripts/check-workspace-structure.ps1' \
    -g '!docs/cutover/**' \
    -g '!docs/rust-template/**' \
    >/dev/null; then
    fail "retired pattern reintroduced into active surface: $pattern"
  fi
done

echo "workspace-structure: ok"
