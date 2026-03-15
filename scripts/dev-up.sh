#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ENV_PATH="$ROOT_DIR/.env"
LOGS_DIR="$ROOT_DIR/logs"

SKIP_BOOTSTRAP=false
ENABLE_GIVEAWAY=false
ENABLE_MUSIC=false
DRY_RUN=false

while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-bootstrap)
      SKIP_BOOTSTRAP=true
      shift
      ;;
    --enable-giveaway)
      ENABLE_GIVEAWAY=true
      shift
      ;;
    --enable-music)
      ENABLE_MUSIC=true
      shift
      ;;
    --dry-run)
      DRY_RUN=true
      shift
      ;;
    *)
      echo "Unknown argument: $1" >&2
      exit 1
      ;;
  esac
done

if [[ ! -f "$ENV_PATH" ]]; then
  echo "Missing .env at $ENV_PATH. Copy .env.example first." >&2
  exit 1
fi

mkdir -p "$LOGS_DIR"

if [[ "$DRY_RUN" != "true" ]] && ! command -v cargo >/dev/null 2>&1; then
  echo "cargo was not found on PATH." >&2
  exit 1
fi

run_with_overrides() {
  local crate="$1"
  shift

  (
    cd "$ROOT_DIR"
    if [[ "$ENABLE_GIVEAWAY" == "true" ]]; then
      export DYNAMO_ENABLE_GIVEAWAY=true
    fi
    if [[ "$ENABLE_MUSIC" == "true" ]]; then
      export DYNAMO_ENABLE_MUSIC=true
    fi
    cargo run -p "$crate" "$@"
  )
}

start_process() {
  local name="$1"
  local crate="$2"
  local stdout_path="$LOGS_DIR/${name}.stdout.log"
  local stderr_path="$LOGS_DIR/${name}.stderr.log"
  local pid_path="$LOGS_DIR/${name}.pid"

  if [[ "$DRY_RUN" == "true" ]]; then
    echo "[dry-run] (cd '$ROOT_DIR' && cargo run -p '$crate') >'$stdout_path' 2>'$stderr_path' &"
    return
  fi

  (
    cd "$ROOT_DIR"
    if [[ "$ENABLE_GIVEAWAY" == "true" ]]; then
      export DYNAMO_ENABLE_GIVEAWAY=true
    fi
    if [[ "$ENABLE_MUSIC" == "true" ]]; then
      export DYNAMO_ENABLE_MUSIC=true
    fi
    nohup cargo run -p "$crate" >"$stdout_path" 2>"$stderr_path" &
    echo $! >"$pid_path"
    echo "$name started (pid=$(cat "$pid_path"))"
    echo "  stdout: $stdout_path"
    echo "  stderr: $stderr_path"
    echo "  pid:    $pid_path"
  )
}

echo "Repo root: $ROOT_DIR"
echo "Logs dir:  $LOGS_DIR"

if [[ "$SKIP_BOOTSTRAP" != "true" ]]; then
  if [[ "$DRY_RUN" == "true" ]]; then
    echo "[dry-run] (cd '$ROOT_DIR' && cargo run -p dynamo-bootstrap)"
  else
    echo "Running Mongo bootstrap..."
    run_with_overrides "dynamo-bootstrap"
  fi
fi

echo "Starting dashboard..."
start_process "dashboard" "dynamo-dashboard"

echo "Starting bot..."
start_process "bot" "dynamo-bot"

echo "Done."
