#!/usr/bin/env bash
# Test of ci/github/release-publish.sh (and release-version.sh) against a fake `gh` and a scratch git
# repository: what each trigger publishes, and that nothing but a push to main or a release tag, on a
# commit that is on main, ever reaches a write.
#
#   ci/github/release-publish-test.sh
# shellcheck disable=SC2016,SC2329 # check() takes expressions as strings; helpers run inside eval
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@t GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@t

# The history: c1 <- c2 (origin/main); side branches off c1; c3 on top of c2 not yet in origin/main's ref.
repo=$work/repo
git init -q "$repo"
cd "$repo"
git commit -q --allow-empty -m c1
c1=$(git rev-parse HEAD)
git commit -q --allow-empty -m c2
c2=$(git rev-parse HEAD)
git update-ref refs/remotes/origin/main "$c2"
git checkout -q -b side "$c1"
git commit -q --allow-empty -m side
side=$(git rev-parse HEAD)
git checkout -q -b newer "$c2"
git commit -q --allow-empty -m newer
newer=$(git rev-parse HEAD)

# A fake gh: logs every call, answers the reads from FAKE_* and keeps the release's assets in a file.
bin=$work/bin
mkdir "$bin"
cat > "$bin/gh" <<'FAKE'
#!/usr/bin/env bash
echo "$*" >> "$LOG"
notfound() { echo "gh: Not Found (HTTP 404)" >&2; exit 1; }
case "$1 $2" in
  "release view")
    if [[ " ${FAKE_RELEASES:-} " != *" $3 "* ]]; then echo "release not found" >&2; exit 1; fi
    case "$*" in *--json*) printf 'false\t%s\n' "${FAKE_ASSET_COUNT:-5}" ;; esac
    exit 0 ;;
  "release upload") n=${4##*/}; echo "$((100 + $(wc -l < "$ASSETS")))	$n" >> "$ASSETS"; exit 0 ;;
  "api repos/"*)
    case "$*" in
      *git/ref/tags/continuous*)
        [[ -z "${FAKE_API_ERROR:-}" ]] || { echo "gh: Service Unavailable (HTTP 503)" >&2; exit 1; }
        [[ -n "${FAKE_CONT:-}" ]] && echo "$FAKE_CONT" || notfound; exit 0 ;;
      *releases/tags/continuous*) echo 42; exit 0 ;;
      *releases/42/assets*) cat "$ASSETS"; exit 0 ;;
    esac ;;
esac
exit 0
FAKE
chmod +x "$bin/gh"
export PATH="$bin:$PATH" LOG=$work/log ASSETS=$work/assets

dist=$work/dist
mkdir -p "$dist"
ver=0.1.0-dev.7.g${c2:0:7}
for s in macos-universal.dmg windows-x64-setup.exe linux-x86_64.AppImage linux-amd64.deb; do echo "$s" > "$dist/dash-wallet-$ver-$s"; done
(cd "$dist" && sha256sum dash-wallet-* > SHA256SUMS)
export DIST=$dist OUT=$work/publish GITHUB_REPOSITORY=o/r GITHUB_RUN_NUMBER=7 GITHUB_RUN_ID=99 GITHUB_SERVER_URL=https://github.com GH_TOKEN=x

fail=0
check() { if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi; }
# run <event> <ref> <sha> <version> [mode ...]: prepare then publish; sets $status, $LOG
run() {
  local event=$1 ref=$2 sha=$3
  export VERSION=$4 GITHUB_EVENT_NAME=$event GITHUB_REF=$ref GITHUB_SHA=$sha
  : > "$LOG"
  status=0
  "$here/release-publish.sh" prepare > "$work/prep.out" 2>&1 || status=$?
  [[ $status -ne 0 ]] || "$here/release-publish.sh" publish > "$work/pub.out" 2>&1 || status=$?
}
refused() { check "$1" '[[ $status -ne 0 && ! -s "$LOG" ]]'; }
log_has() { grep -qF -- "$1" "$LOG"; }
line_of() { grep -nF -- "$1" "$LOG" | head -1 | cut -d: -f1; }

# --- refusals: no write, no gh call at all
run push refs/heads/dw/rel-release "$c2" "$ver";             refused "push to a feature branch publishes nothing"
run pull_request refs/pull/5/merge "$c2" "$ver";             refused "a pull request publishes nothing"
run workflow_dispatch refs/heads/main "$c2" "$ver";          refused "workflow_dispatch on main publishes nothing"
run workflow_dispatch refs/heads/dw/x "$c2" "$ver";          refused "workflow_dispatch on a branch publishes nothing"
run push refs/tags/v0.0.0-rel8 "$c2" 0.0.0-rel8;             refused "a dry-run tag publishes nothing"
check "a dry-run tag is skipped (exit 3), not failed" '[[ $status -eq 3 ]]'
run push refs/tags/v0.1.0-dev.3+abc "$c2" 0.1.0-dev.3+abc;   refused "a dev tag does not trigger a publish"
check "a dev tag is skipped (exit 3)" '[[ $status -eq 3 ]]'
run push refs/tags/v0.1.0 "$side" 0.1.0;                     refused "a release tag on a commit outside main publishes nothing"
run push refs/heads/main "$side" "0.1.0-dev.7.g${side:0:7}"; refused "main pointing outside origin/main publishes nothing"

# --- push to main, first publish
: > "$ASSETS"
run push refs/heads/main "$c2" "$ver"
check "main: succeeds" '[[ $status -eq 0 ]]'
check "main: attest dir holds 4 bundles, SHA256SUMS, 4 continuous files, SHA256SUMS.continuous" '[[ $(ls "$OUT/attest" | wc -l) -eq 10 ]]'
check "main: continuous SHA256SUMS lists stable names only" '[[ $(grep -c "DashWallet-continuous-" "$OUT/continuous/SHA256SUMS") -eq 4 && $(wc -l < "$OUT/continuous/SHA256SUMS") -eq 4 ]]'
tag="v0.1.0-dev.7+${c2:0:7}"
check "main: immutable prerelease $tag, at the commit, a prerelease, not latest" \
  'log_has "release create $tag " && log_has "--target $c2" && log_has "--prerelease" && log_has "--latest=false"'
check "main: no draft, no delete" '! log_has "--draft" && ! log_has "DELETE"'
check "main: creates continuous" 'log_has "release create continuous "'

# --- push to main again: the per-run release exists, continuous exists at the older commit
: > "$ASSETS"
for n in DashWallet-continuous-macos-universal.dmg DashWallet-continuous-windows-x64-setup.exe DashWallet-continuous-linux-x86_64.AppImage DashWallet-continuous-linux-amd64.deb SHA256SUMS; do echo "$((1 + $(wc -l < "$ASSETS")))	$n" >> "$ASSETS"; done
FAKE_RELEASES="$tag continuous" FAKE_CONT="$c1" run push refs/heads/main "$c2" "$ver"
check "main rerun: succeeds" '[[ $status -eq 0 ]]'
check "main rerun: the immutable release is not created again" '! log_has "release create"'
check "main rerun: assets uploaded as .new, then renamed" 'log_has "release upload continuous" && log_has "name=DashWallet-continuous-macos-universal.dmg.old" && log_has "name=DashWallet-continuous-macos-universal.dmg" '
check "main rerun: SHA256SUMS is swapped last" '[[ $(line_of "name=SHA256SUMS.old") -gt $(line_of "name=DashWallet-continuous-linux-amd64.deb.old") ]]'
check "main rerun: the tag moves after the assets" '[[ $(line_of "git/refs/tags/continuous") -gt $(line_of "name=SHA256SUMS.old") ]] && log_has "-f sha=$c2"'

# --- an older run finishing after a newer one must not move continuous back
FAKE_RELEASES="continuous" FAKE_CONT="$newer" run push refs/heads/main "$c2" "$ver"
check "older run: succeeds, creates its own release, leaves continuous alone" \
  '[[ $status -eq 0 ]] && log_has "release create $tag " && ! log_has "release upload" && ! log_has "git/refs/tags/continuous"'

# --- a failing read is not "absent": no write happens
FAKE_RELEASES="$tag continuous" FAKE_API_ERROR=1 run push refs/heads/main "$c2" "$ver"
check "API error on the continuous lookup: fails before any write" \
  '[[ $status -ne 0 ]] && ! log_has "release upload" && ! log_has "PATCH" && ! log_has "POST" && ! log_has "release create"'
# --- an incomplete release that exists is an error, not a success
FAKE_RELEASES="$tag continuous" FAKE_ASSET_COUNT=3 run push refs/heads/main "$c2" "$ver"
check "release exists with 3 of 5 assets: fails, writes nothing" '[[ $status -ne 0 ]] && ! log_has "release create" && ! log_has "PATCH"'
# --- classify
check "classify: main push" '[[ $(GITHUB_EVENT_NAME=push GITHUB_REF=refs/heads/main "$here/release-publish.sh" classify) == main ]]'
check "classify: release tag" '[[ $(GITHUB_EVENT_NAME=push GITHUB_REF=refs/tags/v0.1.0-rc.2 "$here/release-publish.sh" classify) == tag ]]'
check "classify: dry-run tag, branch, PR, dispatch" '[[ $({ for r in refs/tags/v0.0.0-rel8 refs/heads/dw/x; do GITHUB_EVENT_NAME=push GITHUB_REF=$r "$here/release-publish.sh" classify; done; for e in pull_request workflow_dispatch; do GITHUB_EVENT_NAME=$e GITHUB_REF=refs/heads/main "$here/release-publish.sh" classify; done; } | tr "\n" " ") == "skip skip skip skip " ]]'

# --- tag on main: its own release, nothing rolling
mkdir -p "$work/tag-dist"
ver=0.1.0
rm -rf "$dist"/dash-wallet-*; for s in macos-universal.dmg windows-x64-setup.exe linux-x86_64.AppImage linux-amd64.deb; do echo "$s" > "$dist/dash-wallet-$ver-$s"; done
(cd "$dist" && sha256sum dash-wallet-* > SHA256SUMS)
run push refs/tags/v0.1.0 "$c2" 0.1.0
check "tag: succeeds" '[[ $status -eq 0 ]]'
check "tag: a prerelease from the existing tag" 'log_has "release create v0.1.0 " && log_has "--verify-tag" && log_has "--prerelease"'
check "tag: no continuous, no tag moved" '! log_has "continuous" && ! log_has "git/refs"'
check "tag: attests 5 files (4 bundles and SHA256SUMS)" '[[ $(ls "$OUT/attest" | wc -l) -eq 5 ]]'

# --- release-version.sh
cd "$here/../.."
v() { env -i PATH="$PATH" GITHUB_ENV="$work/env" "$@" ci/github/release-version.sh > /dev/null 2>&1 && tail -1 "$work/env"; }
: > "$work/env"
check "version: main push" '[[ $(v GITHUB_REF_TYPE=branch GITHUB_SHA=1234567890 GITHUB_RUN_NUMBER=5) == "VERSION=0.1.0-dev.5.g1234567" ]]'
check "version: tag v0.1.0" '[[ $(v GITHUB_REF_TYPE=tag GITHUB_REF_NAME=v0.1.0) == "VERSION=0.1.0" ]]'
check "version: dry-run tag" '[[ $(v GITHUB_REF_TYPE=tag GITHUB_REF_NAME=v0.0.0-rel8) == "VERSION=0.0.0-rel8" ]]'
check "version: rc tag of the same version" '[[ $(v GITHUB_REF_TYPE=tag GITHUB_REF_NAME=v0.1.0-rc.1) == "VERSION=0.1.0-rc.1" ]]'
check "version: rc tag of another version fails" '! v GITHUB_REF_TYPE=tag GITHUB_REF_NAME=v0.2.0-rc.1 >/dev/null'
check "version: a tag that differs from tauri.conf.json fails" '! v GITHUB_REF_TYPE=tag GITHUB_REF_NAME=v9.9.9 >/dev/null'

exit $fail
