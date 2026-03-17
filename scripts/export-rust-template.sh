#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TEMPLATE_DIR="$ROOT_DIR/templates/rust-template"
OUTPUT_DIR="${1:-$ROOT_DIR/output/rust-template}"

INCLUDE_PATHS=(
  ".cargo"
  ".github"
  "Cargo.toml"
  "Cargo.lock"
  "LICENSE"
  "crates"
  "docs/dev-smoke-checklist.md"
  "playwright.dashboard.config.cjs"
  "scripts/dev-up.ps1"
  "scripts/dev-down.ps1"
  "scripts/dev-up.sh"
  "scripts/dev-down.sh"
  "tests/playwright"
)

echo "Repo root:    $ROOT_DIR"
echo "Template dir: $TEMPLATE_DIR"
echo "Output dir:   $OUTPUT_DIR"

rm -rf "$OUTPUT_DIR"
mkdir -p "$OUTPUT_DIR"

for path in "${INCLUDE_PATHS[@]}"; do
  mkdir -p "$(dirname "$OUTPUT_DIR/$path")"
  if [[ -d "$ROOT_DIR/$path" ]]; then
    mkdir -p "$OUTPUT_DIR/$path"
    cp -R "$ROOT_DIR/$path/." "$OUTPUT_DIR/$path/"
  else
    cp "$ROOT_DIR/$path" "$OUTPUT_DIR/$path"
  fi
done

cp "$TEMPLATE_DIR/README.md" "$OUTPUT_DIR/README.md"
cp "$TEMPLATE_DIR/.env.example" "$OUTPUT_DIR/.env.example"
cp "$TEMPLATE_DIR/package.json" "$OUTPUT_DIR/package.json"
cp "$TEMPLATE_DIR/.gitignore" "$OUTPUT_DIR/.gitignore"

echo "Exported fresh Rust-only template staging repo to $OUTPUT_DIR"
