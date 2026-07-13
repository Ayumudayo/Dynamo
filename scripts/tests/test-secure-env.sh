#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# shellcheck source=../lib/secure-env.sh
source "$ROOT_DIR/scripts/lib/secure-env.sh"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

fail() {
  echo "test-secure-env: $*" >&2
  exit 1
}

expect_failure() {
  if "$@"; then
    fail "command unexpectedly succeeded: $*"
  fi
}

assert_no_owned_temp() {
  local directory="$1"
  local target_name="$2"
  if compgen -G "$directory/.${target_name}.tmp.*" >/dev/null; then
    fail "private temporary file was not cleaned: $directory/.${target_name}.tmp.*"
  fi
}

sha256_of() {
  sha256sum -- "$1" | awk '{print $1}'
}

printf 'TOKEN=alpha-secret\nVALUE=one\n' >"$tmp/template-a"
printf 'TOKEN=beta-secret\nVALUE=two\n' >"$tmp/template-b"

# umask 000 must not broaden the resulting secret file.
( umask 000; create_secure_env "$tmp/template-a" "$tmp/.env" )
[[ "$(stat -c '%a' "$tmp/.env")" == "600" ]] || fail "created mode is not 600"
[[ "$(stat -c '%u' "$tmp/.env")" == "$(id -u)" ]] || fail "created owner is not the effective user"
cmp -s "$tmp/template-a" "$tmp/.env" || fail "created bytes do not match the template"
assert_secure_env "$tmp/.env"

# Existing valid files return before reading TEMPLATE and remain byte-identical.
first_hash="$(sha256_of "$tmp/.env")"
create_secure_env "$tmp/template-b" "$tmp/.env"
create_secure_env "$tmp/does-not-exist" "$tmp/.env"
[[ "$(sha256_of "$tmp/.env")" == "$first_hash" ]] || fail "existing target was replaced"
cmp -s "$tmp/template-a" "$tmp/.env" || fail "existing target bytes changed"

# Broad permissions fail without changing bytes.
cp "$tmp/template-a" "$tmp/broad.env"
chmod 644 "$tmp/broad.env"
broad_hash="$(sha256_of "$tmp/broad.env")"
expect_failure assert_secure_env "$tmp/broad.env"
expect_failure create_secure_env "$tmp/template-b" "$tmp/broad.env"
[[ "$(sha256_of "$tmp/broad.env")" == "$broad_hash" ]] || fail "invalid existing target bytes changed"
[[ "$(stat -c '%a' "$tmp/broad.env")" == "644" ]] || fail "invalid existing target mode was silently repaired"

# Symlinks, dangling symlinks, and non-regular targets are always rejected.
cp "$tmp/template-a" "$tmp/symlink-referent"
referent_hash="$(sha256_of "$tmp/symlink-referent")"
ln -s "$tmp/symlink-referent" "$tmp/symlink.env"
expect_failure assert_secure_env "$tmp/symlink.env"
expect_failure create_secure_env "$tmp/template-b" "$tmp/symlink.env"
[[ "$(sha256_of "$tmp/symlink-referent")" == "$referent_hash" ]] || fail "symlink referent changed"
ln -s "$tmp/missing-referent" "$tmp/dangling.env"
expect_failure create_secure_env "$tmp/template-a" "$tmp/dangling.env"
mkdir "$tmp/directory.env"
expect_failure assert_secure_env "$tmp/directory.env"
expect_failure create_secure_env "$tmp/template-a" "$tmp/directory.env"

# Simulate a different effective owner without requiring root.
real_id="$(command -v id)"
mkdir "$tmp/wrong-owner-bin"
cat >"$tmp/wrong-owner-bin/id" <<EOF
#!/usr/bin/env bash
if [[ "\${1:-}" == "-u" ]]; then
  echo 4294967294
else
  exec "$real_id" "\$@"
fi
EOF
chmod +x "$tmp/wrong-owner-bin/id"
cp "$tmp/template-a" "$tmp/wrong-owner.env"
chmod 600 "$tmp/wrong-owner.env"
wrong_owner_hash="$(sha256_of "$tmp/wrong-owner.env")"
expect_failure env PATH="$tmp/wrong-owner-bin:$PATH" bash -c \
  'source "$1"; assert_secure_env "$2"' _ "$ROOT_DIR/scripts/lib/secure-env.sh" "$tmp/wrong-owner.env"
[[ "$(sha256_of "$tmp/wrong-owner.env")" == "$wrong_owner_hash" ]] || fail "wrong-owner target changed"

# Two creators may race, but the result is one complete input and is never replaced.
for iteration in $(seq 1 20); do
  race_target="$tmp/race-$iteration.env"
  ( create_secure_env "$tmp/template-a" "$race_target" ) &
  pid_a=$!
  ( create_secure_env "$tmp/template-b" "$race_target" ) &
  pid_b=$!
  wait "$pid_a"
  wait "$pid_b"
  assert_secure_env "$race_target"
  if ! cmp -s "$tmp/template-a" "$race_target" && ! cmp -s "$tmp/template-b" "$race_target"; then
    fail "race result is mixed or truncated on iteration $iteration"
  fi
  race_hash="$(sha256_of "$race_target")"
  create_secure_env "$tmp/template-a" "$race_target"
  create_secure_env "$tmp/template-b" "$race_target"
  [[ "$(sha256_of "$race_target")" == "$race_hash" ]] || fail "race winner was replaced"
  assert_no_owned_temp "$tmp" "race-$iteration.env"
done

# Copy and promotion failures leave neither a target nor a same-directory temp.
mkdir "$tmp/fail-install-bin"
cat >"$tmp/fail-install-bin/install" <<'EOF'
#!/usr/bin/env bash
exit 41
EOF
chmod +x "$tmp/fail-install-bin/install"
expect_failure env PATH="$tmp/fail-install-bin:$PATH" bash -c \
  'source "$1"; create_secure_env "$2" "$3"' _ \
  "$ROOT_DIR/scripts/lib/secure-env.sh" "$tmp/template-a" "$tmp/copy-failure.env"
[[ ! -e "$tmp/copy-failure.env" && ! -L "$tmp/copy-failure.env" ]] || fail "copy failure created a target"
assert_no_owned_temp "$tmp" "copy-failure.env"

mkdir "$tmp/fail-promotion-bin"
cat >"$tmp/fail-promotion-bin/ln" <<'EOF'
#!/usr/bin/env bash
exit 42
EOF
chmod +x "$tmp/fail-promotion-bin/ln"
expect_failure env PATH="$tmp/fail-promotion-bin:$PATH" bash -c \
  'source "$1"; create_secure_env "$2" "$3"' _ \
  "$ROOT_DIR/scripts/lib/secure-env.sh" "$tmp/template-a" "$tmp/promotion-failure.env"
[[ ! -e "$tmp/promotion-failure.env" && ! -L "$tmp/promotion-failure.env" ]] || fail "promotion failure created a target"
assert_no_owned_temp "$tmp" "promotion-failure.env"

# All production launchers reject broad mode and wrong ownership before exec,
# preserve bytes, and never print secret contents.
for launcher in prod-bootstrap.sh prod-dashboard.sh prod-bot.sh; do
  for rejection in broad wrong-owner; do
    fixture="$tmp/entrypoint-${launcher%.sh}-$rejection"
    mkdir -p "$fixture/scripts/lib" "$fixture/target/release"
    cp "$ROOT_DIR/scripts/$launcher" "$fixture/scripts/$launcher"
    cp "$ROOT_DIR/scripts/lib/secure-env.sh" "$fixture/scripts/lib/secure-env.sh"
    cp "$tmp/template-a" "$fixture/.env"
    chmod +x "$fixture/scripts/$launcher" "$fixture/scripts/lib/secure-env.sh"
    chmod 600 "$fixture/.env"
    launcher_path="$fixture/scripts/$launcher"
    launcher_path_env="$PATH"
    if [[ "$rejection" == "broad" ]]; then
      chmod 644 "$fixture/.env"
    else
      launcher_path_env="$tmp/wrong-owner-bin:$PATH"
    fi
    before_hash="$(sha256_of "$fixture/.env")"
    if env PATH="$launcher_path_env" "$launcher_path" >"$fixture/stdout" 2>"$fixture/stderr"; then
      fail "$launcher accepted a $rejection .env"
    fi
    grep -q '^secure-env:' "$fixture/stderr" || fail "$launcher did not reject through assert_secure_env"
    [[ "$(sha256_of "$fixture/.env")" == "$before_hash" ]] || fail "$launcher changed a rejected .env"
    if grep -q 'alpha-secret' "$fixture/stdout" "$fixture/stderr"; then
      fail "$launcher leaked environment contents"
    fi
  done
done

echo "secure-env tests passed"
