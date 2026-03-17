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

BOOTSTRAP_MODE="auto"
if [[ "$SKIP_BOOTSTRAP" == "true" ]]; then
  BOOTSTRAP_MODE="skip"
elif [[ "$FORCE_BOOTSTRAP" == "true" ]]; then
  BOOTSTRAP_MODE="force"
fi

ssh "${SSH_ARGS[@]}" "$RPI_USER@$RPI_HOST" "bash -s" -- "$RPI_APP_DIR" "$BOOTSTRAP_MODE" "$RPI_APP_DIR.deploy.tar" <<'REMOTE'
set -euo pipefail
APP_DIR="$1"
BOOTSTRAP_MODE="$2"
ARCHIVE_PATH="$3"

for profile in "$HOME/.profile" "$HOME/.bash_profile" "$HOME/.bashrc"; do
  if [[ -f "$profile" ]]; then
    # shellcheck disable=SC1090
    source "$profile"
  fi
done

if ! command -v pm2 >/dev/null 2>&1; then
  if [[ -d "$HOME/.nvm/versions/node" ]]; then
    latest_pm2_dir="$(find "$HOME/.nvm/versions/node" -path '*/bin/pm2' -printf '%h\n' 2>/dev/null | sort | tail -n 1 || true)"
    if [[ -n "$latest_pm2_dir" ]]; then
      export PATH="$latest_pm2_dir:$PATH"
    fi
  fi
fi

mkdir -p "$APP_DIR" "$APP_DIR/scripts" "$APP_DIR/target/release" "$APP_DIR/logs"
tar -C "$APP_DIR" -xf "$ARCHIVE_PATH"
rm -f "$ARCHIVE_PATH"

chmod +x "$APP_DIR"/scripts/*.sh "$APP_DIR"/target/release/dynamo-*

if [[ ! -f "$APP_DIR/.env" ]]; then
  cp "$APP_DIR/.env.example" "$APP_DIR/.env"
  echo "Created $APP_DIR/.env from .env.example. Fill it with real values and rerun deployment." >&2
  exit 1
fi

cd "$APP_DIR"

if [[ "$BOOTSTRAP_MODE" == "force" ]]; then
  ./scripts/prod-bootstrap.sh
  touch .bootstrap.done
elif [[ "$BOOTSTRAP_MODE" == "auto" && ! -f .bootstrap.done ]]; then
  ./scripts/prod-bootstrap.sh
  touch .bootstrap.done
fi

if ! command -v pm2 >/dev/null 2>&1; then
  echo "PATH=$PATH" >&2
  echo "pm2 is not installed on the target host." >&2
  exit 1
fi

echo "Using pm2 from: $(command -v pm2)"
echo "Using node from: $(command -v node || echo missing)"
if ! pm2 startOrRestart ecosystem.pm2.cjs --update-env; then
  echo "pm2 startOrRestart failed. Dumping pm2 status and recent logs..." >&2
  pm2 status || true
  pm2 logs dynamo-dashboard --lines 80 --nostream || true
  pm2 logs dynamo-bot --lines 80 --nostream || true
  exit 1
fi
pm2 save
REMOTE

rm -f "$ARCHIVE_PATH"

echo "Deployed bundle to $RPI_USER@$RPI_HOST:$RPI_APP_DIR"
