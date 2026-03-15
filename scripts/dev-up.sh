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

dotenv_value() {
  local key="$1"
  local line
  line="$(grep -E "^[[:space:]]*${key}[[:space:]]*=" "$ENV_PATH" | tail -n 1 || true)"
  if [[ -z "$line" ]]; then
    return 1
  fi
  line="${line#*=}"
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
REGISTER_GLOBALLY="$(resolve_bool_setting "DISCORD_REGISTER_GLOBALLY" "true" "false")"
DEV_GUILD_ID="$(dotenv_value "DISCORD_DEV_GUILD_ID" || true)"
if [[ "$REGISTER_GLOBALLY" != "true" ]]; then
  if [[ -n "$DEV_GUILD_ID" ]]; then
    COMMAND_SCOPE="guild ($DEV_GUILD_ID)"
  else
    COMMAND_SCOPE="guild (missing DISCORD_DEV_GUILD_ID)"
  fi
fi
EFFECTIVE_GIVEAWAY="$(resolve_bool_setting "DYNAMO_ENABLE_GIVEAWAY" "false" "$ENABLE_GIVEAWAY")"
EFFECTIVE_MUSIC="$(resolve_bool_setting "DYNAMO_ENABLE_MUSIC" "false" "$ENABLE_MUSIC")"
echo "Command scope: $COMMAND_SCOPE"
echo "Optional modules: giveaway=$EFFECTIVE_GIVEAWAY music=$EFFECTIVE_MUSIC"
if [[ "$EFFECTIVE_MUSIC" != "true" ]]; then
  echo "WARNING: Music module is disabled. Set DYNAMO_ENABLE_MUSIC=true in .env or pass --enable-music to register /music commands and show the module in the dashboard." >&2
fi

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
