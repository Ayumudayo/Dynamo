#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if [[ ! -f "$ROOT_DIR/.env" ]]; then
  echo "Missing .env at $ROOT_DIR/.env. Copy .env.example first." >&2
  exit 1
fi

cd "$ROOT_DIR"
cargo build --release -p dynamo-bootstrap -p dynamo-dashboard -p dynamo-bot

