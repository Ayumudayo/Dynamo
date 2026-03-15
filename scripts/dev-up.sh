#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ENV_PATH="$ROOT_DIR/.env"
LOGS_DIR="$ROOT_DIR/logs"

SKIP_BOOTSTRAP=false
ENABLE_GIVEAWAY=false
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
      echo "--enable-music is no longer needed. Music is a built-in module; use dashboard deployment/guild toggles." >&2
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

dotenv_value() {
  local key="$1"
  local line
  line="$(grep -E "^[[:space:]]*${key}[[:space:]]*=" "$ENV_PATH" | tail -n 1 || true)"
  if [[ -z "$line" ]]; then
    return 1
  fi
  line="${line#*=}"
  line="${line%$'\r'}"
  line="${line%\"}"
  line="${line#\"}"
  line="${line%\'}"
  line="${line#\'}"
  printf '%s\n' "$line"
}

parse_bool_setting() {
  local key="$1"
  local value="${2,,}"
  case "$value" in
    1|true|yes|on) printf 'true\n' ;;
    0|false|no|off) printf 'false\n' ;;
    *)
      echo "$key in .env must be one of true/false/1/0/yes/no/on/off." >&2
      exit 1
      ;;
  esac
}

resolve_bool_setting() {
  local key="$1"
  local default_value="$2"
  local cli_enabled="$3"

  if [[ "$cli_enabled" == "true" ]]; then
    printf 'true\n'
    return
  fi

  local raw_value
  raw_value="$(dotenv_value "$key" || true)"
  if [[ -z "$raw_value" ]]; then
    printf '%s\n' "$default_value"
    return
  fi

  parse_bool_setting "$key" "$raw_value"
}

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
    cargo run -p "$crate" "$@"
  )
}

stop_managed_process() {
  local name="$1"
  local pid_path="$LOGS_DIR/${name}.pid"

  if [[ ! -f "$pid_path" ]]; then
    return
  fi

  local pid
  pid="$(cat "$pid_path" 2>/dev/null || true)"
  if [[ -z "$pid" ]]; then
    rm -f "$pid_path"
    return
  fi

  if kill -0 "$pid" >/dev/null 2>&1; then
    echo "Stopping existing $name process (pid=$pid)..."
    kill "$pid" >/dev/null 2>&1 || true
    sleep 1
    kill -9 "$pid" >/dev/null 2>&1 || true
  fi

  rm -f "$pid_path"
}

start_process() {
  local name="$1"
  local crate="$2"
  local stdout_path="$LOGS_DIR/${name}.stdout.log"
  local stderr_path="$LOGS_DIR/${name}.stderr.log"
  local pid_path="$LOGS_DIR/${name}.pid"

  stop_managed_process "$name"

  if [[ "$DRY_RUN" == "true" ]]; then
    echo "[dry-run] (cd '$ROOT_DIR' && cargo run -p '$crate') >'$stdout_path' 2>'$stderr_path' &"
    return
  fi

  (
    cd "$ROOT_DIR"
    if [[ "$ENABLE_GIVEAWAY" == "true" ]]; then
      export DYNAMO_ENABLE_GIVEAWAY=true
    fi
    nohup cargo run -p "$crate" >"$stdout_path" 2>"$stderr_path" &
    echo $! >"$pid_path"
    echo "$name started (pid=$(cat "$pid_path"))"
    echo "  stdout: $stdout_path"
    echo "  stderr: $stderr_path"
    echo "  pid:    $pid_path"
  )

  sleep 2
  if ! kill -0 "$(cat "$pid_path")" >/dev/null 2>&1; then
    echo "WARNING: $name exited immediately." >&2
    if [[ -f "$stdout_path" ]]; then
      echo "---- $name stdout ----"
      tail -n 40 "$stdout_path"
    fi
    if [[ -f "$stderr_path" ]]; then
      echo "---- $name stderr ----"
      tail -n 40 "$stderr_path"
      if grep -q "Disallowed gateway intents" "$stderr_path"; then
        echo "WARNING: Discord bot intents are not enabled in the developer portal. Enable the required privileged intents, especially Server Members Intent." >&2
      fi
    fi
  fi
}

echo "Repo root: $ROOT_DIR"
echo "Logs dir:  $LOGS_DIR"
COMMAND_SCOPE="global"
DEV_GUILD_ID="$(dotenv_value "DISCORD_DEV_GUILD_ID" || true)"
if [[ -z "$DEV_GUILD_ID" ]]; then
  DEV_GUILD_ID="$(dotenv_value "GUILD_ID" || true)"
fi
REGISTER_GLOBALLY_DEFAULT="true"
if [[ -n "$DEV_GUILD_ID" ]]; then
  REGISTER_GLOBALLY_DEFAULT="false"
fi
REGISTER_GLOBALLY="$(resolve_bool_setting "DISCORD_REGISTER_GLOBALLY" "$REGISTER_GLOBALLY_DEFAULT" "false")"
if [[ "$REGISTER_GLOBALLY" != "true" ]]; then
  if [[ -n "$DEV_GUILD_ID" ]]; then
    COMMAND_SCOPE="guild ($DEV_GUILD_ID)"
  else
    COMMAND_SCOPE="guild (missing DISCORD_DEV_GUILD_ID/GUILD_ID)"
  fi
fi
EFFECTIVE_GIVEAWAY="$(resolve_bool_setting "DYNAMO_ENABLE_GIVEAWAY" "false" "$ENABLE_GIVEAWAY")"
echo "Command scope: $COMMAND_SCOPE"
echo "Optional modules: giveaway=$EFFECTIVE_GIVEAWAY"
echo "Built-in modules: music=available"

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
