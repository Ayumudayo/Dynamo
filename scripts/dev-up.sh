#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ENV_PATH="$ROOT_DIR/.env"
LOGS_DIR="$ROOT_DIR/logs"

SKIP_BOOTSTRAP=false
SKIP_BUILD=false
ENABLE_GIVEAWAY=false
DRY_RUN=false

while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-bootstrap)
      SKIP_BOOTSTRAP=true
      shift
      ;;
    --skip-build)
      SKIP_BUILD=true
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

dashboard_public_base_url() {
  local base_url host port
  base_url="$(dotenv_value "DASHBOARD_BASE_URL" || true)"
  if [[ -n "$base_url" ]]; then
    printf '%s\n' "${base_url%/}"
    return
  fi

  host="$(dotenv_value "DASHBOARD_HOST" || true)"
  port="$(dotenv_value "DASHBOARD_PORT" || true)"
  [[ -z "$host" ]] && host="127.0.0.1"
  [[ -z "$port" ]] && port="3000"
  printf 'http://%s:%s\n' "$host" "$port"
}

dashboard_health_url() {
  local host port
  host="$(dotenv_value "DASHBOARD_HOST" || true)"
  port="$(dotenv_value "DASHBOARD_PORT" || true)"
  [[ -z "$host" ]] && host="127.0.0.1"
  [[ "$host" == "0.0.0.0" || "$host" == "::" ]] && host="127.0.0.1"
  [[ -z "$port" ]] && port="3000"
  printf 'http://%s:%s/healthz\n' "$host" "$port"
}

mkdir -p "$LOGS_DIR"

if [[ "$DRY_RUN" != "true" ]] && ! command -v cargo >/dev/null 2>&1; then
  echo "cargo was not found on PATH." >&2
  exit 1
fi

binary_path() {
  local crate="$1"
  printf '%s/target/debug/%s\n' "$ROOT_DIR" "$crate"
}

find_managed_pids() {
  local crate="$1"
  local binary
  binary="$(binary_path "$crate")"

  pgrep -af "$binary" | awk '{print $1}' || true
}

assert_binary_exists() {
  local crate="$1"
  local binary
  binary="$(binary_path "$crate")"
  if [[ ! -x "$binary" && ! -f "$binary" ]]; then
    echo "Missing built binary at $binary. Run without --skip-build first." >&2
    exit 1
  fi
}

stop_managed_process() {
  local name="$1"
  local crate="$2"
  local pid_path="$LOGS_DIR/${name}.pid"
  local stopped_any=false

  if [[ ! -f "$pid_path" ]]; then
    echo "No managed pid file for $name. Scanning for lingering processes..."
  else
    local pid
    pid="$(cat "$pid_path" 2>/dev/null || true)"
    if [[ -z "$pid" ]]; then
      rm -f "$pid_path"
    else
      if kill -0 "$pid" >/dev/null 2>&1; then
        echo "Stopping existing $name wrapper (pid=$pid)..."
        kill "$pid" >/dev/null 2>&1 || true
        sleep 1
        kill -9 "$pid" >/dev/null 2>&1 || true
        stopped_any=true
      fi
    fi
  fi

  rm -f "$pid_path"

  while IFS= read -r pid; do
    [[ -z "$pid" ]] && continue
    if kill -0 "$pid" >/dev/null 2>&1; then
      echo "Stopping lingering $name process (pid=$pid)..."
      kill "$pid" >/dev/null 2>&1 || true
      sleep 1
      kill -9 "$pid" >/dev/null 2>&1 || true
      stopped_any=true
    fi
  done < <(find_managed_pids "$crate")

  if [[ "$stopped_any" == "true" ]]; then
    sleep 1
  fi
}

start_process() {
  local name="$1"
  local crate="$2"
  local stdout_path="$LOGS_DIR/${name}.stdout.log"
  local stderr_path="$LOGS_DIR/${name}.stderr.log"
  local pid_path="$LOGS_DIR/${name}.pid"
  local binary

  binary="$(binary_path "$crate")"
  if [[ "$DRY_RUN" != "true" ]]; then
    assert_binary_exists "$crate"
  fi

  if [[ "$DRY_RUN" == "true" ]]; then
    echo "[dry-run] (cd '$ROOT_DIR' && '$binary') >'$stdout_path' 2>'$stderr_path' &"
    LAST_START_PID="-"
    LAST_START_STATUS="dry-run"
    LAST_START_LOG="$stdout_path"
    return
  fi

  stop_managed_process "$name" "$crate"

  (
    cd "$ROOT_DIR"
    if [[ "$ENABLE_GIVEAWAY" == "true" ]]; then
      export DYNAMO_ENABLE_GIVEAWAY=true
    fi
    nohup "$binary" >"$stdout_path" 2>"$stderr_path" &
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
    LAST_START_STATUS="degraded"
  else
    LAST_START_STATUS="ok"
  fi

  LAST_START_PID="$(cat "$pid_path")"
  LAST_START_LOG="$stdout_path"
}

run_binary_foreground() {
  local crate="$1"
  local binary
  binary="$(binary_path "$crate")"
  if [[ "$DRY_RUN" != "true" ]]; then
    assert_binary_exists "$crate"
  fi

  if [[ "$DRY_RUN" == "true" ]]; then
    echo "[dry-run] (cd '$ROOT_DIR' && '$binary')"
    LAST_RUN_STATUS="dry-run"
    return
  fi

  (
    cd "$ROOT_DIR"
    if [[ "$ENABLE_GIVEAWAY" == "true" ]]; then
      export DYNAMO_ENABLE_GIVEAWAY=true
    fi
    "$binary"
  )
  LAST_RUN_STATUS="ok"
}

test_dashboard_health() {
  local health_url="$1"

  if [[ "$DRY_RUN" == "true" ]]; then
    DASHBOARD_HEALTH_STATUS="dry-run"
    return
  fi

  if command -v curl >/dev/null 2>&1; then
    if curl -fsS --max-time 5 "$health_url" >/dev/null 2>&1; then
      DASHBOARD_HEALTH_STATUS="ok"
    else
      DASHBOARD_HEALTH_STATUS="degraded"
    fi
    return
  fi

  if command -v wget >/dev/null 2>&1; then
    if wget -q -T 5 -O /dev/null "$health_url" >/dev/null 2>&1; then
      DASHBOARD_HEALTH_STATUS="ok"
    else
      DASHBOARD_HEALTH_STATUS="degraded"
    fi
    return
  fi

  DASHBOARD_HEALTH_STATUS="degraded"
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
if [[ "$EFFECTIVE_GIVEAWAY" == "true" ]]; then
  echo "Giveaway module override: enabled"
fi

if [[ "$SKIP_BUILD" != "true" ]]; then
  if [[ "$DRY_RUN" == "true" ]]; then
    echo "[dry-run] (cd '$ROOT_DIR' && cargo build -p dynamo-bootstrap -p dynamo-dashboard -p dynamo-bot)"
    BUILD_STATUS="dry-run"
  else
    echo "Prebuilding shared Rust artifacts..."
    (
      cd "$ROOT_DIR"
      if [[ "$ENABLE_GIVEAWAY" == "true" ]]; then
        export DYNAMO_ENABLE_GIVEAWAY=true
      fi
      cargo build -p dynamo-bootstrap -p dynamo-dashboard -p dynamo-bot
    )
    BUILD_STATUS="ok"
  fi
else
  BUILD_STATUS="skipped"
fi

if [[ "$SKIP_BOOTSTRAP" != "true" ]]; then
  echo "Running Mongo bootstrap..."
  run_binary_foreground "dynamo-bootstrap"
  BOOTSTRAP_STATUS="$LAST_RUN_STATUS"
else
  BOOTSTRAP_STATUS="skipped"
fi

echo "Starting dashboard..."
start_process "dashboard" "dynamo-dashboard"
DASHBOARD_PID="$LAST_START_PID"
DASHBOARD_STATUS="$LAST_START_STATUS"
DASHBOARD_LOG="$LAST_START_LOG"

echo "Starting bot..."
start_process "bot" "dynamo-bot"
BOT_PID="$LAST_START_PID"
BOT_STATUS="$LAST_START_STATUS"
BOT_LOG="$LAST_START_LOG"

DASHBOARD_URL="$(dashboard_public_base_url)"
test_dashboard_health "$(dashboard_health_url)"

OVERALL_STATUS="ok"
if [[ "$DASHBOARD_STATUS" == "degraded" || "$BOT_STATUS" == "degraded" || "$DASHBOARD_HEALTH_STATUS" == "degraded" ]]; then
  OVERALL_STATUS="degraded"
fi

echo
echo "Startup summary:"
echo "  artifacts: $ROOT_DIR/target/debug [$BUILD_STATUS]"
echo "  bootstrap: $BOOTSTRAP_STATUS"
echo "  dashboard: pid=$DASHBOARD_PID url=$DASHBOARD_URL health=$DASHBOARD_HEALTH_STATUS log=$DASHBOARD_LOG"
echo "  bot: pid=$BOT_PID scope=$COMMAND_SCOPE log=$BOT_LOG"
echo "  overall: $OVERALL_STATUS"

echo "Done."
