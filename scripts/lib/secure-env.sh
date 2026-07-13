#!/usr/bin/env bash

# This file is sourced by production entrypoints. Do not enable shell options here;
# callers own their shell state.

secure_env_error() {
  printf 'secure-env: %s\n' "$*" >&2
}

assert_secure_env() (
  local target="${1:-}"
  local target_dir
  local target_name
  local target_path
  local expected_uid
  local actual_uid
  local actual_mode

  if [[ -z "$target" ]]; then
    secure_env_error "assert_secure_env requires a target path"
    return 2
  fi

  target_dir="$(dirname -- "$target")"
  target_name="$(basename -- "$target")"
  if ! target_dir="$(cd -- "$target_dir" 2>/dev/null && pwd -P)"; then
    secure_env_error "target directory does not exist: $(dirname -- "$target")"
    return 1
  fi
  target_path="$target_dir/$target_name"

  if [[ -L "$target_path" ]]; then
    secure_env_error "refusing symbolic-link environment file: $target_path"
    return 1
  fi
  if [[ ! -e "$target_path" ]]; then
    secure_env_error "missing environment file: $target_path"
    return 1
  fi
  if [[ ! -f "$target_path" ]]; then
    secure_env_error "environment path is not a regular file: $target_path"
    return 1
  fi

  if ! expected_uid="$(id -u)"; then
    secure_env_error "could not determine the effective user id"
    return 1
  fi
  if ! actual_uid="$(stat -c '%u' -- "$target_path")"; then
    secure_env_error "could not read environment file ownership: $target_path"
    return 1
  fi
  if [[ "$actual_uid" != "$expected_uid" ]]; then
    secure_env_error "environment file owner mismatch for $target_path (expected uid $expected_uid, found uid $actual_uid)"
    return 1
  fi

  if ! actual_mode="$(stat -c '%a' -- "$target_path")"; then
    secure_env_error "could not read environment file mode: $target_path"
    return 1
  fi
  if [[ "$actual_mode" != "600" ]]; then
    secure_env_error "environment file mode must be 600: $target_path (found $actual_mode)"
    return 1
  fi

  # Read the whole file without emitting it. This catches ACL/filesystem readback
  # failures that a successful stat alone cannot detect.
  if ! cat -- "$target_path" >/dev/null; then
    secure_env_error "environment file is not readable by its owner: $target_path"
    return 1
  fi
)

create_secure_env() (
  local template="${1:-}"
  local target="${2:-}"
  local target_dir
  local target_name
  local target_path
  local owned_temp=""

  if [[ -z "$template" || -z "$target" ]]; then
    secure_env_error "create_secure_env requires TEMPLATE and TARGET paths"
    return 2
  fi

  target_dir="$(dirname -- "$target")"
  target_name="$(basename -- "$target")"
  if ! target_dir="$(cd -- "$target_dir" 2>/dev/null && pwd -P)"; then
    secure_env_error "target directory does not exist: $(dirname -- "$target")"
    return 1
  fi
  target_path="$target_dir/$target_name"

  # An existing target wins before TEMPLATE is inspected. A valid deployed file
  # therefore remains byte-for-byte unchanged even if the template is absent.
  if [[ -e "$target_path" || -L "$target_path" ]]; then
    assert_secure_env "$target_path"
    return
  fi

  if [[ -L "$template" || ! -f "$template" || ! -r "$template" ]]; then
    secure_env_error "template must be a readable regular non-symlink file: $template"
    return 1
  fi

  cleanup_secure_env_temp() {
    if [[ -n "$owned_temp" ]]; then
      rm -f -- "$owned_temp" || true
    fi
  }
  trap cleanup_secure_env_temp EXIT
  trap 'exit 129' HUP
  trap 'exit 130' INT
  trap 'exit 143' TERM

  umask 077
  if ! owned_temp="$(mktemp -- "$target_dir/.${target_name}.tmp.XXXXXXXXXX")"; then
    secure_env_error "could not create a private temporary environment file in $target_dir"
    return 1
  fi

  if ! install -m 600 -- "$template" "$owned_temp"; then
    secure_env_error "could not copy the environment template into a private temporary file"
    return 1
  fi
  if ! assert_secure_env "$owned_temp"; then
    secure_env_error "temporary environment file failed owner/mode/readback verification"
    return 1
  fi
  if ! cmp -s -- "$template" "$owned_temp"; then
    secure_env_error "temporary environment file failed content readback verification"
    return 1
  fi

  # A hard-link promotion is atomic, same-filesystem, and fails if TARGET exists.
  # This gives create-if-absent semantics without a check-then-rename overwrite.
  if ln -- "$owned_temp" "$target_path" 2>/dev/null; then
    if ! rm -f -- "$owned_temp"; then
      secure_env_error "environment file was created, but its private temporary link could not be removed"
      return 1
    fi
    owned_temp=""
    assert_secure_env "$target_path"
    return
  fi

  # A concurrent creator may have won. Remove only this invocation's private
  # temporary file and validate the winner; never alter the target.
  if ! rm -f -- "$owned_temp"; then
    secure_env_error "could not remove the losing private temporary environment file"
    return 1
  fi
  owned_temp=""
  if [[ -e "$target_path" || -L "$target_path" ]]; then
    assert_secure_env "$target_path"
    return
  fi

  secure_env_error "could not atomically promote the environment file: $target_path"
  return 1
)
