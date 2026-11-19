#!/usr/bin/env bash
# Tests ci/github/linux-host-deps.sh's mirror fallback with stand-ins for sudo, timeout and apt-get.
# Nothing is installed and no network is used.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir "$work/bin"

cat > "$work/bin/sudo" <<'EOF'
#!/usr/bin/env bash
exec env "$@"
EOF
cat > "$work/bin/timeout" <<'EOF'
#!/usr/bin/env bash
shift 2
exec "$@"
EOF
# A download succeeds only from the mirror named in FAKE_GOOD_MIRROR, as if every other one stalled.
cat > "$work/bin/apt-get" <<'EOF'
#!/usr/bin/env bash
first=$(head -n 1 "$DW_APT_MIRRORS" 2>/dev/null || true)
case " $* " in
  *" --download-only "*)
    echo "download ${first%%[[:space:]]*}" >> "$FAKE_LOG"
    [[ "$first" == "$FAKE_GOOD_MIRROR"* ]] ;;
  *" --no-download "*) echo "install" >> "$FAKE_LOG" ;;
  *) : ;;
esac
EOF
chmod +x "$work/bin/sudo" "$work/bin/timeout" "$work/bin/apt-get"

regional=$'http://us-east-2.ec2.archive.ubuntu.com/ubuntu/\tpriority:1'
main=$'https://archive.ubuntu.com/ubuntu/\tpriority:2'
security=$'https://security.ubuntu.com/ubuntu/\tpriority:3'

run() {
  : > "$work/log"
  PATH="$work/bin:$PATH" DW_APT_MIRRORS="$work/mirrors" FAKE_LOG="$work/log" FAKE_GOOD_MIRROR="$1" \
    "$here/linux-host-deps.sh" rust > /dev/null 2>&1
}
fail() {
  echo "FAIL: $1" >&2
  exit 1
}
expect_log() { [ "$(tr '\n' ' ' < "$work/log")" = "$1" ] || fail "$2: log was: $(tr '\n' ' ' < "$work/log")"; }
expect_mirrors() { [ "$(cat "$work/mirrors")" = "$1" ] || fail "$2: mirrors were: $(cat "$work/mirrors")"; }

printf '%s\n%s\n%s\n' "$regional" "$main" "$security" > "$work/mirrors"
run https://archive.ubuntu.com/ || fail "a stalled regional mirror: the script failed"
expect_log "download http://us-east-2.ec2.archive.ubuntu.com/ubuntu/ download https://archive.ubuntu.com/ubuntu/ install " "a stalled regional mirror"
expect_mirrors "$(printf '%s\n%s' "$main" "$security")" "a stalled regional mirror"

printf '%s\n%s\n%s\n' "$regional" "$main" "$security" > "$work/mirrors"
run http://us-east-2.ec2. || fail "a healthy regional mirror: the script failed"
expect_log "download http://us-east-2.ec2.archive.ubuntu.com/ubuntu/ install " "a healthy regional mirror"
expect_mirrors "$(printf '%s\n%s\n%s' "$regional" "$main" "$security")" "a healthy regional mirror"

printf '%s\n%s\n' "$main" "$security" > "$work/mirrors"
if run nowhere; then fail "every mirror stalled: the script passed"; fi
expect_log "download https://archive.ubuntu.com/ubuntu/ download https://archive.ubuntu.com/ubuntu/ download https://archive.ubuntu.com/ubuntu/ " "every mirror stalled"
expect_mirrors "$(printf '%s\n%s' "$main" "$security")" "every mirror stalled"

printf '%s\n%s\n' "$regional" "$security" > "$work/mirrors"
if run nowhere; then fail "no main mirror listed: the script passed"; fi
expect_mirrors "$(printf '%s\n%s' "$regional" "$security")" "no main mirror listed"

rm "$work/mirrors"
if run nowhere; then fail "no mirror list: the script passed"; fi
[ ! -e "$work/mirrors" ] || fail "no mirror list: the script created one"

echo "linux-host-deps: mirror fallback ok"
