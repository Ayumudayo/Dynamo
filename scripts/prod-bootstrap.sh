#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BINARY="$ROOT_DIR/target/release/dynamo-bootstrap"

if [[ ! -f "$ROOT_DIR/.env" ]]; then
  echo "Missing .env at $ROOT_DIR/.env. Copy .env.example first." >&2
  exit 1
fi

if [[ ! -x "$BINARY" && ! -f "$BINARY" ]]; then
  echo "Missing release binary at $BINARY. Run ./scripts/prod-build.sh first." >&2
  exit 1
fi

cd "$ROOT_DIR"
exec "$BINARY"

