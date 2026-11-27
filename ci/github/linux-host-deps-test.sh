#!/usr/bin/env bash
# Tests ci/github/linux-host-deps.sh's mirror fallback with stand-ins for sudo, timeout and apt-get.
# Nothing is installed and no network is used.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir "$work/bin" "$work/parts"

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
  *" Dir::Etc::SourceList=/dev/null "*) echo "install-cache" >> "$FAKE_LOG" ;;
  *" --no-download "*) echo "install" >> "$FAKE_LOG" ;;
  *) : ;;
esac
EOF
# Every requested package installed, as after a good install from the cache.
cat > "$work/bin/dpkg-query" <<'FAKE'
#!/usr/bin/env bash
shift 3
for _ in "$@"; do echo "install ok installed"; done
FAKE
# ci/github/apt-cache.sh, through DW_APT_CACHE_TOOL: verify passes unless FAKE_VERIFY=fail.
cat > "$work/apt-cache" <<'FAKE'
#!/usr/bin/env bash
echo "$1" >> "$FAKE_LOG"
[[ "$1" != verify || "${FAKE_VERIFY:-}" != fail ]]
FAKE
chmod +x "$work/bin/sudo" "$work/bin/timeout" "$work/bin/apt-get" "$work/bin/dpkg-query" "$work/apt-cache"

regional=$'http://us-east-2.ec2.archive.ubuntu.com/ubuntu/\tpriority:1'
main=$'https://archive.ubuntu.com/ubuntu/\tpriority:2'
security=$'https://security.ubuntu.com/ubuntu/\tpriority:3'
azure=$'http://azure.archive.ubuntu.com/ubuntu/\tpriority:1'

run() {
  : > "$work/log"
  PATH="$work/bin:$PATH" DW_APT_MIRRORS="$work/mirrors" DW_APT_SOURCES="$work/sources" FAKE_LOG="$work/log" \
    DW_APT_SOURCE_PARTS="$work/parts" \
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

# A packages.microsoft.com source is dropped before the update; every other source stays.
write_sources
printf '%s\n%s\n%s\n' "$regional" "$main" "$security" > "$work/mirrors"
echo 'deb [arch=amd64] https://packages.microsoft.com/ubuntu/24.04/prod noble main' > "$work/parts/microsoft-prod.list"
printf 'Types: deb\nURIs: https://packages.microsoft.com/repos/azure-cli/\nSuites: noble\n' > "$work/parts/azure-cli.sources"
echo 'deb http://ppa.launchpadcontent.net/example/ppa/ubuntu noble main' > "$work/parts/other.list"
run http://us-east-2.ec2. || fail "a packages.microsoft.com source: the script failed"
[ ! -e "$work/parts/microsoft-prod.list" ] || fail "the packages.microsoft.com .list source was kept"
[ ! -e "$work/parts/azure-cli.sources" ] || fail "the packages.microsoft.com .sources source was kept"
[ -f "$work/parts/other.list" ] || fail "a source not on packages.microsoft.com was dropped"
rm -f "$work/parts/other.list"
echo "linux-host-deps: packages.microsoft.com sources dropped"

# The Actions cache (.github/actions/linux-host-deps): a hit installs from it; live-first tries once.
mkdir -p "$work/cache/archives"
touch "$work/cache/archives/a.deb"
run_cached() {
  : > "$work/log"
  : > "$work/summary"
  PATH="$work/bin:$PATH" DW_APT_MIRRORS="$work/mirrors" DW_APT_SOURCES="$work/sources" FAKE_LOG="$work/log" \
    DW_APT_SOURCE_PARTS="$work/parts" \
    FAKE_GOOD_MIRROR="$1" DW_APT_CACHE="$work/cache" DW_APT_CACHE_TOOL="$work/apt-cache" \
    GITHUB_STEP_SUMMARY="$work/summary" "${@:2}" "$here/linux-host-deps.sh" rust > /dev/null 2> "$work/err"
}
no_summary() {
  [ ! -s "$work/summary" ] || fail "$1: the summary was: $(cat "$work/summary")"
}
images_mirrors() {
  write_sources
  printf '%s\n%s\n%s\n' "$regional" "$main" "$security" > "$work/mirrors"
}

images_mirrors
run_cached nowhere env || fail "a cache hit: the script failed"
expect_log "verify install-cache " "a cache hit"
no_summary "a cache hit"

images_mirrors
run_cached http://us-east-2.ec2. env DW_APT_LIVE_FIRST=1 || fail "live-first, live works: the script failed"
expect_log "download http://us-east-2.ec2.archive.ubuntu.com/ubuntu/ install " "live-first, live works"
no_summary "live-first, live works"

images_mirrors
run_cached nowhere env DW_APT_LIVE_FIRST=1 || fail "live-first, live fails: the script failed"
expect_log "download http://us-east-2.ec2.archive.ubuntu.com/ubuntu/ verify install-cache " "live-first, live fails"
expect_mirrors "$(printf '%s\n%s\n%s' "$regional" "$main" "$security")" "live-first, live fails"
expect_security http://security.ubuntu.com/ubuntu/ "live-first, live fails"
grep -q "^apt: cache fallback used: " "$work/summary" || fail "live-first, live fails: the summary was: $(cat "$work/summary")"

images_mirrors
if run_cached nowhere env DW_APT_LIVE_FIRST=1 FAKE_VERIFY=fail; then
  fail "live-first, live fails, the cache does not verify: the script passed"
fi
expect_log "download http://us-east-2.ec2.archive.ubuntu.com/ubuntu/ verify " "live-first, live fails, the cache does not verify"
no_summary "live-first, live fails, the cache does not verify"

rm "$work/cache/archives/a.deb"
images_mirrors
run_cached http://us-east-2.ec2. env || fail "a cache miss: the script failed"
expect_log "download http://us-east-2.ec2.archive.ubuntu.com/ubuntu/ save install " "a cache miss"
no_summary "a cache miss"
images_mirrors
run_cached http://us-east-2.ec2. env DW_APT_SAVE=0 || fail "a cache miss on a pull request: the script failed"
expect_log "download http://us-east-2.ec2.archive.ubuntu.com/ubuntu/ install " "a cache miss on a pull request"

echo "linux-host-deps: cache, live-first and fallback ok"

# The pockets apt-cache.sh accepts, from ubuntu.sources read as libapt reads deb822.
expect_suites() {
  local got
  got=$("$here/apt-cache.sh" suites "$work/deb822" | tr '\n' ' ')
  [ "$got" == "$1 " ] || fail "$2: suites were '$got', not '$1 '"
}
printf '%s\n' '# Suites: commented' 'Types: deb' 'Suites: noble noble-updates' ' noble-backports' '' \
  'Types: deb' $'suites:\tnoble-security' > "$work/deb822"
expect_suites "noble noble-updates noble-backports noble-security" "comments, folding and case"
for off in 'Enabled: no' 'Enabled: false' 'enabled: Off' 'Enabled:' ; do
  printf '%s\n' 'Suites: noble' '' 'Suites: noble-proposed' "$off" > "$work/deb822"
  [ "$off" != 'Enabled:' ] || printf '%s\n' 'Suites: noble' '' 'Suites: noble-proposed' 'Enabled:' ' no' > "$work/deb822"
  expect_suites "noble" "a stanza with '$off'"
done
printf '%s\n' 'Suites: noble' '' 'Suites: noble-proposed' 'Enabled: 0' '' 'Suites : noble-updates' '' \
  'Suites: noble-backports' 'Enabled : no' '' 'Suites: noble-security' 'Enabled: fal se' > "$work/deb822"
expect_suites "noble noble-updates noble-security" "Enabled: 0, spaces before a colon, and an unknown Enabled value"
printf '%s\n' 'Suites: noble-proposed' ' ' 'Enabled: no' '' 'Suites: noble' > "$work/deb822"
expect_suites "noble" "a line of spaces inside a disabled stanza"
printf 'Suites: noble noble-updates\r\nEnabled: yes\r\n\r\nSuites: noble-proposed\r\nEnabled: no\r\n' > "$work/deb822"
expect_suites "noble noble-updates" "CRLF line ends"

echo "linux-host-deps: deb822 suites ok"
