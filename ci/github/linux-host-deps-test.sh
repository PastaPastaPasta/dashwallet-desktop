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
azure=$'http://azure.archive.ubuntu.com/ubuntu/\tpriority:1'

run() {
  : > "$work/log"
  PATH="$work/bin:$PATH" DW_APT_MIRRORS="$work/mirrors" DW_APT_SOURCES="$work/sources" FAKE_LOG="$work/log" \
    FAKE_GOOD_MIRROR="$1" "$here/linux-host-deps.sh" rust > /dev/null 2> "$work/err"
}
fail() {
  echo "FAIL: $1" >&2
  exit 1
}
expect_log() { [ "$(tr '\n' ' ' < "$work/log")" = "$1" ] || fail "$2: log was: $(tr '\n' ' ' < "$work/log")"; }
expect_mirrors() { [ "$(cat "$work/mirrors")" = "$1" ] || fail "$2: mirrors were: $(cat "$work/mirrors")"; }
expect_security() {
  [ "$(grep -A1 -xF "URIs: $1" "$work/sources" | tail -n 1)" = "Suites: noble-security" ] ||
    fail "$2: sources were: $(cat "$work/sources")"
}
from_list="mirror+file:$work/mirrors"
write_sources() {
  printf 'Types: deb\nURIs: mirror+file:%s\nSuites: noble noble-updates\n\n' "$work/mirrors" > "$work/sources"
  printf 'Types: deb\nURIs: http://security.ubuntu.com/ubuntu/\nSuites: noble-security\n' >> "$work/sources"
}

write_sources
printf '%s\n%s\n%s\n' "$regional" "$main" "$security" > "$work/mirrors"
run http://azure.archive. || fail "the image's mirrors stalled: the script failed"
expect_log "download http://us-east-2.ec2.archive.ubuntu.com/ubuntu/ download http://azure.archive.ubuntu.com/ubuntu/ install " "the image's mirrors stalled"
expect_mirrors "$(printf '%s\n%s\n%s' "$azure" "$main" "$security")" "the image's mirrors stalled"
expect_security "$from_list" "the image's mirrors stalled"
[ "$(grep -cxF "URIs: $from_list" "$work/sources")" = 2 ] || fail "the mirror stanza changed"

write_sources
printf '%s\n%s\n%s\n' "$regional" "$main" "$security" > "$work/mirrors"
run http://us-east-2.ec2. || fail "a healthy regional mirror: the script failed"
expect_log "download http://us-east-2.ec2.archive.ubuntu.com/ubuntu/ install " "a healthy regional mirror"
expect_mirrors "$(printf '%s\n%s\n%s' "$regional" "$main" "$security")" "a healthy regional mirror"
expect_security http://security.ubuntu.com/ubuntu/ "a healthy regional mirror"

write_sources
printf '%s\n%s\n%s\n' "$regional" "$main" "$security" > "$work/mirrors"
if run nowhere; then fail "every mirror stalled: the script passed"; fi
expect_log "download http://us-east-2.ec2.archive.ubuntu.com/ubuntu/ download http://azure.archive.ubuntu.com/ubuntu/ download https://archive.ubuntu.com/ubuntu/ " "every mirror stalled"
expect_mirrors "$(printf '%s\n%s\n%s' "$azure" "$main" "$security")" "every mirror stalled"
expect_security "$from_list" "every mirror stalled"
[ "$(wc -l < "$work/sources")" = 7 ] || fail "every mirror stalled: sources were: $(cat "$work/sources")"

write_sources
printf '%s\n%s\n%s\n' "$azure" "$main" "$security" > "$work/mirrors"
run https://archive.ubuntu. || fail "the Azure mirror is the regional one and stalled: the script failed"
expect_log "download http://azure.archive.ubuntu.com/ubuntu/ download https://archive.ubuntu.com/ubuntu/ install " \
  "the Azure mirror is the regional one and stalled"
expect_mirrors "$(printf '%s\n%s\n%s' $'https://archive.ubuntu.com/ubuntu/\tpriority:1' \
  $'https://security.ubuntu.com/ubuntu/\tpriority:2' $'http://azure.archive.ubuntu.com/ubuntu/\tpriority:3')" \
  "the Azure mirror is the regional one and stalled"
expect_security "$from_list" "the Azure mirror is the regional one and stalled"

write_sources
printf '%s\n%s\n%s\n' "$regional" "$main" "$security" > "$work/mirrors"
run https://archive.ubuntu. || fail "the regional and Azure mirrors stalled: the script failed"
expect_log "download http://us-east-2.ec2.archive.ubuntu.com/ubuntu/ download http://azure.archive.ubuntu.com/ubuntu/ download https://archive.ubuntu.com/ubuntu/ install " \
  "the regional and Azure mirrors stalled"

write_sources
printf '%s\n' "$azure" > "$work/mirrors"
if run nowhere; then fail "only the Azure mirror listed: the script passed"; fi
expect_mirrors "$azure" "only the Azure mirror listed"

write_sources
printf '%s\n%s\n' "$regional" "$security" > "$work/mirrors"
run http://azure.archive. || fail "no main mirror listed: the script failed"
expect_mirrors "$(printf '%s\n%s\n%s' "$azure" $'http://us-east-2.ec2.archive.ubuntu.com/ubuntu/\tpriority:2' "$security")" \
  "no main mirror listed"
expect_security "$from_list" "no main mirror listed"

write_sources
printf '%s\n%s\n%s\n' "$regional" "$main" "$security" > "$work/mirrors"
chmod 000 "$work/sources"
if [ ! -r "$work/sources" ]; then
  if run http://azure.archive.; then fail "unreadable sources: the script passed"; fi
  chmod 600 "$work/sources"
  grep -qF "could not switch apt mirrors" "$work/err" || fail "unreadable sources: not reported: $(cat "$work/err")"
  expect_mirrors "$(printf '%s\n%s\n%s' "$regional" "$main" "$security")" "unreadable sources"
  expect_security http://security.ubuntu.com/ubuntu/ "unreadable sources"
fi
chmod 600 "$work/sources"

rm "$work/sources"
printf '%s\n%s\n' "$main" "$security" > "$work/mirrors"
run http://azure.archive. || fail "no sources file: the script failed"
[ ! -e "$work/sources" ] || fail "no sources file: the script created one"

rm "$work/mirrors"
if run nowhere; then fail "no mirror list: the script passed"; fi
[ ! -e "$work/mirrors" ] || fail "no mirror list: the script created one"

echo "linux-host-deps: mirror fallback ok"
