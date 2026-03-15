#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOGS_DIR="$ROOT_DIR/logs"

stop_managed_process() {
  local name="$1"
  local pid_path="$LOGS_DIR/${name}.pid"

  if [[ ! -f "$pid_path" ]]; then
    echo "$name is not running (no pid file)."
    return
  fi

  local pid
  pid="$(cat "$pid_path" 2>/dev/null || true)"
  if [[ -z "$pid" ]]; then
    rm -f "$pid_path"
    echo "$name pid file was empty and has been removed."
    return
  fi

  if kill -0 "$pid" >/dev/null 2>&1; then
    kill "$pid" >/dev/null 2>&1 || true
    sleep 1
    kill -9 "$pid" >/dev/null 2>&1 || true
    echo "Stopped $name (pid=$pid)."
  else
    echo "$name process was already stopped."
  fi

  rm -f "$pid_path"
}

echo "Repo root: $ROOT_DIR"
echo "Logs dir:  $LOGS_DIR"

stop_managed_process "dashboard"
stop_managed_process "bot"
