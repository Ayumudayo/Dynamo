#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BINARY="$ROOT_DIR/target/release/dynamo-bot"

# shellcheck source=lib/secure-env.sh
source "$ROOT_DIR/scripts/lib/secure-env.sh"
assert_secure_env "$ROOT_DIR/.env"

if [[ ! -x "$BINARY" && ! -f "$BINARY" ]]; then
  echo "Missing release binary at $BINARY. Run ./scripts/prod-build.sh first." >&2
  exit 1
fi

cd "$ROOT_DIR"
exec "$BINARY"
