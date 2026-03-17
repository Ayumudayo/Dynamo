#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STAGE_DIR="$ROOT_DIR/output/rpi-aarch64"
RPI_HOST="${RPI_HOST:-}"
RPI_USER="${RPI_USER:-}"
RPI_PORT="${RPI_PORT:-22}"
RPI_APP_DIR="${RPI_APP_DIR:-}"
SKIP_BUILD=false
SKIP_BOOTSTRAP=false

while [[ $# -gt 0 ]]; do
  case "$1" in
    --host)
      RPI_HOST="$2"
      shift 2
      ;;
    --user)
      RPI_USER="$2"
      shift 2
      ;;
    --port)
      RPI_PORT="$2"
      shift 2
      ;;
    --app-dir)
      RPI_APP_DIR="$2"
      shift 2
      ;;
    --skip-build)
      SKIP_BUILD=true
      shift
      ;;
    --skip-bootstrap)
      SKIP_BOOTSTRAP=true
      shift
      ;;
    *)
      echo "Unknown argument: $1" >&2
      exit 1
      ;;
  esac
done

if [[ -z "$RPI_HOST" || -z "$RPI_USER" ]]; then
  echo "RPI_HOST and RPI_USER (or --host/--user) are required." >&2
  exit 1
fi

if [[ -z "$RPI_APP_DIR" ]]; then
  RPI_APP_DIR="/home/$RPI_USER/dynamo"
fi

if [[ "$SKIP_BUILD" != "true" ]]; then
  "$ROOT_DIR/scripts/build-rpi-aarch64.sh"
fi

if [[ ! -d "$STAGE_DIR" ]]; then
  echo "Missing staged bundle at $STAGE_DIR. Run build-rpi-aarch64 first." >&2
  exit 1
fi

ssh -p "$RPI_PORT" "$RPI_USER@$RPI_HOST" "mkdir -p '$RPI_APP_DIR' '$RPI_APP_DIR/scripts' '$RPI_APP_DIR/target/release' '$RPI_APP_DIR/logs'"
scp -P "$RPI_PORT" "$STAGE_DIR/ecosystem.pm2.cjs" "$STAGE_DIR/.env.example" "$RPI_USER@$RPI_HOST:$RPI_APP_DIR/"
scp -P "$RPI_PORT" "$STAGE_DIR/scripts/"* "$RPI_USER@$RPI_HOST:$RPI_APP_DIR/scripts/"
scp -P "$RPI_PORT" "$STAGE_DIR/target/release/"* "$RPI_USER@$RPI_HOST:$RPI_APP_DIR/target/release/"

BOOTSTRAP_FLAG="true"
if [[ "$SKIP_BOOTSTRAP" == "true" ]]; then
  BOOTSTRAP_FLAG="false"
fi

ssh -p "$RPI_PORT" "$RPI_USER@$RPI_HOST" "bash -s" -- "$RPI_APP_DIR" "$BOOTSTRAP_FLAG" <<'REMOTE'
set -euo pipefail
APP_DIR="$1"
RUN_BOOTSTRAP="$2"

chmod +x "$APP_DIR"/scripts/*.sh

if [[ ! -f "$APP_DIR/.env" ]]; then
  cp "$APP_DIR/.env.example" "$APP_DIR/.env"
  echo "Created $APP_DIR/.env from .env.example. Fill it with real values and rerun deployment." >&2
  exit 1
fi

cd "$APP_DIR"

if [[ "$RUN_BOOTSTRAP" == "true" ]]; then
  ./scripts/prod-bootstrap.sh
fi

if ! command -v pm2 >/dev/null 2>&1; then
  echo "pm2 is not installed on the target host." >&2
  exit 1
fi

pm2 startOrRestart ecosystem.pm2.cjs --update-env
pm2 save
REMOTE

echo "Deployed bundle to $RPI_USER@$RPI_HOST:$RPI_APP_DIR"

