#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SOURCE_HELPER="$ROOT_DIR/scripts/lib/secure-env.sh"

fail() {
  echo "rpi-security-bundle: $*" >&2
  return 1
}

sha256_of() {
  sha256sum -- "$1" | awk '{print $1}'
}

validate_bundle() {
  local stage_dir="$1"
  local staged_helper
  local source_hash
  local staged_hash
  local launcher
  local postdeploy

  [[ -d "$stage_dir" ]] || { fail "stage directory does not exist: $stage_dir"; return 1; }
  stage_dir="$(cd -- "$stage_dir" && pwd -P)"
  staged_helper="$stage_dir/scripts/lib/secure-env.sh"

  [[ -e "$staged_helper" ]] || { fail "missing exact helper path: scripts/lib/secure-env.sh"; return 1; }
  [[ ! -L "$staged_helper" && -f "$staged_helper" ]] || { fail "staged helper must be a regular non-symlink file"; return 1; }
  [[ -x "$staged_helper" ]] || { fail "staged helper is not executable"; return 1; }

  source_hash="$(sha256_of "$SOURCE_HELPER")"
  staged_hash="$(sha256_of "$staged_helper")"
  [[ "$staged_hash" == "$source_hash" ]] || {
    fail "staged helper SHA-256 mismatch (expected $source_hash, found $staged_hash)"
    return 1
  }

  for launcher in prod-bootstrap.sh prod-dashboard.sh prod-bot.sh; do
    launcher="$stage_dir/scripts/$launcher"
    [[ -f "$launcher" && ! -L "$launcher" ]] || { fail "missing production entrypoint: ${launcher##*/}"; return 1; }
    grep -Fq 'source "$ROOT_DIR/scripts/lib/secure-env.sh"' "$launcher" || {
      fail "${launcher##*/} does not source scripts/lib/secure-env.sh"
      return 1
    }
    grep -Fq 'assert_secure_env "$ROOT_DIR/.env"' "$launcher" || {
      fail "${launcher##*/} does not verify .env before launch"
      return 1
    }
  done

  postdeploy="$stage_dir/scripts/remote-rpi-postdeploy.sh"
  [[ -f "$postdeploy" && ! -L "$postdeploy" ]] || { fail "missing remote-rpi-postdeploy.sh"; return 1; }
  grep -Fq '"$APP_DIR"/scripts/lib/*.sh' "$postdeploy" || {
    fail "remote postdeploy does not restore helper executable mode"
    return 1
  }
  grep -Fq 'source "$APP_DIR/scripts/lib/secure-env.sh"' "$postdeploy" || {
    fail "remote postdeploy does not source the exact helper path"
    return 1
  }
  grep -Fq 'create_secure_env "$APP_DIR/.env.example" "$APP_DIR/.env"' "$postdeploy" || {
    fail "remote postdeploy does not use create_secure_env"
    return 1
  }
}

make_fixture() {
  local stage_dir="$1"
  rm -rf -- "$stage_dir"
  mkdir -p "$stage_dir/scripts/lib"
  cp "$SOURCE_HELPER" "$stage_dir/scripts/lib/secure-env.sh"
  cp "$ROOT_DIR/scripts/prod-bootstrap.sh" "$stage_dir/scripts/prod-bootstrap.sh"
  cp "$ROOT_DIR/scripts/prod-dashboard.sh" "$stage_dir/scripts/prod-dashboard.sh"
  cp "$ROOT_DIR/scripts/prod-bot.sh" "$stage_dir/scripts/prod-bot.sh"
  cp "$ROOT_DIR/scripts/remote-rpi-postdeploy.sh" "$stage_dir/scripts/remote-rpi-postdeploy.sh"
  chmod +x "$stage_dir"/scripts/*.sh "$stage_dir"/scripts/lib/*.sh
}

expect_invalid_fixture() {
  local name="$1"
  local stage_dir="$2"
  if validate_bundle "$stage_dir" >/dev/null 2>&1; then
    fail "$name fixture unexpectedly passed"
    exit 1
  fi
}

self_test() {
  local tmp
  local stage
  local archive
  local app_dir
  local fake_bin
  local first_env_hash
  local postdeploy_status
  local assert_no_temp_pattern
  local symlink_hash
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' RETURN
  stage="$tmp/stage"

  make_fixture "$stage"
  validate_bundle "$stage"

  rm "$stage/scripts/lib/secure-env.sh"
  expect_invalid_fixture "missing helper" "$stage"

  make_fixture "$stage"
  mv "$stage/scripts/lib/secure-env.sh" "$stage/scripts/secure-env.sh"
  expect_invalid_fixture "flattened helper" "$stage"

  make_fixture "$stage"
  printf '\n# fixture hash mismatch\n' >>"$stage/scripts/lib/secure-env.sh"
  expect_invalid_fixture "hash mismatch" "$stage"

  make_fixture "$stage"
  chmod 600 "$stage/scripts/lib/secure-env.sh"
  expect_invalid_fixture "non-executable helper" "$stage"

  make_fixture "$stage"
  sed -i 's#source "$ROOT_DIR/scripts/lib/secure-env.sh"#source "$ROOT_DIR/scripts/secure-env.sh"#' \
    "$stage/scripts/prod-bot.sh"
  expect_invalid_fixture "entrypoint source drift" "$stage"

  # Exercise the real remote extraction path. The first deployment creates a
  # mode-600 file even under umask 000 and stops for operator input. A later
  # deployment preserves it despite a changed template.
  make_fixture "$stage"
  mkdir -p "$stage/target/release"
  printf 'TOKEN=remote-alpha-secret\n' >"$stage/.env.example"
  : >"$stage/target/release/dynamo-bootstrap"
  : >"$stage/target/release/dynamo-dashboard"
  : >"$stage/target/release/dynamo-bot"
  chmod +x "$stage/target/release"/dynamo-*
  archive="$tmp/bundle.tar"
  app_dir="$tmp/app"
  fake_bin="$tmp/fake-bin"
  mkdir -p "$fake_bin" "$tmp/home"
  cat >"$fake_bin/pm2" <<'EOF'
#!/usr/bin/env bash
if [[ "${1:-}" == "jlist" ]]; then
  printf '[]\n'
fi
exit 0
EOF
  cat >"$fake_bin/node" <<'EOF'
#!/usr/bin/env bash
# The postdeploy contract only needs a successful parser for the fake empty
# `pm2 jlist`; the production host uses its real Node binary.
cat >/dev/null
exit 0
EOF
  chmod +x "$fake_bin/pm2" "$fake_bin/node"

  tar -C "$stage" -cf "$archive" .
  postdeploy_status=0
  if ( umask 000; HOME="$tmp/home" PATH="$fake_bin:$PATH" \
    bash "$ROOT_DIR/scripts/remote-rpi-postdeploy.sh" "$app_dir" skip "$archive" \
    >"$tmp/first-postdeploy.stdout" 2>"$tmp/first-postdeploy.stderr" ); then
    fail "first remote postdeploy unexpectedly continued past .env creation"
  else
    postdeploy_status=$?
  fi
  [[ "$postdeploy_status" -eq 1 ]] || fail "first remote postdeploy returned $postdeploy_status instead of 1"
  [[ "$(stat -c '%a' "$app_dir/.env")" == "600" ]] || fail "remote postdeploy created a non-600 .env"
  cmp -s "$stage/.env.example" "$app_dir/.env" || fail "remote postdeploy .env bytes mismatch"
  if grep -q 'remote-alpha-secret' "$tmp/first-postdeploy.stdout" "$tmp/first-postdeploy.stderr"; then
    fail "remote postdeploy leaked environment contents"
  fi
  assert_no_temp_pattern="$app_dir/..env.tmp.*"
  if compgen -G "$assert_no_temp_pattern" >/dev/null; then
    fail "remote postdeploy left a private environment temp"
  fi

  first_env_hash="$(sha256_of "$app_dir/.env")"
  printf 'TOKEN=remote-beta-secret\n' >"$stage/.env.example"
  tar -C "$stage" -cf "$archive" .
  HOME="$tmp/home" PATH="$fake_bin:$PATH" \
    bash "$ROOT_DIR/scripts/remote-rpi-postdeploy.sh" "$app_dir" skip "$archive" \
    >"$tmp/second-postdeploy.stdout" 2>"$tmp/second-postdeploy.stderr"
  [[ "$(sha256_of "$app_dir/.env")" == "$first_env_hash" ]] || fail "remote postdeploy replaced an existing .env"
  [[ "$(stat -c '%a' "$app_dir/.env")" == "600" ]] || fail "remote postdeploy changed existing .env mode"

  # A deployed symlink is rejected without modifying its referent.
  rm "$app_dir/.env"
  printf 'TOKEN=symlink-referent-secret\n' >"$tmp/referent.env"
  chmod 600 "$tmp/referent.env"
  symlink_hash="$(sha256_of "$tmp/referent.env")"
  ln -s "$tmp/referent.env" "$app_dir/.env"
  tar -C "$stage" -cf "$archive" .
  if HOME="$tmp/home" PATH="$fake_bin:$PATH" \
    bash "$ROOT_DIR/scripts/remote-rpi-postdeploy.sh" "$app_dir" skip "$archive" \
    >"$tmp/symlink-postdeploy.stdout" 2>"$tmp/symlink-postdeploy.stderr"; then
    fail "remote postdeploy accepted a symlink .env"
  fi
  [[ "$(sha256_of "$tmp/referent.env")" == "$symlink_hash" ]] || fail "remote postdeploy changed a symlink referent"
  grep -q '^secure-env: refusing symbolic-link' "$tmp/symlink-postdeploy.stderr" || \
    fail "remote postdeploy did not reject the symlink through secure-env"

  echo "Raspberry Pi security bundle Bash contract tests passed"
}

if [[ "${1:-}" == "--self-test" ]]; then
  self_test
elif [[ $# -eq 1 ]]; then
  validate_bundle "$1"
  echo "Raspberry Pi security bundle Bash contract passed: $1"
else
  echo "Usage: $0 STAGE_DIR | --self-test" >&2
  exit 2
fi
