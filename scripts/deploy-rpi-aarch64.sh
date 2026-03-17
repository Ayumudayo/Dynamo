#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STAGE_DIR="$ROOT_DIR/output/rpi-aarch64"
ARCHIVE_PATH="$ROOT_DIR/output/rpi-aarch64.tar"
RPI_HOST="${RPI_HOST:-}"
RPI_USER="${RPI_USER:-}"
RPI_PORT="${RPI_PORT:-22}"
RPI_APP_DIR="${RPI_APP_DIR:-}"
RPI_SSH_KEY="${RPI_SSH_KEY:-}"
SKIP_BUILD=false
SKIP_BOOTSTRAP=false
FORCE_BOOTSTRAP=false

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
    --key)
      RPI_SSH_KEY="$2"
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
    --force-bootstrap)
      FORCE_BOOTSTRAP=true
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

SSH_ARGS=(-p "$RPI_PORT")
SCP_ARGS=(-P "$RPI_PORT")
if [[ -n "$RPI_SSH_KEY" ]]; then
  SSH_ARGS+=(-i "$RPI_SSH_KEY")
  SCP_ARGS+=(-i "$RPI_SSH_KEY")
fi

rm -f "$ARCHIVE_PATH"
tar -C "$STAGE_DIR" -cf "$ARCHIVE_PATH" .

scp "${SCP_ARGS[@]}" "$ARCHIVE_PATH" "$RPI_USER@$RPI_HOST:$RPI_APP_DIR.deploy.tar"
scp "${SCP_ARGS[@]}" "$ROOT_DIR/scripts/remote-rpi-postdeploy.sh" "$RPI_USER@$RPI_HOST:$RPI_APP_DIR.postdeploy.sh"

BOOTSTRAP_MODE="auto"
if [[ "$SKIP_BOOTSTRAP" == "true" ]]; then
  BOOTSTRAP_MODE="skip"
elif [[ "$FORCE_BOOTSTRAP" == "true" ]]; then
  BOOTSTRAP_MODE="force"
fi

ssh "${SSH_ARGS[@]}" "$RPI_USER@$RPI_HOST" \
  "chmod +x '$RPI_APP_DIR.postdeploy.sh' && bash '$RPI_APP_DIR.postdeploy.sh' '$RPI_APP_DIR' '$BOOTSTRAP_MODE' '$RPI_APP_DIR.deploy.tar'"

rm -f "$ARCHIVE_PATH"

echo "Deployed bundle to $RPI_USER@$RPI_HOST:$RPI_APP_DIR"
