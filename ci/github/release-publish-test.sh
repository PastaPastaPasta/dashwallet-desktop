#!/usr/bin/env bash
# Test of ci/github/release-publish.sh, release-decide.sh and release-version.sh against a stateful fake
# `gh` and a scratch git repository:
#   - what each trigger publishes, and that nothing but a push to main or a release tag, on a commit that
#     is on main, ever reaches a write (the publish script's own check and the decide job's);
#   - the `continuous` swap under faults: a write that fails once, an outage from some write on, and a
#     kill of the script just before or just after some write, at every write of a full update. The end state
#     after the run (or after the next run, for an outage or a kill) is one complete generation: every
#     canonical asset present, binaries matching the SHA256SUMS next to them, no .old/.new left.
#
#   ci/github/release-publish-test.sh
# shellcheck disable=SC2015,SC2016,SC2034,SC2329 # check() takes expressions as strings; helpers run inside eval
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@t GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@t

# The history: c1 <- c2 (origin/main) <- newer (docs only) <- code; `side` branches off c1.
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
mkdir docs && echo a > docs/a.md && git add docs && git commit -q -m docs
newer=$(git rev-parse HEAD)
mkdir rust && echo a > rust/x.rs && git add rust && git commit -q -m code
code=$(git rev-parse HEAD)
echo b > docs/b.md && git add docs && git commit -q -m docs2
docsafter=$(git rev-parse HEAD)
git mv rust/x.rs docs/x.rs && git commit -q -m move
moved=$(git rev-parse HEAD)

# A stateful fake gh. State in $FS: ids (id<TAB>name), data/<id>, ref (the continuous tag), mut (writes so far).
# Faults: FAKE_FAIL_AT=k with FAKE_FAIL_MODE once (only write k fails), outage (write k and every later one
# fails), lateerr (write k takes effect but reports an error), kill (the calling script is SIGKILLed instead
# of doing write k) or killafter (after doing it).
# FAKE_API_ERROR=releases|assets|ref makes that read fail with a 503. FAKE_NO_DIGEST=1 leaves the digest out.
bin=$work/bin
mkdir "$bin"
cat > "$bin/gh" <<'FAKE'
#!/usr/bin/env bash
echo "$*" >> "$LOG"
FS=$FAKE_FS
e503() { echo "gh: Service Unavailable (HTTP 503)" >&2; exit 1; }
n404() { echo "gh: Not Found (HTTP 404)" >&2; exit 1; }
KILLAFTER=; LATE=
mutate() { # count a write; maybe fail or kill
  local n
  n=$(( $(cat "$FS/mut" 2>/dev/null || echo 0) + 1 )); echo "$n" > "$FS/mut"
  [[ -n "${FAKE_FAIL_AT:-}" ]] || return 0
  case "${FAKE_FAIL_MODE:-once}" in
    once) [[ $n -ne $FAKE_FAIL_AT ]] || { echo "gh: boom (HTTP 500)" >&2; exit 1; } ;;
    outage) [[ $n -lt $FAKE_FAIL_AT ]] || { echo "gh: boom (HTTP 500)" >&2; exit 1; } ;;
    lateerr) [[ $n -ne $FAKE_FAIL_AT ]] || LATE=1 ;;
    kill) [[ $n -ne $FAKE_FAIL_AT ]] || { kill -9 "$PPID"; exit 1; } ;;
    killafter) [[ $n -ne $FAKE_FAIL_AT ]] || KILLAFTER=1 ;;
  esac
}
finish() {
  [[ -z "$LATE" ]] || { echo "gh: Bad Gateway (HTTP 502)" >&2; exit 1; }
  [[ -z "$KILLAFTER" ]] || kill -9 "$PPID"
  exit 0
}
id_of() { awk -F'\t' -v n="$1" '$2==n{print $1; exit}' "$FS/ids"; }
add_asset() { # name, source file
  local id=$(( $(cat "$FS/next") )); echo $((id + 1)) > "$FS/next"
  printf '%s\t%s\n' "$id" "$1" >> "$FS/ids"; cp "$2" "$FS/data/$id"
}
case "$1 $2" in
  "release upload")
    [[ -z "${FAKE_UPLOAD_FAIL:-}" ]] || { echo "gh: upload failed (HTTP 500)" >&2; exit 1; }
    mutate
    f=$4; n=${f##*/}
    [[ -z "$(id_of "$n")" ]] || { echo "gh: asset under the same name already exists (HTTP 422)" >&2; exit 1; }
    add_asset "$n" "$f"; finish ;;
  "release create") mutate; finish ;;
  "release edit")
    mutate
    prev=; for a in "$@"; do [[ $prev != --notes ]] || printf "%s" "$a" > "$FS/notes"; prev=$a; done
    finish ;;
  "api "*)
    args=("$@"); method=GET; name=; sha=; path=
    for ((i = 0; i < ${#args[@]}; i++)); do
      case "${args[i]}" in
        -X) method=${args[i+1]} ;;
        name=*) name=${args[i]#name=} ;;
        sha=*) sha=${args[i]#sha=} ;;
        repos/*) path=${args[i]#repos/*/*/} ;;
      esac
    done
    case "$method $path" in
      "GET releases")
        [[ "${FAKE_API_ERROR:-}" != releases ]] || e503
        for t in ${FAKE_RELEASES:-}; do printf '%s\t1\tfalse\t%s\n' "$t" "${FAKE_ASSET_COUNT:-8}"; done
        for t in ${FAKE_DRAFTS:-}; do printf '%s\t2\ttrue\t0\n' "$t"; done
        [[ -z "${FAKE_CONT_RELEASE:-}" ]] || printf 'continuous\t42\tfalse\t%s\n' "$(wc -l < "$FS/ids")"
        exit 0 ;;
      "GET releases/42") cat "$FS/notes" 2>/dev/null; exit 0 ;;
      "GET releases/42/assets")
        [[ "${FAKE_API_ERROR:-}" != assets ]] || e503
        while IFS=$'\t' read -r id n; do
          d=; [[ -n "${FAKE_NO_DIGEST:-}" ]] || d=sha256:$(sha256sum < "$FS/data/$id" | cut -d' ' -f1)
          printf '%s\t%s\t%s\n' "$id" "$n" "$d"
        done < "$FS/ids"
        exit 0 ;;
      "GET releases/assets/"*) cat "$FS/data/${path#releases/assets/}"; exit 0 ;;
      "PATCH releases/assets/"*)
        mutate
        id=${path#releases/assets/}
        [[ -z "$(id_of "$name")" ]] || { echo "gh: name already exists (HTTP 422)" >&2; exit 1; }
        awk -F'\t' -v id="$id" -v n="$name" 'BEGIN{OFS="\t"} $1==id{$2=n} {print}' "$FS/ids" > "$FS/ids.t" && mv "$FS/ids.t" "$FS/ids"
        finish ;;
      "DELETE releases/assets/"*)
        mutate
        id=${path#releases/assets/}
        awk -F'\t' -v id="$id" '$1!=id' "$FS/ids" > "$FS/ids.t" && mv "$FS/ids.t" "$FS/ids"; rm -f "$FS/data/$id"
        finish ;;
      "GET git/ref/tags/continuous")
        [[ "${FAKE_API_ERROR:-}" != ref ]] || e503
        [[ -f "$FS/ref" ]] || n404
        cat "$FS/ref"; exit 0 ;;
      "PATCH git/refs/tags/continuous"|"POST git/refs") mutate; echo "$sha" > "$FS/ref"; finish ;;
    esac ;;
esac
exit 0
FAKE
chmod +x "$bin/gh"
FS=$work/fs
export PATH="$bin:$PATH" LOG=$work/log FAKE_FS=$FS

dist=$work/dist
mkdir -p "$dist"
ver=0.1.0-dev.7.g${c2:0:7}
for s in macos-universal.dmg windows-x64-setup.exe linux-x86_64.AppImage linux-amd64.deb linux-x86_64.flatpak sbom-rust.cdx.json sbom-npm.cdx.json; do echo "$s" > "$dist/dash-wallet-$ver-$s"; done
(cd "$dist" && sha256sum dash-wallet-* > SHA256SUMS)
export DIST=$dist OUT=$work/publish GITHUB_REPOSITORY=o/r GITHUB_RUN_NUMBER=7 GITHUB_RUN_ID=99 GITHUB_SERVER_URL=https://github.com GH_TOKEN=x
CN=(DashWallet-continuous-macos-universal.dmg DashWallet-continuous-windows-x64-setup.exe DashWallet-continuous-linux-x86_64.AppImage DashWallet-continuous-linux-amd64.deb
  DashWallet-continuous-linux-x86_64.flatpak DashWallet-continuous-sbom-npm.cdx.json DashWallet-continuous-sbom-rust.cdx.json SHA256SUMS)

fail=0
check() { if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi; }
# run <event> <ref> <sha> <version>: prepare then publish; sets $status and $LOG
run() {
  local event=$1 ref=$2 sha=$3
  export VERSION=$4 GITHUB_EVENT_NAME=$event GITHUB_REF=$ref GITHUB_SHA=$sha
  : > "$LOG"
  status=0
  "$here/release-publish.sh" prepare > "$work/prep.out" 2>&1 || status=$?
  [[ $status -ne 0 ]] || publish_only
}
publish_only() { # a wrapper shell takes the "Killed" notice of a SIGKILLed script
  status=0; bash -c '"$@"; exit $?' _ "$here/release-publish.sh" publish > "$work/pub.out" 2>&1 || status=$?
}
refused() { check "$1" '[[ $status -ne 0 && ! -s "$LOG" ]]'; }
log_has() { grep -qF -- "$1" "$LOG"; }
line_of() { grep -nF -- "$1" "$LOG" | head -1 | cut -d: -f1; }
writes() { cat "$FS/mut" 2>/dev/null || echo 0; }

# The release's assets: the old generation holds "old:<name>" binaries and the SHA256SUMS that matches.
fs_reset() { rm -rf "$FS"; mkdir -p "$FS/data"; echo 100 > "$FS/next"; : > "$FS/ids"; echo 0 > "$FS/mut"; }
seed_old() {
  fs_reset
  local n t=$work/seed
  rm -rf "$t"; mkdir -p "$t"
  local names=("${CN[@]}")
  [[ -z "${SEED_NO_SBOM:-}" ]] || names=("${CN[@]/*sbom*/}") # a generation from before the SBOMs
  for n in "${names[@]}"; do [[ -z $n || $n == SHA256SUMS ]] || echo "old:$n" > "$t/$n"; done
  (cd "$t" && sha256sum DashWallet-continuous-* > SHA256SUMS)
  for n in "${names[@]}"; do [[ -n $n ]] || continue; FS=$FS bash -c 'id=$(( $(cat "$FS/next") )); echo $((id+1)) > "$FS/next"; printf "%s\t%s\n" "$id" "$2" >> "$FS/ids"; cp "$1" "$FS/data/$id"' _ "$t/$n" "$n"; done
  echo "$c1" > "$FS/ref"; printf old-notes > "$FS/notes"
  export FAKE_CONT_RELEASE=1
}
body() { local id; id=$(awk -F'\t' -v n="$1" '$2==n{print $1; exit}' "$FS/ids"); [[ -z "$id" ]] || cat "$FS/data/$id"; }
leftovers() { awk -F'\t' '$2 ~ /\.(old|new)$/ || $2 == "txn.json"' "$FS/ids" | wc -l; }
# which generation the tag and the notes describe: old | new (the notes name the commit)
meta_gen() { if grep -q "commit $c2" "$FS/notes"; then echo new; else echo old; fi; }
# the tag and notes of generation $1 (old: tag c1, old notes; new: tag c2, new notes)
meta_ok() { [[ "$(meta_gen)" == "$1" && "$(cat "$FS/ref")" == "$([[ $1 == new ]] && echo "$c2" || echo "$c1")" ]]; }
# old | new | missing:<name> | mixed | badsums
# GEN_OLD_NO_SBOM=1: the old generation is from before the SBOMs, so a missing SBOM counts as old.
gen_of() {
  local n g="" one want have
  [[ -n "$(body SHA256SUMS)" ]] || { echo "missing:SHA256SUMS"; return; }
  for n in "${CN[@]}"; do
    [[ $n == SHA256SUMS ]] && continue
    if [[ -z "$(body "$n")" ]]; then
      [[ -n "${GEN_OLD_NO_SBOM:-}" && $n == *sbom* ]] || { echo "missing:$n"; return; }
      one=old
    else
      case "$(body "$n")" in old:*) one=old ;; *) one=new ;; esac
      want=$(body SHA256SUMS | awk -v n="$n" '$2==n{print $1}')
      have=$(body "$n" | sha256sum | cut -d' ' -f1)
      [[ "$want" == "$have" ]] || { echo badsums; return; }
    fi
    [[ -z "$g" || "$g" == "$one" ]] || { echo mixed; return; }
    g=$one
  done
  echo "$g"
}

# --- refusals: no write, no gh call at all
fs_reset
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

# --- push to main, first publish (no continuous release, no tag)
fs_reset; unset FAKE_CONT_RELEASE
run push refs/heads/main "$c2" "$ver"
check "main: succeeds" '[[ $status -eq 0 ]]'
check "main: attest dir holds 5 bundles, 2 SBOMs, SHA256SUMS, 7 continuous files, SHA256SUMS.continuous" '[[ $(ls "$OUT/attest" | wc -l) -eq 16 ]]'
check "main: continuous SHA256SUMS lists stable names only" '[[ $(grep -c "DashWallet-continuous-" "$OUT/continuous/SHA256SUMS") -eq 7 && $(wc -l < "$OUT/continuous/SHA256SUMS") -eq 7 ]]'
tag="v0.1.0-dev.7+${c2:0:7}"
check "main: immutable prerelease $tag, at the commit, a prerelease, not latest" \
  'log_has "release create $tag " && log_has "--target $c2" && log_has "--prerelease" && log_has "--latest=false"'
check "main: no draft, no delete" '! log_has "--draft" && ! log_has "DELETE"'
check "main: creates continuous" 'log_has "release create continuous "'

# --- push of a release tag: a version is a full release, a release candidate a prerelease
tag_bundles() { # the seven files under version $1, alone in their own dist
  rm -rf "$work/tagdist"; mkdir -p "$work/tagdist"
  for f in "$dist"/dash-wallet-"$ver"-*; do cp "$f" "$work/tagdist/$(basename "${f/$ver/$1}")"; done
  (cd "$work/tagdist" && sha256sum dash-wallet-* > SHA256SUMS); }
tag_bundles 0.1.0; fs_reset
DIST=$work/tagdist run push refs/tags/v0.1.0 "$c2" 0.1.0
check "tag v0.1.0: a full release, not a prerelease, Latest left to GitHub" \
  '[[ $status -eq 0 ]] && log_has "release create v0.1.0 " && ! log_has "--prerelease" && ! log_has "--latest"'
tag_bundles 0.1.0-rc.1; fs_reset
DIST=$work/tagdist run push refs/tags/v0.1.0-rc.1 "$c2" 0.1.0-rc.1
check "tag v0.1.0-rc.1: a prerelease, not Latest" \
  '[[ $status -eq 0 ]] && log_has "release create v0.1.0-rc.1 " && log_has "--prerelease" && log_has "--latest=false"'

# --- push to main again: the per-run release exists, continuous exists at the older commit
seed_old
FAKE_RELEASES="$tag" run push refs/heads/main "$c2" "$ver"
check "main rerun: succeeds" '[[ $status -eq 0 ]]'
check "main rerun: the immutable release is not created again" '! log_has "release create"'
check "main rerun: assets uploaded as .new, then renamed" 'log_has "release upload continuous" && log_has "name=DashWallet-continuous-macos-universal.dmg.old" && log_has "name=DashWallet-continuous-macos-universal.dmg" '
check "main rerun: SHA256SUMS is swapped last" '[[ $(line_of "name=SHA256SUMS.old") -gt $(line_of "name=DashWallet-continuous-linux-amd64.deb.old") ]]'
check "main rerun: the tag moves after the assets, the .old copies go after the tag" \
  '[[ $(line_of "git/refs/tags/continuous") -gt $(line_of "name=SHA256SUMS.old") && $(line_of "DELETE") -gt $(line_of "git/refs/tags/continuous") ]] && log_has "-f sha=$c2"'
check "main rerun: ends as the complete new generation, nothing left over" '[[ $(gen_of) == new && $(leftovers) -eq 0 && $(cat "$FS/ref") == "$c2" ]]'
SEED_NO_SBOM=1 seed_old
FAKE_RELEASES="$tag" run push refs/heads/main "$c2" "$ver"
check "main after a generation without SBOMs: adds them, ends as the complete new generation" \
  '[[ $status -eq 0 && $(gen_of) == new && $(leftovers) -eq 0 && -n "$(body DashWallet-continuous-sbom-rust.cdx.json)" ]]'
# a failed or cancelled update of that generation rolls back to it, without the SBOMs it never had
SEED_NO_SBOM=1 seed_old
FAKE_RELEASES="$tag" publish_only
total_nosbom=$(writes)
bad=0
for mode in once lateerr; do
  for ((k = 1; k <= total_nosbom; k++)); do
    SEED_NO_SBOM=1 seed_old
    FAKE_RELEASES="$tag" FAKE_FAIL_AT=$k FAKE_FAIL_MODE=$mode publish_only
    g=$(GEN_OLD_NO_SBOM=1 gen_of)
    if [[ $status -eq 0 ]]; then [[ $g == new ]] && meta_ok new || { echo "  no-sbom $mode@$k: exit 0 but $g"; bad=1; }
    else [[ $g == old && $(leftovers) -eq 0 ]] && meta_ok old || { echo "  no-sbom $mode@$k: exit $status, state $g, leftovers $(leftovers)"; bad=1; }; fi
  done
done
check "faults (once, lateerr) at each of the $total_nosbom writes of the first update with SBOMs end in one complete generation" '[[ $bad -eq 0 ]]'

# --- an older run finishing after a newer one must not move continuous back
seed_old; echo "$code" > "$FS/ref"
FAKE_RELEASES="" run push refs/heads/main "$c2" "$ver"
check "older run: succeeds, creates its own release, leaves continuous alone" \
  '[[ $status -eq 0 ]] && log_has "release create $tag " && ! log_has "release upload" && [[ $(gen_of) == old ]]'

# --- fault matrix over every write of a full update
seed_old
FAKE_RELEASES="$tag" run push refs/heads/main "$c2" "$ver"
total=$(writes)
check "a full update is $total writes" '[[ $total -ge 20 ]]'
for mode in once lateerr outage kill killafter; do
  bad=0
  for ((k = 1; k <= total; k++)); do
    seed_old
    export FAKE_RELEASES="$tag" FAKE_FAIL_AT=$k FAKE_FAIL_MODE=$mode
    publish_only; s1=$status
    unset FAKE_FAIL_AT FAKE_FAIL_MODE
    g1=$(gen_of); l1=$(leftovers)
    if [[ $mode == once || $mode == lateerr ]]; then
      # the run itself leaves one whole generation with its tag and notes: the new one, or the old one restored
      if [[ $s1 -eq 0 ]]; then [[ $g1 == new ]] && meta_ok new || { echo "  $mode@$k: exit 0 but $g1, tag $(cat "$FS/ref")"; bad=1; }
      else [[ $g1 == old && $l1 -eq 0 ]] && meta_ok old || { echo "  $mode@$k: exit $s1, state $g1, leftovers $l1, tag $(cat "$FS/ref"), notes $(meta_gen)"; bad=1; }; fi
    else
      # an outage or a kill may leave a gap; the next run repairs it.
      # First the next run's recovery on its own (its upload fails, so nothing of the new generation survives
      # it): whatever it keeps must be one whole generation, never a mix or a SHA256SUMS that disagrees, and
      # the tag and notes must describe that generation.
      FAKE_UPLOAD_FAIL=1 publish_only
      g2=$(gen_of)
      [[ $status -ne 0 && ( $g2 == old || $g2 == new ) && $(leftovers) -eq 0 ]] && meta_ok "$g2" ||
        { echo "  $mode@$k: recovery alone: exit $status, state $g2, leftovers $(leftovers), tag $(cat "$FS/ref"), notes $(meta_gen)"; bad=1; }
      publish_only
      [[ $status -eq 0 && $(gen_of) == new && $(leftovers) -eq 0 ]] && meta_ok new ||
        { echo "  $mode@$k: next run exit $status, state $(gen_of), leftovers $(leftovers)"; bad=1; }
    fi
  done
  check "faults ($mode) at each of the $total writes end in one complete generation, tag and notes matching" '[[ $bad -eq 0 ]]'
done
# an OLDER run after an interrupted update of the newer commit still repairs it (then leaves continuous alone)
bad=0
for ((k = 1; k <= total; k++)); do
  seed_old; echo "$c2" > "$FS/ref"
  FAKE_RELEASES="$tag" FAKE_FAIL_AT=$k FAKE_FAIL_MODE=killafter publish_only
  GITHUB_SHA=$c1 publish_only
  g=$(gen_of)
  [[ $status -eq 0 && ( $g == old || $g == new ) && $(leftovers) -eq 0 && "$(cat "$FS/ref")" == "$c2" && "$(meta_gen)" == "$g" ]] ||
    { echo "  older run after killafter@$k: exit $status, state $g, leftovers $(leftovers), tag $(cat "$FS/ref"), notes $(meta_gen)"; bad=1; }
done
check "an older run repairs an interrupted update of the newer commit (killafter at each write)" '[[ $bad -eq 0 ]]'
export GITHUB_SHA=$c2
unset FAKE_RELEASES
# recovery without the API's digests: hashes the downloaded bodies
seed_old
FAKE_RELEASES="$tag" FAKE_FAIL_AT=12 FAKE_FAIL_MODE=kill publish_only
FAKE_RELEASES="$tag" FAKE_NO_DIGEST=1 publish_only
check "recovery without digests: complete new generation" '[[ $status -eq 0 && $(gen_of) == new && $(leftovers) -eq 0 ]]'
# an outage during the promotions fails the run and leaves one whole generation after the rollback attempt
seed_old
FAKE_RELEASES="$tag" FAKE_FAIL_AT=10 FAKE_FAIL_MODE=outage publish_only
check "outage mid-promotion leaves a failure exit" '[[ $status -ne 0 ]]'

# --- a failing read is not "absent": no write happens
for what in releases assets ref; do
  seed_old
  FAKE_RELEASES="" FAKE_API_ERROR=$what publish_only
  check "a fresh run (no immutable release yet): a failing $what read stops it before any write" '[[ $status -ne 0 && $(writes) -eq 0 ]]'
  seed_old
  FAKE_RELEASES="$tag" FAKE_API_ERROR=$what publish_only
  check "a rerun: a failing $what read stops it before any write" '[[ $status -ne 0 && $(writes) -eq 0 ]]'
done
seed_old
FAKE_RELEASES="$tag" FAKE_DRAFTS="" FAKE_ASSET_COUNT=3 publish_only
check "release exists with 3 of 8 assets: fails, writes nothing" '[[ $status -ne 0 && $(writes) -eq 0 ]]'
seed_old
FAKE_RELEASES="" FAKE_DRAFTS="$tag" publish_only
check "a draft with the tag: fails, writes nothing" '[[ $status -ne 0 && $(writes) -eq 0 ]]'
# leftovers + a failing asset listing: still nothing written
seed_old; echo x > "$work/x"; FS=$FS bash -c 'echo 200 > "$FS/next"; printf "200\tDashWallet-continuous-linux-amd64.deb.new\n" >> "$FS/ids"; echo junk > "$FS/data/200"'
FAKE_RELEASES="$tag" FAKE_API_ERROR=assets publish_only
check "leftovers and an unreadable asset list: fails before any write" '[[ $status -ne 0 && $(writes) -eq 0 ]]'

# --- the update feed (PK-03): release-feed.sh writes it, and only a stable release tag publishes it
feed_of() { # stage the updater files of version $1 (any of the four names after it), run `feed`; sets $fs
  local v=$1; shift
  rm -rf "$work/fin" "$work/fout"; mkdir -p "$work/fin"
  for f in "$@"; do printf 'c2lnbmF0dXJlIG9mICVz\n' > "$work/fin/dash-wallet-$v-$f"; done
  fs=0; "$here/release-feed.sh" feed "$work/fin" "$work/fout" "$v" o/r > /dev/null 2>&1 || fs=$?
}
ALL_UPDATER=(linux-x86_64.AppImage.sig windows-x64-setup.exe.sig macos-universal.app.tar.gz macos-universal.app.tar.gz.sig)
feed_of 0.1.0 "${ALL_UPDATER[@]}"
check "feed: latest.json with the AppImage, NSIS and macOS app entries, on the tag's download URLs" \
  '[[ $fs -eq 0 ]] && jq -e ".version == \"0.1.0\" and (.platforms | keys) == [\"darwin-aarch64-app\",\"darwin-x86_64-app\",\"linux-x86_64-appimage\",\"windows-x86_64-nsis\"] and .platforms[\"linux-x86_64-appimage\"].url == \"https://github.com/o/r/releases/download/v0.1.0/dash-wallet-0.1.0-linux-x86_64.AppImage\" and .platforms[\"windows-x86_64-nsis\"].signature == \"c2lnbmF0dXJlIG9mICVz\"" "$work/fout/latest.json" > /dev/null && [[ $(ls "$work/fout" | wc -l) -eq 2 ]]'
tag_bundles 0.1.0; fs_reset
FEED=$work/fout DIST=$work/tagdist DW_UPDATER_REQUIRED=1 run push refs/tags/v0.1.0 "$c2" 0.1.0
check "tag v0.1.0 with the feed: latest.json and the macOS archive are attested and published with the release" \
  '[[ $status -eq 0 && -f "$OUT/attest/latest.json" && -f "$OUT/attest/dash-wallet-0.1.0-macos-universal.app.tar.gz" ]] && log_has "release create v0.1.0 " && log_has "/latest.json" && ! log_has "--prerelease"'
FEED=$work/fout DIST=$work/tagdist FAKE_RELEASES=v0.1.0 FAKE_ASSET_COUNT=10 run push refs/tags/v0.1.0 "$c2" 0.1.0
check "tag v0.1.0 with the feed, rerun: the complete release (10 assets) is left as it is" '[[ $status -eq 0 ]] && ! log_has "release create"'
fs_reset
FEED=$work/empty-feed DIST=$work/tagdist run push refs/tags/v0.1.0 "$c2" 0.1.0
check "tag v0.1.0 without a feed (no updater key): the release as before" '[[ $status -eq 0 && ! -f "$OUT/attest/latest.json" ]] && ! log_has "latest.json"'
fs_reset
FEED=$work/empty-feed DIST=$work/tagdist DW_UPDATER_REQUIRED=1 run push refs/tags/v0.1.0 "$c2" 0.1.0; refused "DW_UPDATER_REQUIRED=1: a stable tag without a feed is refused"
tag_bundles 0.1.0-rc.1; fs_reset
FEED=$work/empty-feed DIST=$work/tagdist DW_UPDATER_REQUIRED=1 run push refs/tags/v0.1.0-rc.1 "$c2" 0.1.0-rc.1
check "DW_UPDATER_REQUIRED=1: a release candidate still needs none" '[[ $status -eq 0 ]] && log_has "release create v0.1.0-rc.1 "'
feed_of 0.1.0-rc.1 "${ALL_UPDATER[@]}"
check "feed: a release candidate gets none" '[[ $fs -eq 0 && ! -e "$work/fout" ]]'
feed_of 0.1.0
check "feed: a build without the key gets none" '[[ $fs -eq 0 && ! -e "$work/fout" ]]'
feed_of 0.1.0 linux-x86_64.AppImage.sig windows-x64-setup.exe.sig
check "feed: an OS without its updater files fails" '[[ $fs -ne 0 ]]'
# a feed that reaches the publish job anyway is refused anywhere but a stable tag
feed_of 0.1.0-rc.1 "${ALL_UPDATER[@]}"; mkdir -p "$work/fout"; cp "$work/fin/dash-wallet-0.1.0-rc.1-macos-universal.app.tar.gz" "$work/fout/"
jq -n '{version: "0.1.0-rc.1", platforms: {a: {url: "https://github.com/o/r/releases/download/v0.1.0-rc.1/x"}, b: {url: "https://github.com/o/r/releases/download/v0.1.0-rc.1/x"}, c: {url: "https://github.com/o/r/releases/download/v0.1.0-rc.1/x"}, d: {url: "https://github.com/o/r/releases/download/v0.1.0-rc.1/x"}}}' > "$work/fout/latest.json"
tag_bundles 0.1.0-rc.1; fs_reset
FEED=$work/fout DIST=$work/tagdist run push refs/tags/v0.1.0-rc.1 "$c2" 0.1.0-rc.1; refused "a release candidate never publishes a feed"
fs_reset; unset FAKE_CONT_RELEASE
FEED=$work/fout run push refs/heads/main "$c2" "$ver";                              refused "main never publishes a feed"
feed_of 0.0.9 "${ALL_UPDATER[@]}"
mv "$work/fout/dash-wallet-0.0.9-macos-universal.app.tar.gz" "$work/fout/dash-wallet-0.1.0-macos-universal.app.tar.gz"
tag_bundles 0.1.0; fs_reset
FEED=$work/fout DIST=$work/tagdist run push refs/tags/v0.1.0 "$c2" 0.1.0;         refused "another version's feed is refused"

# --- PGP SHA256SUMS.asc: optional on tags, attested and published when present, required on final tags only when DW_SIGNING_REQUIRED=1
tag_bundles 0.1.0; printf -- '-----BEGIN PGP SIGNATURE-----\ntest\n-----END PGP SIGNATURE-----\n' > "$work/tagdist/SHA256SUMS.asc"; fs_reset
DIST=$work/tagdist run push refs/tags/v0.1.0 "$c2" 0.1.0
check "tag v0.1.0 with SHA256SUMS.asc: attested and published (9 assets)" \
  '[[ $status -eq 0 && -f "$OUT/attest/SHA256SUMS.asc" ]] && log_has "release create v0.1.0 " && log_has "/SHA256SUMS.asc"'
DIST=$work/tagdist FAKE_RELEASES=v0.1.0 FAKE_ASSET_COUNT=9 run push refs/tags/v0.1.0 "$c2" 0.1.0
check "tag v0.1.0 with SHA256SUMS.asc, rerun: complete release (9 assets) is left as it is" \
  '[[ $status -eq 0 ]] && ! log_has "release create"'
feed_of 0.1.0 "${ALL_UPDATER[@]}"
fs_reset
FEED=$work/fout DIST=$work/tagdist DW_SIGNING_REQUIRED=1 DW_UPDATER_REQUIRED=1 run push refs/tags/v0.1.0 "$c2" 0.1.0
check "tag v0.1.0 with SHA256SUMS.asc and feed: all 11 assets attested and published" \
  '[[ $status -eq 0 && -f "$OUT/attest/SHA256SUMS.asc" && -f "$OUT/attest/latest.json" ]] && log_has "/SHA256SUMS.asc" && log_has "/latest.json"'
FEED=$work/fout DIST=$work/tagdist FAKE_RELEASES=v0.1.0 FAKE_ASSET_COUNT=11 run push refs/tags/v0.1.0 "$c2" 0.1.0
check "tag v0.1.0 with SHA256SUMS.asc and feed, rerun: complete release (11 assets) is left as it is" \
  '[[ $status -eq 0 ]] && ! log_has "release create"'
tag_bundles 0.1.0; fs_reset
DIST=$work/tagdist run push refs/tags/v0.1.0 "$c2" 0.1.0
check "tag v0.1.0 without SHA256SUMS.asc: succeeds without DW_SIGNING_REQUIRED" \
  '[[ $status -eq 0 && ! -f "$OUT/attest/SHA256SUMS.asc" ]] && ! log_has "SHA256SUMS.asc"'
fs_reset
DIST=$work/tagdist DW_SIGNING_REQUIRED=1 run push refs/tags/v0.1.0 "$c2" 0.1.0
refused "DW_SIGNING_REQUIRED=1: final tag without SHA256SUMS.asc is refused"
check "DW_SIGNING_REQUIRED=1: publish refusal names missing PGP secrets" \
  'grep -qF "RELEASE_PGP_PRIVATE_KEY" "$work/prep.out"'
tag_bundles 0.1.0-rc.1; fs_reset
DIST=$work/tagdist DW_SIGNING_REQUIRED=1 run push refs/tags/v0.1.0-rc.1 "$c2" 0.1.0-rc.1
check "DW_SIGNING_REQUIRED=1: release candidate without SHA256SUMS.asc still succeeds" \
  '[[ $status -eq 0 ]] && log_has "release create v0.1.0-rc.1 " && ! log_has "SHA256SUMS.asc"'
printf -- '-----BEGIN PGP SIGNATURE-----\nrc\n-----END PGP SIGNATURE-----\n' > "$work/tagdist/SHA256SUMS.asc"; fs_reset
DIST=$work/tagdist run push refs/tags/v0.1.0-rc.1 "$c2" 0.1.0-rc.1
check "signed candidate v0.1.0-rc.1 with SHA256SUMS.asc: attested and published as prerelease" \
  '[[ $status -eq 0 && -f "$OUT/attest/SHA256SUMS.asc" ]] && log_has "release create v0.1.0-rc.1 " && log_has "/SHA256SUMS.asc" && log_has "--prerelease"'
rm -rf "$work/maindist"; cp -R "$dist" "$work/maindist"
printf -- '-----BEGIN PGP SIGNATURE-----\nmain\n-----END PGP SIGNATURE-----\n' > "$work/maindist/SHA256SUMS.asc"; fs_reset; unset FAKE_CONT_RELEASE
DIST=$work/maindist run push refs/heads/main "$c2" "$ver"
refused "main never publishes SHA256SUMS.asc"

# --- tauri-build.sh: the key decides the updater; a stable tag without it warns, or fails when required
tb() { # tb EVENT REF KEY REQUIRED: runs tauri-build.sh with a fake pnpm; sets $tbs, $TB (its output)
  local env_file=$work/tb-env
  : > "$env_file"
  tbs=0
  TB=$(cd "$work" && PATH="$work/tb-bin:$PATH" GITHUB_EVENT_NAME=$1 GITHUB_REF=$2 TAURI_SIGNING_PRIVATE_KEY=$3 \
    DW_UPDATER_REQUIRED=$4 GITHUB_ENV=$env_file "$here/tauri-build.sh" --bundles appimage 2>&1) || tbs=$?
}
mkdir -p "$work/tb-bin"
printf '#!/usr/bin/env bash\necho "pnpm $*"\n' > "$work/tb-bin/pnpm"; chmod +x "$work/tb-bin/pnpm"
tb push refs/tags/v0.1.0 key ""
check "tauri-build: with the key, the feed overlay and UPDATER=1" \
  '[[ $tbs -eq 0 && "$TB" == *"pnpm tauri build --bundles appimage --config ../../rust/crates/dw-app/tauri.updater.conf.json"* ]] && grep -qx UPDATER=1 "$work/tb-env"'
tb push refs/heads/main "" ""
check "tauri-build: main without the key is the plain build, no warning" \
  '[[ $tbs -eq 0 && "$TB" == *"pnpm tauri build --bundles appimage"* && "$TB" != *config* && "$TB" != *warning* && ! -s "$work/tb-env" ]]'
tb push refs/tags/v0.1.0 "" ""
check "tauri-build: a stable tag without the key warns and builds" '[[ $tbs -eq 0 && "$TB" == *"::warning::"* && "$TB" == *"pnpm tauri build"* ]]'
tb push refs/tags/v0.1.0 "" 1
check "tauri-build: a stable tag without the key fails once DW_UPDATER_REQUIRED=1" '[[ $tbs -ne 0 && "$TB" == *"::error::"* && "$TB" != *"pnpm tauri build"* ]]'
tb push refs/tags/v0.1.0-rc.1 "" 1
check "tauri-build: a release candidate or a dry run without the key builds quietly" '[[ $tbs -eq 0 && "$TB" != *"::"* ]]'
tb workflow_dispatch refs/tags/v0.1.0 "" 1
check "tauri-build: a dispatch on a tag (never given the key) builds quietly" '[[ $tbs -eq 0 && "$TB" != *"::"* ]]'
# Empty APPLE_* variables must be unset so tauri.conf.json's ad-hoc "-" identity is not overwritten by "".
cat > "$work/tb-bin/pnpm-env" <<'PNPM_ENV'
#!/usr/bin/env bash
printf 'APPLE_SIGNING_IDENTITY=%s APPLE_CERTIFICATE=%s\n' "${APPLE_SIGNING_IDENTITY-<unset>}" "${APPLE_CERTIFICATE-<unset>}"
PNPM_ENV
chmod +x "$work/tb-bin/pnpm-env"
mv "$work/tb-bin/pnpm" "$work/tb-bin/pnpm.orig"
mv "$work/tb-bin/pnpm-env" "$work/tb-bin/pnpm"
TB_EMPTY=$(cd "$work" && PATH="$work/tb-bin:$PATH" GITHUB_EVENT_NAME=push GITHUB_REF=refs/heads/main \
  APPLE_SIGNING_IDENTITY="" APPLE_CERTIFICATE="" "$here/tauri-build.sh" --bundles dmg 2>&1)
check "tauri-build: empty APPLE_* env vars are unset before pnpm tauri build" \
  '[[ "$TB_EMPTY" == *"APPLE_SIGNING_IDENTITY=<unset> APPLE_CERTIFICATE=<unset>"* ]]'
TB_SET=$(cd "$work" && PATH="$work/tb-bin:$PATH" GITHUB_EVENT_NAME=push GITHUB_REF=refs/tags/v0.1.0 \
  APPLE_SIGNING_IDENTITY="Developer ID Application: Test" APPLE_CERTIFICATE="cert" "$here/tauri-build.sh" --bundles dmg 2>&1)
check "tauri-build: non-empty APPLE_* env vars are preserved" \
  '[[ "$TB_SET" == *"APPLE_SIGNING_IDENTITY=Developer ID Application: Test APPLE_CERTIFICATE=cert"* ]]'
mv "$work/tb-bin/pnpm.orig" "$work/tb-bin/pnpm"
export GITHUB_SHA=$c2

# --- classify and decide
check "classify: main push" '[[ $(GITHUB_EVENT_NAME=push GITHUB_REF=refs/heads/main "$here/release-publish.sh" classify) == main ]]'
check "classify: release tag" '[[ $(GITHUB_EVENT_NAME=push GITHUB_REF=refs/tags/v0.1.0-rc.2 "$here/release-publish.sh" classify) == tag ]]'
check "classify: dry-run tag, branch, PR, dispatch" '[[ $({ for r in refs/tags/v0.0.0-rel8 refs/heads/dw/x; do GITHUB_EVENT_NAME=push GITHUB_REF=$r "$here/release-publish.sh" classify; done; for e in pull_request workflow_dispatch; do GITHUB_EVENT_NAME=$e GITHUB_REF=refs/heads/main "$here/release-publish.sh" classify; done; } | tr "\n" " ") == "skip skip skip skip " ]]'
# decide <event> <ref> <sha> <base> [repo]: prints "publish build"
decide() {
  local out=$work/decide.out
  : > "$out"
  GITHUB_EVENT_NAME=$1 GITHUB_REF=$2 GITHUB_SHA=$3 DECIDE_BASE=${4:-} GITHUB_REPOSITORY=${5:-PastaPastaPasta/dashwallet-desktop} \
    GITHUB_OUTPUT=$out DECIDE_NO_FETCH=1 "$here/release-decide.sh" > "$work/decide.log" 2>&1 || { echo "decide-failed"; return; }
  echo "$(sed -n 's/^publish=//p' "$out") $(sed -n 's/^build=//p' "$out")"
}
check "decide: push to main publishes and builds" '[[ $(decide push refs/heads/main "$c2" "$c1") == "true true" ]]'
check "decide: a docs-only change to main still builds" '[[ $(decide push refs/heads/main "$newer" "$c2") == "false true" ]]'
check "decide: a release tag on main publishes" '[[ $(decide push refs/tags/v0.1.0 "$c2" 0000000000000000000000000000000000000000) == "true true" ]]'
check "decide: a release tag OFF main does not publish (so the write job never starts)" '[[ $(decide push refs/tags/v0.1.0 "$side" "") == "false true" ]]'
check "decide: an rc tag off main does not publish" '[[ $(decide push refs/tags/v0.1.0-rc.1 "$code" "") == "false true" ]]'
check "decide: a dry-run tag does not publish" '[[ $(decide push refs/tags/v0.0.0-rel8 "$c2" "") == "false true" ]]'
check "decide: main on a commit outside origin/main does not publish" '[[ $(decide push refs/heads/main "$side" "") == "false true" ]]'
check "decide: another repository does not publish" '[[ $(decide push refs/heads/main "$c2" "" someone/dashwallet-desktop) == "false true" ]]'
check "decide: a pull request and a dispatch do not publish" '[[ $(decide pull_request refs/pull/9/merge "$c2" "$c1") == "false true" && $(decide workflow_dispatch refs/heads/main "$c2" "") == "false true" ]]'
check "decide: the schedule builds main and does not publish" '[[ $(decide schedule refs/heads/main "$c2" "") == "false true" ]]'
check "decide: a docs-only branch push does not build" '[[ $(decide push refs/heads/dw/x "$newer" "$c2") == "false false" ]]'
signing() { decide "$@" > /dev/null; sed -n 's/^signing=//p' "$work/decide.out"; }
check "decide: only a stable tag on main gets the updater key" '[[ $(signing push refs/tags/v0.1.0 "$c2" "") == true ]]'
check "decide: an rc, a dry run, a tag off main, main and a dispatch get no key" \
  '[[ "$(signing push refs/tags/v0.1.0-rc.1 "$c2" "") $(signing push refs/tags/v0.0.0-rel8 "$c2" "") $(signing push refs/tags/v0.1.0 "$side" "") $(signing push refs/heads/main "$c2" "") $(signing workflow_dispatch refs/tags/v0.1.0 "$c2" "")" == "false false false false false" ]]'
check "decide: DW_SIGN_CANDIDATES=1 allows signing on a candidate on main only" \
  '[[ "$(DW_SIGN_CANDIDATES=1 signing push refs/tags/v0.1.0-rc.1 "$c2" "") $(DW_SIGN_CANDIDATES=1 signing push refs/tags/v0.1.0-rc.1 "$side" "") $(DW_SIGN_CANDIDATES=1 signing push refs/tags/v0.0.0-rel8 "$c2" "") $(DW_SIGN_CANDIDATES=1 signing push refs/heads/main "$c2" "") $(DW_SIGN_CANDIDATES=1 signing workflow_dispatch refs/tags/v0.1.0-rc.1 "$c2" "")" == "true false false false false" ]]'
check "decide: a docs-only pull request does not build" '[[ $(decide pull_request refs/pull/9/merge "$newer" "$c2") == "false false" ]]'
check "decide: a code change builds" '[[ $(decide push refs/heads/dw/x "$code" "$c2") == "false true" ]]'
check "decide: a docs-only push after a code push on the branch still builds (measured from main, not the last push)" '[[ $(decide push refs/heads/dw/x "$docsafter" "$code") == "false true" ]]'
check "decide: moving a source file into docs/ is a code change" '[[ $(decide pull_request refs/pull/9/merge "$moved" "$code") == "false true" ]]'
check "decide: a pull request with no known base builds" '[[ $(decide pull_request refs/pull/9/merge "$newer" "") == "false true" ]]'
check "decide: a docs-only change on a tag still builds" '[[ $(decide push refs/tags/v0.1.0 "$newer" "$c2") == "false true" ]]'

# --- release-signing-check.sh: DW_SIGNING_REQUIRED, missing-secret errors, warnings, and candidates
sc() { # sc <target> [VAR=VAL ...]: runs release-signing-check.sh in a clean env; sets $scs, $SC, $SC_ENV
  local target=$1; shift
  local env_file=$work/sc-env
  : > "$env_file"
  scs=0
  SC=$(env -i PATH="$PATH" GITHUB_ENV="$env_file" "$@" "$here/release-signing-check.sh" "$target" 2>&1) || scs=$?
  SC_ENV=$(cat "$env_file")
}
sc macos GITHUB_EVENT_NAME=push GITHUB_REF=refs/tags/v1.0.0 DW_SIGNING_ENABLED=true DW_SIGNING_REQUIRED=1
check "signing-check macos: DW_SIGNING_REQUIRED=1 refuses final tag missing secrets and names them" \
  '[[ $scs -ne 0 && "$SC" == *"::error::"* && "$SC" == *"APPLE_CERTIFICATE"* && "$SC" == *"APPLE_CERTIFICATE_PASSWORD"* && "$SC" == *"APPLE_SIGNING_IDENTITY"* && "$SC" == *"APPLE_API_ISSUER"* && "$SC" == *"APPLE_API_KEY"* && "$SC" == *"APPLE_API_KEY_P8"* && "$SC" == *"APPLE_TEAM_ID"* ]]'
sc macos GITHUB_EVENT_NAME=push GITHUB_REF=refs/tags/v1.0.0 DW_SIGNING_ENABLED=true DW_SIGNING_REQUIRED=1 \
  APPLE_CERTIFICATE=SECRET_VAL_1 APPLE_CERTIFICATE_PASSWORD=SECRET_VAL_2 APPLE_SIGNING_IDENTITY=SECRET_VAL_3 \
  APPLE_API_ISSUER=SECRET_VAL_4 APPLE_API_KEY=SECRET_VAL_5 APPLE_TEAM_ID=TEAM123
check "signing-check macos: partial set fails naming only missing secret and never leaks values" \
  '[[ $scs -ne 0 && "$SC" == *"APPLE_API_KEY_P8"* && "$SC" != *"APPLE_CERTIFICATE"* && "$SC" != *"SECRET_VAL_"* ]]'
sc windows GITHUB_EVENT_NAME=push GITHUB_REF=refs/tags/v1.0.0 DW_SIGNING_ENABLED=true DW_SIGNING_REQUIRED=1
check "signing-check windows: DW_SIGNING_REQUIRED=1 refuses final tag missing Azure set and names them" \
  '[[ $scs -ne 0 && "$SC" == *"::error::"* && "$SC" == *"AZURE_TENANT_ID"* && "$SC" == *"AZURE_CLIENT_ID"* && "$SC" == *"AZURE_CLIENT_SECRET"* && "$SC" == *"AZURE_SIGNING_ENDPOINT"* && "$SC" == *"AZURE_SIGNING_ACCOUNT"* && "$SC" == *"AZURE_SIGNING_PROFILE"* ]]'
sc pgp GITHUB_EVENT_NAME=push GITHUB_REF=refs/tags/v1.0.0 DW_SIGNING_ENABLED=true DW_SIGNING_REQUIRED=1
check "signing-check pgp: DW_SIGNING_REQUIRED=1 refuses final tag missing PGP set and names them" \
  '[[ $scs -ne 0 && "$SC" == *"::error::"* && "$SC" == *"RELEASE_PGP_PRIVATE_KEY"* && "$SC" == *"RELEASE_PGP_PASSPHRASE"* && "$SC" == *"RELEASE_PGP_FINGERPRINT"* ]]'
sc macos GITHUB_EVENT_NAME=push GITHUB_REF=refs/tags/v1.0.0 DW_SIGNING_ENABLED=true
check "signing-check macos: final tag without secrets or DW_SIGNING_REQUIRED warns once and sets MACOS_SIGN=0" \
  '[[ $scs -eq 0 && "$SC" == *"::warning::"* && "$SC_ENV" == "MACOS_SIGN=0" ]]'
sc windows GITHUB_EVENT_NAME=push GITHUB_REF=refs/tags/v1.0.0 DW_SIGNING_ENABLED=true
check "signing-check windows: final tag without secrets or DW_SIGNING_REQUIRED warns once and sets WINDOWS_SIGN=0" \
  '[[ $scs -eq 0 && "$SC" == *"::warning::"* && "$SC_ENV" == "WINDOWS_SIGN=0" ]]'
sc pgp GITHUB_EVENT_NAME=push GITHUB_REF=refs/tags/v1.0.0 DW_SIGNING_ENABLED=true
check "signing-check pgp: final tag without secrets or DW_SIGNING_REQUIRED warns once and sets PGP_SIGN=0" \
  '[[ $scs -eq 0 && "$SC" == *"::warning::"* && "$SC_ENV" == "PGP_SIGN=0" ]]'
sc macos GITHUB_EVENT_NAME=push GITHUB_REF=refs/tags/v1.0.0 DW_SIGNING_ENABLED=true DW_SIGNING_REQUIRED=1 \
  APPLE_CERTIFICATE=c APPLE_CERTIFICATE_PASSWORD=p APPLE_SIGNING_IDENTITY=i \
  APPLE_API_ISSUER=iss APPLE_API_KEY=k APPLE_API_KEY_P8=p8 APPLE_TEAM_ID=t
check "signing-check macos: complete set enables MACOS_SIGN=1" '[[ $scs -eq 0 && "$SC_ENV" == "MACOS_SIGN=1" ]]'
sc windows GITHUB_EVENT_NAME=push GITHUB_REF=refs/tags/v1.0.0 DW_SIGNING_ENABLED=true DW_SIGNING_REQUIRED=1 \
  AZURE_TENANT_ID=t AZURE_CLIENT_ID=c AZURE_CLIENT_SECRET=s \
  AZURE_SIGNING_ENDPOINT=https://eus.codesigning.azure.net AZURE_SIGNING_ACCOUNT=a AZURE_SIGNING_PROFILE=p
check "signing-check windows: complete set enables WINDOWS_SIGN=1" '[[ $scs -eq 0 && "$SC_ENV" == "WINDOWS_SIGN=1" ]]'
sc pgp GITHUB_EVENT_NAME=push GITHUB_REF=refs/tags/v1.0.0 DW_SIGNING_ENABLED=true DW_SIGNING_REQUIRED=1 \
  RELEASE_PGP_PRIVATE_KEY=k RELEASE_PGP_PASSPHRASE=p RELEASE_PGP_FINGERPRINT=0123456789ABCDEF0123456789ABCDEF01234567
check "signing-check pgp: complete set enables PGP_SIGN=1" '[[ $scs -eq 0 && "$SC_ENV" == "PGP_SIGN=1" ]]'
for t in macos windows pgp; do
  sc "$t" GITHUB_EVENT_NAME=push GITHUB_REF=refs/tags/v1.0.0-rc.1 DW_SIGNING_ENABLED=false DW_SIGNING_REQUIRED=1 APPLE_TEAM_ID=T AZURE_SIGNING_ACCOUNT=A RELEASE_PGP_FINGERPRINT=F
  check "signing-check $t: candidate (signing disabled) never requires signing" '[[ $scs -eq 0 && "$SC" != *"::error::"* && "$SC_ENV" == *"=0" ]]'
  sc "$t" GITHUB_EVENT_NAME=push GITHUB_REF=refs/tags/v1.0.0-rc.1 DW_SIGNING_ENABLED=true DW_SIGNING_REQUIRED=1
  check "signing-check $t: candidate (DW_SIGN_CANDIDATES=1) still never requires signing" '[[ $scs -eq 0 && "$SC" != *"::error::"* && "$SC_ENV" == *"=0" ]]'
done
sc macos GITHUB_EVENT_NAME=push GITHUB_REF=refs/heads/main DW_SIGNING_ENABLED=false DW_SIGNING_REQUIRED=1 APPLE_TEAM_ID=TEAM123
check "signing-check macos: main push with repo var set builds quietly without secrets" \
  '[[ $scs -eq 0 && "$SC" != *"::"* && "$SC_ENV" == "MACOS_SIGN=0" ]]'

# --- release-pgp-sign.sh: subkey import, fingerprint pin, detached signature, verification, and cleanup
if command -v gpg >/dev/null 2>&1; then
  gpg_gen=$work/gpg-gen
  mkdir -p "$gpg_gen" && chmod 700 "$gpg_gen"
  GNUPGHOME=$gpg_gen gpg --batch --passphrase "subkey-pass" --quick-generate-key "Release Test <rel@example.com>" ed25519 cert never >/dev/null 2>&1
  gpg_fpr=$(GNUPGHOME=$gpg_gen gpg --batch --with-colons --list-secret-keys | awk -F: '$1=="sec"{s=1;next} s&&$1=="fpr"{print $10; exit}')
  GNUPGHOME=$gpg_gen gpg --batch --pinentry-mode loopback --passphrase "subkey-pass" --quick-add-key "$gpg_fpr" ed25519 sign never >/dev/null 2>&1
  gpg_subkey=$(GNUPGHOME=$gpg_gen gpg --batch --pinentry-mode loopback --passphrase "subkey-pass" --armor --export-secret-subkeys "$gpg_fpr" 2>/dev/null)
  gpg_pub=$(GNUPGHOME=$gpg_gen gpg --batch --armor --export "$gpg_fpr" 2>/dev/null)
  GNUPGHOME=$gpg_gen gpgconf --kill gpg-agent >/dev/null 2>&1 || true

  pgp_sums=$work/pgp-SHA256SUMS
  printf 'deadbeef  dash-wallet-1.0.0-linux-x86_64.AppImage\n' > "$pgp_sums"
  pgp_rt=$work/pgp-runner-temp
  mkdir -p "$pgp_rt"

  pgp_rc=0
  RUNNER_TEMP=$pgp_rt RELEASE_PGP_PRIVATE_KEY=$gpg_subkey RELEASE_PGP_PASSPHRASE=subkey-pass \
    RELEASE_PGP_FINGERPRINT=$gpg_fpr "$here/release-pgp-sign.sh" "$pgp_sums" > "$work/pgp.out" 2>&1 || pgp_rc=$?
  check "release-pgp-sign: signs SHA256SUMS with subkey, verifies against primary fingerprint, and removes GNUPGHOME" \
    '[[ $pgp_rc -eq 0 && -s "$pgp_sums.asc" && ! -e "$pgp_rt/gnupg-release" ]]'

  gpg_ver=$work/gpg-verify
  mkdir -p "$gpg_ver" && chmod 700 "$gpg_ver"
  GNUPGHOME=$gpg_ver gpg --batch --quiet --import <<<"$gpg_pub" >/dev/null 2>&1
  check "release-pgp-sign: produced SHA256SUMS.asc verifies cleanly with the public key" \
    'GNUPGHOME=$gpg_ver gpg --batch --verify "$pgp_sums.asc" "$pgp_sums" >/dev/null 2>&1'
  GNUPGHOME=$gpg_ver gpgconf --kill gpg-agent >/dev/null 2>&1 || true

  rm -f "$pgp_sums.asc"
  pgp_bad_rc=0
  RUNNER_TEMP=$pgp_rt RELEASE_PGP_PRIVATE_KEY=$gpg_subkey RELEASE_PGP_PASSPHRASE=subkey-pass \
    RELEASE_PGP_FINGERPRINT=0000000000000000000000000000000000000000 "$here/release-pgp-sign.sh" "$pgp_sums" > "$work/pgp-bad.out" 2>&1 || pgp_bad_rc=$?
  check "release-pgp-sign: mismatched RELEASE_PGP_FINGERPRINT fails and writes no signature" \
    '[[ $pgp_bad_rc -ne 0 && ! -e "$pgp_sums.asc" && ! -e "$pgp_rt/gnupg-release" ]] && grep -qF "does not match RELEASE_PGP_FINGERPRINT" "$work/pgp-bad.out"'
fi

# --- release-version.sh
cd "$here/../.."
v() { env -i PATH="$PATH" GITHUB_ENV="$work/env" "$@" ci/github/release-version.sh > /dev/null 2>&1 && tail -1 "$work/env"; }
: > "$work/env"
check "version: main push" '[[ $(v GITHUB_REF_TYPE=branch GITHUB_SHA=1234567890 GITHUB_RUN_NUMBER=5) == "VERSION=0.3.0-dev.5.g1234567" ]]'
check "version: tag v0.3.0" '[[ $(v GITHUB_REF_TYPE=tag GITHUB_REF_NAME=v0.3.0) == "VERSION=0.3.0" ]]'
check "version: dry-run tag" '[[ $(v GITHUB_REF_TYPE=tag GITHUB_REF_NAME=v0.0.0-rel8) == "VERSION=0.0.0-rel8" ]]'
check "version: rc tag of the same version" '[[ $(v GITHUB_REF_TYPE=tag GITHUB_REF_NAME=v0.3.0-rc.1) == "VERSION=0.3.0-rc.1" ]]'
check "version: rc tag of another version fails" '! v GITHUB_REF_TYPE=tag GITHUB_REF_NAME=v0.4.0-rc.1 >/dev/null'
check "version: a tag that differs from tauri.conf.json fails" '! v GITHUB_REF_TYPE=tag GITHUB_REF_NAME=v9.9.9 >/dev/null'

exit $fail
