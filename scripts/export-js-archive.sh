#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TEMPLATE_DIR="$ROOT_DIR/templates/js-archive"
OUTPUT_DIR="${1:-$ROOT_DIR/output/Dynamo-JS}"

INCLUDE_PATHS=(
  "bot.js"
  "config.js"
  "dashboard"
  "docs/commands"
  "jsconfig.json"
  "package.json"
  "package-lock.json"
  "scripts/db-v4-to-v5.js"
  "src"
  ".eslintrc.json"
  ".prettierrc.json"
  "LICENSE"
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
cp "$TEMPLATE_DIR/.gitignore" "$OUTPUT_DIR/.gitignore"

echo "Exported Dynamo-JS archive staging repo to $OUTPUT_DIR"
