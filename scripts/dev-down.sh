#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOGS_DIR="$ROOT_DIR/logs"

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

stop_managed_process() {
  local name="$1"
  local crate="$2"
  local pid_path="$LOGS_DIR/${name}.pid"
  local stopped_any=false

  if [[ ! -f "$pid_path" ]]; then
    echo "$name pid file was not found. Scanning for lingering processes..."
  else
    local pid
    pid="$(cat "$pid_path" 2>/dev/null || true)"
    if [[ -z "$pid" ]]; then
      rm -f "$pid_path"
      echo "$name pid file was empty and has been removed."
    else
      if kill -0 "$pid" >/dev/null 2>&1; then
        kill "$pid" >/dev/null 2>&1 || true
        sleep 1
        kill -9 "$pid" >/dev/null 2>&1 || true
        echo "Stopped $name wrapper (pid=$pid)."
        stopped_any=true
      fi
    fi
  fi

  rm -f "$pid_path"

  while IFS= read -r pid; do
    [[ -z "$pid" ]] && continue
    if kill -0 "$pid" >/dev/null 2>&1; then
      kill "$pid" >/dev/null 2>&1 || true
      sleep 1
      kill -9 "$pid" >/dev/null 2>&1 || true
      echo "Stopped $name lingering process (pid=$pid)."
      stopped_any=true
    fi
  done < <(find_managed_pids "$crate")

  if [[ "$stopped_any" != "true" ]]; then
    echo "$name was not running."
  fi
}

echo "Repo root: $ROOT_DIR"
echo "Logs dir:  $LOGS_DIR"

stop_managed_process "dashboard" "dynamo-dashboard"
stop_managed_process "bot" "dynamo-bot"
