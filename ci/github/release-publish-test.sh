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
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null PYTHONDONTWRITEBYTECODE=1
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
      "GET releases/1/assets")
        [[ "${FAKE_API_ERROR:-}" != assets ]] || e503
        if [[ -n "${FAKE_TAG_MANIFEST:-}" && -f "${FAKE_TAG_MANIFEST}" ]]; then
          python3 - "$FAKE_TAG_MANIFEST" "${FAKE_TAG_DIGEST_MISMATCH:-}" <<'PY'
import json, sys
m = json.loads(open(sys.argv[1]).read())
mismatch = sys.argv[2] == "1"
for idx, (name, digest) in enumerate(sorted(m["files"].items()), 101):
    if mismatch and idx == 101:
        digest = "0" * 64
    print(f"{idx}\t{name}\tsha256:{digest}")
PY
        fi
        exit 0 ;;
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
if ! command -v sha256sum >/dev/null 2>&1; then
  printf '#!/usr/bin/env bash\nexec shasum -a 256 "$@"\n' > "$bin/sha256sum"
  chmod +x "$bin/sha256sum"
fi
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
DIST=$work/tagdist DW_ALLOW_UNSIGNED_STABLE=1 run push refs/tags/v0.1.0 "$c2" 0.1.0
check "tag v0.1.0 (DW_ALLOW_UNSIGNED_STABLE=1, no feed): full release, not prerelease, --latest=false so it never displaces a signed updater feed" \
  '[[ $status -eq 0 ]] && log_has "release create v0.1.0 " && ! log_has "--prerelease" && log_has "--latest=false"'
fs_reset
DIST=$work/tagdist DW_ALLOW_UNSIGNED_STABLE=1 DW_UPDATER_REQUIRED=1 run push refs/tags/v0.1.0 "$c2" 0.1.0
refused "DW_UPDATER_REQUIRED=1 refuses unsigned stable override without latest.json"
tag_bundles 0.1.0-rc.1; fs_reset
DIST=$work/tagdist run push refs/tags/v0.1.0-rc.1 "$c2" 0.1.0-rc.1
check "tag v0.1.0-rc.1: a prerelease, not Latest" \
  '[[ $status -eq 0 ]] && log_has "release create v0.1.0-rc.1 " && log_has "--prerelease" && log_has "--latest=false"'
# r4687084410: publish must refuse staged $OUT/attest prepared for a different tag/version
tag_bundles 0.1.0-rc.2; fs_reset
DIST=$work/tagdist VERSION=0.1.0-rc.2 GITHUB_EVENT_NAME=push GITHUB_REF=refs/tags/v0.1.0-rc.2 GITHUB_SHA=$c2 publish_only
check "release-publish refuses staged attest dir prepared for a different tag (v0.1.0-rc.1 vs v0.1.0-rc.2)" \
  '[[ $status -ne 0 && $(writes) -eq 0 ]] && grep -q "prepared for a different" "$work/pub.out"'

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
if [[ "${DW_TEST_FAST:-}" != 1 ]]; then
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
fi
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
FEED=$work/fout DIST=$work/tagdist DW_ALLOW_UNSIGNED_STABLE=1 run push refs/tags/v0.1.0 "$c2" 0.1.0
check "tag v0.1.0 with the feed: latest.json and the macOS archive are attested and published with the release" \
  '[[ $status -eq 0 && -f "$OUT/attest/latest.json" && -f "$OUT/attest/dash-wallet-0.1.0-macos-universal.app.tar.gz" ]] && log_has "release create v0.1.0 " && log_has "/latest.json" && ! log_has "--prerelease"'
FEED=$work/fout DIST=$work/tagdist DW_ALLOW_UNSIGNED_STABLE=1 FAKE_RELEASES=v0.1.0 FAKE_ASSET_COUNT=10 run push refs/tags/v0.1.0 "$c2" 0.1.0
check "tag v0.1.0 with the feed, rerun: the complete release (10 assets) is left as it is" '[[ $status -eq 0 ]] && ! log_has "release create"'
fs_reset
FEED=$work/empty-feed DIST=$work/tagdist DW_ALLOW_UNSIGNED_STABLE=1 run push refs/tags/v0.1.0 "$c2" 0.1.0
check "tag v0.1.0 without a feed (DW_ALLOW_UNSIGNED_STABLE=1): the release as before" '[[ $status -eq 0 && ! -f "$OUT/attest/latest.json" ]] && ! log_has "latest.json"'
# L6: misrouted updater URL (darwin pointing to .exe or ../) is refused by release-publish.sh
cp -R "$work/fout" "$work/fout-badurl"
jq '.platforms["darwin-aarch64-app"].url = "https://github.com/o/r/releases/download/v0.1.0/dash-wallet-0.1.0-windows-x64-setup.exe"' "$work/fout/latest.json" > "$work/fout-badurl/latest.json"
fs_reset
FEED=$work/fout-badurl DIST=$work/tagdist DW_ALLOW_UNSIGNED_STABLE=1 run push refs/tags/v0.1.0 "$c2" 0.1.0
refused "L6: updater feed with wrong platform filename in URL is refused"
feed_of 0.1.0-rc.1 "${ALL_UPDATER[@]}"
check "feed: a release candidate gets none" '[[ $fs -eq 0 && ! -e "$work/fout" ]]'
feed_of 0.1.0
check "feed: a build without updater sigs gets none" '[[ $fs -eq 0 && ! -e "$work/fout" ]]'
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
FEED=$work/fout DIST=$work/tagdist DW_ALLOW_UNSIGNED_STABLE=1 run push refs/tags/v0.1.0 "$c2" 0.1.0; refused "another version's feed is refused"

# --- Detached signing, fail-closed default (Decision 1), pinned manifest (H2/L4), keyless RC unchanged
write_manifest() { # write_manifest VERSION DIST FEED OUT_MANIFEST
  python3 - "$1" "$2" "$3" "$4" <<'PY'
import hashlib, json, sys
from pathlib import Path
v, d, f, m = sys.argv[1:]
names = [
    "SHA256SUMS", "SHA256SUMS.asc", "latest.json",
    f"dash-wallet-{v}-linux-amd64.deb",
    f"dash-wallet-{v}-linux-x86_64.AppImage",
    f"dash-wallet-{v}-linux-x86_64.flatpak",
    f"dash-wallet-{v}-macos-universal.app.tar.gz",
    f"dash-wallet-{v}-macos-universal.dmg",
    f"dash-wallet-{v}-sbom-npm.cdx.json",
    f"dash-wallet-{v}-sbom-rust.cdx.json",
    f"dash-wallet-{v}-windows-x64-setup.exe",
]
files = {}
for n in sorted(names):
    p = Path(d) / n
    if not p.is_file():
        p = Path(f) / n
    files[n] = hashlib.sha256(p.read_bytes()).hexdigest()
Path(m).write_text(json.dumps({
    "schema": 1,
    "version": v,
    "pgp_fpr": "A" * 40,
    "apple_team_id": "TESTTEAM01",
    "windows_leaf_sha256": "sha256:" + "b" * 64,
    "files": files,
}, indent=2, sort_keys=True) + "\n")
PY
}

tag_bundles 0.1.0; printf -- '-----BEGIN PGP SIGNATURE-----\ntest\n-----END PGP SIGNATURE-----\n' > "$work/tagdist/SHA256SUMS.asc"; fs_reset
DIST=$work/tagdist DW_ALLOW_UNSIGNED_STABLE=1 run push refs/tags/v0.1.0 "$c2" 0.1.0
check "tag v0.1.0 with SHA256SUMS.asc (DW_ALLOW_UNSIGNED_STABLE=1): attested and published (9 assets)" \
  '[[ $status -eq 0 && -f "$OUT/attest/SHA256SUMS.asc" ]] && log_has "release create v0.1.0 " && log_has "/SHA256SUMS.asc"'
DIST=$work/tagdist DW_ALLOW_UNSIGNED_STABLE=1 FAKE_RELEASES=v0.1.0 FAKE_ASSET_COUNT=9 run push refs/tags/v0.1.0 "$c2" 0.1.0
check "tag v0.1.0 with SHA256SUMS.asc, rerun: complete release (9 assets) is left as it is" \
  '[[ $status -eq 0 ]] && ! log_has "release create"'
feed_of 0.1.0 "${ALL_UPDATER[@]}"
write_manifest 0.1.0 "$work/tagdist" "$work/fout" "$work/signed-manifest.json"
fs_reset
SIGNED_MANIFEST=$work/signed-manifest.json FEED=$work/fout DIST=$work/tagdist run push refs/tags/v0.1.0 "$c2" 0.1.0
check "stable tag (fail-closed default): with attached signatures (SHA256SUMS.asc, feed, 11-file manifest) publishes all 11 attested assets" \
  '[[ $status -eq 0 && -f "$OUT/attest/SHA256SUMS.asc" && -f "$OUT/attest/latest.json" ]] && log_has "/attest/SHA256SUMS.asc" && log_has "/attest/latest.json"'
SIGNED_MANIFEST=$work/signed-manifest.json FEED=$work/fout DIST=$work/tagdist FAKE_RELEASES=v0.1.0 FAKE_ASSET_COUNT=11 FAKE_TAG_MANIFEST=$work/signed-manifest.json run push refs/tags/v0.1.0 "$c2" 0.1.0
check "Blocker #9: rerun of complete 11-asset signed release with matching remote digests is left as it is" \
  '[[ $status -eq 0 ]] && ! log_has "release create"'
fs_reset
SIGNED_MANIFEST=$work/signed-manifest.json FEED=$work/fout DIST=$work/tagdist FAKE_RELEASES=v0.1.0 FAKE_ASSET_COUNT=11 FAKE_TAG_MANIFEST=$work/signed-manifest.json FAKE_TAG_DIGEST_MISMATCH=1 run push refs/tags/v0.1.0 "$c2" 0.1.0
check "Blocker #9: rerun of existing 11-asset signed release with differing remote asset digest fails closed" \
  '[[ $status -ne 0 && $(writes) -eq 0 ]] && grep -q "from signed-manifest.json" "$work/pub.out"'
# Blocker #1: real workflow_dispatch with DW_SIGNED_DISPATCH=1 publishes signed tag, refuses unsigned/RC/main
fs_reset
SIGNED_MANIFEST=$work/signed-manifest.json FEED=$work/fout DIST=$work/tagdist DW_SIGNED_DISPATCH=1 DW_SIGNING_REQUIRED=1 run workflow_dispatch refs/tags/v0.1.0 "$c2" 0.1.0
check "Blocker #1: workflow_dispatch with DW_SIGNED_DISPATCH=1 publishes all 11 attested signed assets" \
  '[[ $status -eq 0 && $(ls "$OUT/attest" | wc -l) -eq 11 ]] && log_has "release create v0.1.0 " && log_has "/attest/SHA256SUMS.asc" && log_has "/attest/latest.json"'
fs_reset
SIGNED_MANIFEST=$work/signed-manifest.json FEED=$work/fout DIST=$work/tagdist run workflow_dispatch refs/tags/v0.1.0 "$c2" 0.1.0
check "Blocker #1: workflow_dispatch WITHOUT DW_SIGNED_DISPATCH=1 publishes nothing" '[[ $status -ne 0 && $(writes) -eq 0 ]]'
fs_reset
DIST=$work/tagdist DW_SIGNED_DISPATCH=1 DW_ALLOW_UNSIGNED_STABLE=1 run workflow_dispatch refs/tags/v0.1.0 "$c2" 0.1.0
refused "Blocker #1: DW_SIGNED_DISPATCH=1 refuses unsigned even if DW_ALLOW_UNSIGNED_STABLE=1"
fs_reset
DIST=$work/tagdist DW_SIGNED_DISPATCH=1 run workflow_dispatch refs/tags/v0.1.0-rc.1 "$c2" 0.1.0-rc.1
check "Blocker #1: DW_SIGNED_DISPATCH=1 never publishes a release candidate" '[[ $status -ne 0 && $(writes) -eq 0 ]]'
# H2: signed-manifest.json missing pgp_fpr is refused
jq 'del(.pgp_fpr)' "$work/signed-manifest.json" > "$work/signed-manifest-nofpr.json"
fs_reset
SIGNED_MANIFEST=$work/signed-manifest-nofpr.json FEED=$work/fout DIST=$work/tagdist run push refs/tags/v0.1.0 "$c2" 0.1.0
refused "H2: signed-manifest.json without pgp_fpr is refused"
# Missing signed-manifest.json (even with .asc and feed) is refused by default
fs_reset
SIGNED_MANIFEST=$work/no-such-manifest.json FEED=$work/fout DIST=$work/tagdist run push refs/tags/v0.1.0 "$c2" 0.1.0
refused "stable tag: final tag without signed-manifest.json fails closed by default"
check "stable tag: missing-manifest refusal names attached signatures, not CI secrets" \
  'grep -qF "signed-manifest.json" "$work/prep.out" && ! grep -qF "PRIVATE_KEY" "$work/prep.out"'
# L4: Tampering ANY of the 11 files (e.g., linux .deb, not just .exe) is refused
echo "tampered" >> "$work/tagdist/dash-wallet-0.1.0-linux-amd64.deb"
fs_reset
SIGNED_MANIFEST=$work/signed-manifest.json FEED=$work/fout DIST=$work/tagdist run push refs/tags/v0.1.0 "$c2" 0.1.0
refused "L4: final tag with tampered Linux .deb (11-file manifest mismatch) is refused"
# Decision 1: Unsigned final tag fails closed by default; DW_ALLOW_UNSIGNED_STABLE=1 allows it; DW_SIGNING_REQUIRED=1 overrides escape hatch
rm -f "$work/signed-manifest.json"
tag_bundles 0.1.0; fs_reset
DIST=$work/tagdist run push refs/tags/v0.1.0 "$c2" 0.1.0
refused "Decision 1: unsigned final tag fails closed by default (no env vars set)"
check "Decision 1: unsigned final tag refusal names SHA256SUMS.asc, updater feed, and signed-manifest.json" \
  'grep -qF "SHA256SUMS.asc" "$work/prep.out" && grep -qF "updater feed" "$work/prep.out" && grep -qF "signed-manifest.json" "$work/prep.out" && ! grep -qF "PRIVATE_KEY" "$work/prep.out"'
fs_reset
DIST=$work/tagdist DW_ALLOW_UNSIGNED_STABLE=1 run push refs/tags/v0.1.0 "$c2" 0.1.0
check "Decision 1: unsigned final tag succeeds only with explicit DW_ALLOW_UNSIGNED_STABLE=1" \
  '[[ $status -eq 0 && ! -f "$OUT/attest/SHA256SUMS.asc" ]] && ! log_has "SHA256SUMS.asc"'
fs_reset
DIST=$work/tagdist DW_ALLOW_UNSIGNED_STABLE=1 DW_SIGNING_REQUIRED=1 run push refs/tags/v0.1.0 "$c2" 0.1.0
refused "Decision 1: DW_SIGNING_REQUIRED=1 overrides DW_ALLOW_UNSIGNED_STABLE=1"
# Keyless RC stays unchanged even when DW_SIGNING_REQUIRED=1 (8 assets, no .asc, nothing required)
tag_bundles 0.1.0-rc.1; fs_reset
DIST=$work/tagdist DW_SIGNING_REQUIRED=1 run push refs/tags/v0.1.0-rc.1 "$c2" 0.1.0-rc.1
check "keyless release candidate publishes 8 assets unchanged (no .asc, no feed, no override needed)" \
  '[[ $status -eq 0 && $(ls "$OUT/attest" | wc -l) -eq 8 ]] && log_has "release create v0.1.0-rc.1 " && log_has "--prerelease" && ! log_has "SHA256SUMS.asc" && ! log_has "latest.json"'
rm -rf "$work/maindist"; cp -R "$dist" "$work/maindist"
printf -- '-----BEGIN PGP SIGNATURE-----\nmain\n-----END PGP SIGNATURE-----\n' > "$work/maindist/SHA256SUMS.asc"; fs_reset; unset FAKE_CONT_RELEASE
DIST=$work/maindist run push refs/heads/main "$c2" "$ver"
refused "main never publishes SHA256SUMS.asc"

# --- tauri-build.sh: DW_ENABLE_UPDATER=1 embeds stable feed URL with zero private keys & rejects retired/small-order/overlay keys
python3 - "$work" "$here" <<'PY'
import base64, importlib.util, json, sys
from pathlib import Path
wk, here = Path(sys.argv[1]), Path(sys.argv[2])
vm_spec = importlib.util.spec_from_file_location("vm_tb", str(here / "verify-minisign.py"))
vm = importlib.util.module_from_spec(vm_spec); vm_spec.loader.exec_module(vm)

def enc(pt):
    x, y, z, _ = pt
    zi = pow(z, vm._P - 2, vm._P)
    return (((y * zi) % vm._P) | ((((x * zi) % vm._P) & 1) << 255)).to_bytes(32, "little")

# Find an order-8 torsion point T8 and compute torsion-shifted retired key A' = A_retired + T8 (L2)
ident = (1).to_bytes(32, "little")
t8 = None
for y in range(2, 10000):
    pt = vm._ed_decode_point(y.to_bytes(32, "little"))
    if pt is None:
        continue
    q = vm._ed_scalar_mul(pt, vm._L)
    if enc(q) != ident and enc(vm._ed_scalar_mul(q, 4)) != ident:
        t8 = q
        break
assert t8 is not None

valid_ed25519 = bytes.fromhex("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a")
retired_ed25519 = bytes.fromhex("5482c426fb97af310e6cccdb492b20e0fa2fa94dc9bc0424066ecd7e1a144152")
torsion_retired_ed25519 = enc(vm._ed_point_add(vm._ed_decode_point(retired_ed25519), t8))

def make_conf(kid_hex: str, path: Path, ed_bytes: bytes = valid_ed25519) -> None:
    kid = bytes.fromhex(kid_hex)[::-1]
    pub = b"Ed" + kid + ed_bytes
    b64 = base64.b64encode(f"untrusted comment: minisign public key: {kid_hex}\n{base64.b64encode(pub).decode()}\n".encode()).decode()
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps({"plugins": {"updater": {"pubkey": b64}}}))

make_conf("1122334455667788", wk / "tb-ok/tauri.conf.json")
make_conf("ED322B963B527B21", wk / "tb-retired/tauri.conf.json")
make_conf("1122334455667788", wk / "tb-relabelled/tauri.conf.json", retired_ed25519)
make_conf("1122334455667788", wk / "tb-torsion/tauri.conf.json", torsion_retired_ed25519)
make_conf("1122334455667788", wk / "tb-ident/tauri.conf.json", ident)
# L1: valid base tauri.conf.json with a sibling tauri.macos.conf.json attempting a pubkey override
make_conf("1122334455667788", wk / "tb-overlay/tauri.conf.json")
make_conf("ED322B963B527B21", wk / "tb-overlay/tauri.macos.conf.json")
PY
tb() { # tb ENABLE_UPDATER LOCAL_KEY [TAURI_CONF] [EXTRA_ARGS...]: runs tauri-build.sh with fake pnpm; sets $tbs, $TB
  local env_file=$work/tb-env en=$1 lk=$2 tc=${3:-$work/tb-ok/tauri.conf.json}
  shift 3 2>/dev/null || shift $#
  : > "$env_file"
  tbs=0
  TB=$(cd "$work" && PATH="$work/tb-bin:$PATH" DW_ENABLE_UPDATER=$en TAURI_SIGNING_PRIVATE_KEY=$lk \
    TAURI_CONF=$tc GITHUB_ENV=$env_file "$here/tauri-build.sh" --bundles appimage "$@" 2>&1) || tbs=$?
}
mkdir -p "$work/tb-bin"
printf '#!/usr/bin/env bash\necho "pnpm $*"\n' > "$work/tb-bin/pnpm"; chmod +x "$work/tb-bin/pnpm"
tb 1 "" "$work/tb-ok/tauri.conf.json"
check "tauri-build: DW_ENABLE_UPDATER=1 merges feed URL with createUpdaterArtifacts=false and no secret" \
  '[[ $tbs -eq 0 && "$TB" == *"--config ../../rust/crates/dw-app/tauri.updater.conf.json"* && "$TB" == *"createUpdaterArtifacts\":false"* && ! -s "$work/tb-env" ]]'
tb 1 "" "$work/tb-retired/tauri.conf.json"
check "Blocker #6: tauri-build refuses DW_ENABLE_UPDATER=1 with retired CI key ED322B963B527B21" \
  '[[ $tbs -ne 0 && "$TB" == *"ED322B963B527B21"* ]]'
tb 1 "" "$work/tb-relabelled/tauri.conf.json"
check "tauri-build refuses DW_ENABLE_UPDATER=1 when retired CI Ed25519 pubkey bytes are relabelled with a new key ID" \
  '[[ $tbs -ne 0 && "$TB" == *"7821a52f7420d388"* ]]'
tb 1 "" "$work/tb-torsion/tauri.conf.json"
check "L2: tauri-build refuses torsion-shifted retired Ed25519 pubkey (A' = A + T8)" \
  '[[ $tbs -ne 0 && "$TB" == *"non-prime-order"* ]]'
tb 1 "" "$work/tb-ident/tauri.conf.json"
check "L2: tauri-build refuses small-order identity Ed25519 pubkey" \
  '[[ $tbs -ne 0 && "$TB" == *"non-prime-order"* ]]'
tb 1 "" "$work/tb-overlay/tauri.conf.json"
check "L1: tauri-build refuses plugins.updater.pubkey override in sibling tauri.macos.conf.json" \
  '[[ $tbs -ne 0 && "$TB" == *"tauri.macos.conf.json"* ]]'
tb 1 "" "$work/tb-ok/tauri.conf.json" --config '{"plugins":{"updater":{"pubkey":"evil"}}}'
check "L1: tauri-build refuses plugins.updater.pubkey override in inline --config JSON" \
  '[[ $tbs -ne 0 && "$TB" == *"inline --config JSON"* ]]'
tb 0 "" "$work/tb-retired/tauri.conf.json"
check "tauri-build: plain build without DW_ENABLE_UPDATER adds no updater config (even before key rotation)" \
  '[[ $tbs -eq 0 && "$TB" == *"pnpm tauri build --bundles appimage"* && "$TB" != *config* && ! -s "$work/tb-env" ]]'
tb 0 local-key
check "tauri-build: local developer build with TAURI_SIGNING_PRIVATE_KEY still enables UPDATER=1" \
  '[[ $tbs -eq 0 && "$TB" == *"--config ../../rust/crates/dw-app/tauri.updater.conf.json"* && "$TB" != *"createUpdaterArtifacts\":false"* ]] && grep -qx UPDATER=1 "$work/tb-env"'
export GITHUB_SHA=$c2

# --- classify and decide
check "classify: main push" '[[ $(GITHUB_EVENT_NAME=push GITHUB_REF=refs/heads/main "$here/release-publish.sh" classify) == main ]]'
check "classify: release tag" '[[ $(GITHUB_EVENT_NAME=push GITHUB_REF=refs/tags/v0.1.0-rc.2 "$here/release-publish.sh" classify) == tag ]]'
check "classify: signed workflow_dispatch on stable tag" '[[ $(GITHUB_EVENT_NAME=workflow_dispatch GITHUB_REF=refs/tags/v0.1.0 DW_SIGNED_DISPATCH=1 "$here/release-publish.sh" classify) == tag ]]'
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
stable_tag() { decide "$@" > /dev/null; sed -n 's/^stable_tag=//p' "$work/decide.out"; }
signed_stable() { decide "$@" > /dev/null; sed -n 's/^signed_stable=//p' "$work/decide.out"; }
check "decide: only a final stable tag on main sets stable_tag=true" '[[ $(stable_tag push refs/tags/v0.1.0 "$c2" "") == true ]]'
check "decide: an rc, a dry run, a tag off main, main and a dispatch set stable_tag=false" \
  '[[ "$(stable_tag push refs/tags/v0.1.0-rc.1 "$c2" "") $(stable_tag push refs/tags/v0.0.0-rel8 "$c2" "") $(stable_tag push refs/tags/v0.1.0 "$side" "") $(stable_tag push refs/heads/main "$c2" "") $(stable_tag workflow_dispatch refs/tags/v0.1.0 "$c2" "")" == "false false false false false" ]]'
check "Blocker #6: decide sets signed_stable=true by default, false under DW_ALLOW_UNSIGNED_STABLE=1, true when DW_SIGNING_REQUIRED=1 overrides" \
  '[[ $(signed_stable push refs/tags/v0.1.0 "$c2" "") == true && $(DW_ALLOW_UNSIGNED_STABLE=1 signed_stable push refs/tags/v0.1.0 "$c2" "") == false && $(DW_ALLOW_UNSIGNED_STABLE=1 DW_SIGNING_REQUIRED=1 signed_stable push refs/tags/v0.1.0 "$c2" "") == true ]]'
check "decide: a docs-only pull request does not build" '[[ $(decide pull_request refs/pull/9/merge "$newer" "$c2") == "false false" ]]'
check "decide: a code change builds" '[[ $(decide push refs/heads/dw/x "$code" "$c2") == "false true" ]]'
check "decide: a docs-only push after a code push on the branch still builds (measured from main, not the last push)" '[[ $(decide push refs/heads/dw/x "$docsafter" "$code") == "false true" ]]'
check "decide: moving a source file into docs/ is a code change" '[[ $(decide pull_request refs/pull/9/merge "$moved" "$code") == "false true" ]]'
check "decide: a pull request with no known base builds" '[[ $(decide pull_request refs/pull/9/merge "$newer" "") == "false true" ]]'
check "decide: a docs-only change on a tag still builds" '[[ $(decide push refs/tags/v0.1.0 "$newer" "$c2") == "false true" ]]'

# --- End-to-end detached-sigs pipeline: macos-detached-sig.py, verify-minisign.py, osslsigncode, gpg, release-attach.sh -> release-publish.sh
missing_e2e_tools=()
for tool in python3 openssl gpg osslsigncode; do
  command -v "$tool" >/dev/null 2>&1 || missing_e2e_tools+=("$tool")
done
if ((${#missing_e2e_tools[@]})); then
  if [[ -n "${CI:-}" ]]; then
    echo "FAIL release-attach e2e: missing required tools in CI: ${missing_e2e_tools[*]}"
    fail=1
  else
    echo "SKIP release-attach e2e: missing tools: ${missing_e2e_tools[*]}"
  fi
else
  e2e=$work/detached-e2e
  mkdir -p "$e2e/unsigned" "$e2e/sigs" "$e2e/work" "$e2e/attached"
  python3 - "$e2e" "$here" <<'PY'
import base64, hashlib, json, os, shutil, struct, subprocess, sys, tempfile
from pathlib import Path

e2e = Path(sys.argv[1])
here = Path(sys.argv[2])
ver = "0.1.0"
unsigned = e2e / "unsigned"
sigs = e2e / "sigs"
wk = e2e / "work"

# 1. Create unchanged Linux + SBOM CI files
for suf in ("linux-x86_64.AppImage", "linux-amd64.deb", "linux-x86_64.flatpak", "sbom-rust.cdx.json", "sbom-npm.cdx.json"):
    (unsigned / f"dash-wallet-{ver}-{suf}").write_bytes(f"ci-{suf}\n".encode())

# 2. Create minimal valid PE32+ Windows installer, sign with throwaway cert, extract detached PEM
def make_minimal_pe() -> bytes:
    dos = bytearray(0x80)
    dos[0:2] = b"MZ"
    struct.pack_into("<I", dos, 0x3C, 0x80)
    coff = struct.pack("<HHIIIHH", 0x8664, 1, 0, 0, 0, 240, 0x0022)
    opt = bytearray(240)
    struct.pack_into("<H", opt, 0, 0x20B)
    struct.pack_into("<I", opt, 16, 0x1000)
    struct.pack_into("<Q", opt, 24, 0x140000000)
    struct.pack_into("<I", opt, 32, 0x1000)
    struct.pack_into("<I", opt, 36, 0x200)
    struct.pack_into("<HH", opt, 40, 6, 0)
    struct.pack_into("<HH", opt, 48, 6, 0)
    struct.pack_into("<I", opt, 56, 0x2000)
    struct.pack_into("<I", opt, 60, 0x200)
    struct.pack_into("<H", opt, 68, 3)
    struct.pack_into("<I", opt, 108, 16)
    sec = struct.pack("<8sIIIIIIHHI", b".text\x00\x00\x00", 0x200, 0x1000, 0x200, 0x200, 0, 0, 0, 0, 0x60000020)
    hdrs = (bytes(dos) + b"PE\x00\x00" + coff + bytes(opt) + sec).ljust(0x200, b"\x00")
    return hdrs + b"\xC3" + b"\x90" * (0x200 - 1)

win_name = f"dash-wallet-{ver}-windows-x64-setup.exe"
(unsigned / win_name).write_bytes(make_minimal_pe())
win_key = wk / "win.key"; win_cert = wk / "win.crt"; win_cnf = wk / "win.cnf"
# Use a config file instead of -addext so macOS system LibreSSL 3.3.6 and OpenSSL 1.1/3/4 all work.
win_cnf.write_text(
    "[req]\ndistinguished_name = dn\nx509_extensions = ext\nprompt = no\n"
    "[dn]\nCN = Dash Core Test Signer\n"
    "[ext]\nextendedKeyUsage = codeSigning\n"
)
subprocess.run([
    "openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes",
    "-keyout", str(win_key), "-out", str(win_cert), "-days", "2",
    "-config", str(win_cnf), "-extensions", "ext",
], check=True, capture_output=True)
(sigs / "win").mkdir(parents=True)
win_signed = wk / win_name
subprocess.run(["osslsigncode", "sign", "-certs", str(win_cert), "-key", str(win_key), "-h", "sha256",
                "-in", str(unsigned / win_name), "-out", str(win_signed)], check=True, capture_output=True)
subprocess.run(["osslsigncode", "extract-signature", "-pem", "-in", str(win_signed),
                "-out", str(sigs / "win" / f"{win_name}.pem")], check=True, capture_output=True)

# 3. Create synthetic universal FAT (x86_64 + arm64) macOS .app (unsigned ad-hoc vs signed+stapled), extract detached sigs
def make_slice(cputype: int, cs_blob: bytes, code_byte: bytes = b"\x90") -> bytes:
    text_body = code_byte * 256
    linkedit_pre = b"\xAA" * 64
    dataoff = 192 + len(text_body) + len(linkedit_pre)
    le_fileoff = 192 + len(text_body)
    le_filesize = len(linkedit_pre) + len(cs_blob)
    le_vmsize = ((le_filesize + 4095) // 4096) * 4096
    hdr = struct.pack("<8I", 0xFEEDFACF, cputype, 3, 2, 3, 160, 0, 0)
    seg_text = struct.pack("<2I16s4Q4I", 0x19, 72, b"__TEXT", 0x100000000, 4096, 0, le_fileoff, 5, 5, 0, 0)
    seg_le = struct.pack("<2I16s4Q4I", 0x19, 72, b"__LINKEDIT", 0x100001000, le_vmsize, le_fileoff, le_filesize, 1, 1, 0, 0)
    lc_cs = struct.pack("<4I", 0x1D, 16, dataoff, len(cs_blob))
    return hdr + seg_text + seg_le + lc_cs + text_body + linkedit_pre + cs_blob

def make_superblob(cms_payload: bytes) -> bytes:
    bw = struct.pack(">2I", 0xFADE0B01, 8 + len(cms_payload)) + cms_payload
    return struct.pack(">3I2I", 0xFADE0CC0, 20 + len(bw), 1, 0x00010000, 20) + bw

def make_fat(s1: bytes, s2: bytes) -> bytes:
    off1 = 4096
    off2 = ((off1 + len(s1) + 4095) // 4096) * 4096
    buf = bytearray(off2 + len(s2))
    struct.pack_into(">2I", buf, 0, 0xCAFEBABE, 2)
    struct.pack_into(">5I", buf, 8, 0x01000007, 3, off1, len(s1), 12)
    struct.pack_into(">5I", buf, 28, 0x0100000C, 3, off2, len(s2), 12)
    buf[off1:off1+len(s1)] = s1
    buf[off2:off2+len(s2)] = s2
    return bytes(buf)

u_app = wk / "unsigned-app/Dash Wallet.app"
s_app = wk / "signed-app/Dash Wallet.app"
for d in (u_app, s_app):
    (d / "Contents/MacOS").mkdir(parents=True)
    (d / "Contents/Info.plist").write_text("<plist/>")
adhoc = make_superblob(b"")
cms = make_superblob(b"APPLE-DEV-ID-CMS-PAYLOAD" * 16)
(u_app / "Contents/MacOS/dash-wallet").write_bytes(make_fat(make_slice(0x01000007, adhoc), make_slice(0x0100000C, adhoc)))
(s_app / "Contents/MacOS/dash-wallet").write_bytes(make_fat(make_slice(0x01000007, cms), make_slice(0x0100000C, cms)))
(s_app / "Contents/_CodeSignature").mkdir()
(s_app / "Contents/_CodeSignature/CodeResources").write_text("<plist>codesig</plist>")
(s_app / "Contents/CodeResources").write_bytes(b"APPLE-STAPLER-TICKET")

subprocess.run(["python3", str(here / "macos-detached-sig.py"), "pack-app", str(u_app),
                str(unsigned / f"dash-wallet-{ver}-macos-unsigned.tar.gz")], check=True, capture_output=True)
(unsigned / f"dash-wallet-{ver}-macos-universal.dmg").write_bytes(b"unsigned-dmg")
subprocess.run(["python3", str(here / "macos-detached-sig.py"), "extract",
                str(u_app), str(s_app), str(sigs / "osx/Dash Wallet.app")], check=True, capture_output=True)

# Verify tampering 1 byte of __TEXT is refused by extract
t_app = wk / "tampered-app/Dash Wallet.app"
shutil.copytree(s_app, t_app)
(t_app / "Contents/MacOS/dash-wallet").write_bytes(make_fat(make_slice(0x01000007, cms, b"\x91"), make_slice(0x0100000C, cms)))
tamper_res = subprocess.run(["python3", str(here / "macos-detached-sig.py"), "extract",
                             str(u_app), str(t_app), str(wk / "bad-sigs")], capture_output=True)
assert tamper_res.returncode != 0, "extract must refuse modified __TEXT bytes"

# Pack expected deterministic signed .app.tar.gz and synthetic signed .dmg
signed_app_tar = wk / f"dash-wallet-{ver}-macos-universal.app.tar.gz"
subprocess.run(["python3", str(here / "macos-detached-sig.py"), "pack-app", str(s_app), str(signed_app_tar)], check=True, capture_output=True)
signed_dmg = wk / f"dash-wallet-{ver}-macos-universal.dmg"
signed_dmg.write_bytes(b"signed-notarized-dmg")

# Write unsigned.SHA256SUMS in both unsigned/ (CI copy) and sigs/ (signer copy)
def sha256_file(p: Path) -> str:
    return hashlib.sha256(p.read_bytes()).hexdigest()

u_lines = [f"{sha256_file(p)}  {p.name}\n" for p in sorted(unsigned.iterdir(), key=lambda x: x.name)]
u_text = "".join(u_lines)
(unsigned / "unsigned.SHA256SUMS").write_text(u_text)
(sigs / "unsigned.SHA256SUMS").write_text(u_text)

# 4. Sign updater latest.json with pure-Python RFC 8032 Ed25519 (zero openssl Ed25519 CLI dependency;
#    works identically on macOS LibreSSL/OpenSSL 1.1.1w and Linux) and cross-check RFC 8032 §7.1 vectors.
import importlib.util
vm_spec = importlib.util.spec_from_file_location("vm", str(here / "verify-minisign.py"))
vm = importlib.util.module_from_spec(vm_spec); vm_spec.loader.exec_module(vm)

# Blocker #3 / L2: Cross-check pure-Python Ed25519 verifier against 4 official RFC 8032 §7.1 vectors
# (vectors 1-3 + SHA(abc)) and verify small-order identity and torsion-shifted keys are rejected.
rfc_vectors = [
    ("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a", "",
     "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b"),
    ("3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c", "72",
     "92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00"),
    ("fc51cd8e6218a1a38da47ed00230f0580816ed13ba3303ac5deb911548908025", "af82",
     "6291d657deec24024827e69c3abe01a30ce548a284743a445e3680d7db5ac3ac18ff9b538d16f290ae67f760984dc6594a7c15e9716ed28dc027beceea1ec40a"),
    ("ec172b93ad5e563bf4932c70e1245034c35467ef2efd4d64ebf819683467e2bf",
     "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f",
     "dc2a4459e7369633a52b1bf277839a00201009a3efbf3ecb69bea2186c26b58909351fc9ac90b3ecfdfbc7c66431e0303dca179c138ac17ad9bef1177331a704"),
]
for vpub, vmsg, vsig in rfc_vectors:
    assert vm.ed25519_verify(bytes.fromhex(vpub), bytes.fromhex(vmsg), bytes.fromhex(vsig)), "RFC 8032 vector failed"
ident_pt = (1).to_bytes(32, "little")
assert not vm.ed25519_verify(ident_pt, b"anything", ident_pt + bytes(32)), "identity pubkey must be rejected"

def ed_encode_point(pt):
    x, y, z, _ = pt
    zi = pow(z, vm._P - 2, vm._P)
    rx, ry = (x * zi) % vm._P, (y * zi) % vm._P
    return (ry | ((rx & 1) << 255)).to_bytes(32, "little")

def ed_keypair(seed: bytes):
    h = hashlib.sha512(seed).digest()
    a = int.from_bytes(h[:32], "little")
    a &= (1 << 254) - 8
    a |= 1 << 254
    return seed, a, h[32:], ed_encode_point(vm._ed_scalar_mul(vm._B, a))

def ed_sign(kp, msg: bytes) -> bytes:
    _, a, prefix, pub = kp
    r = int.from_bytes(hashlib.sha512(prefix + msg).digest(), "little") % vm._L
    r_enc = ed_encode_point(vm._ed_scalar_mul(vm._B, r))
    k = int.from_bytes(hashlib.sha512(r_enc + pub + msg).digest(), "little") % vm._L
    s = (r + k * a) % vm._L
    return r_enc + s.to_bytes(32, "little")

def make_minisign_pub(kid: bytes, pub: bytes) -> str:
    return base64.b64encode(
        f"untrusted comment: minisign public key: {kid[::-1].hex().upper()}\n"
        f"{base64.b64encode(b'Ed' + kid + pub).decode()}\n".encode()
    ).decode()

def minisign_with_kp(kp, kid: bytes, p: Path, version: str) -> str:
    digest = hashlib.blake2b(p.read_bytes(), digest_size=64).digest()
    art_sig = ed_sign(kp, digest)
    tc = f"timestamp:1700000000\tfile:{p.name}\tversion:{version}"
    gsig = ed_sign(kp, art_sig + tc.encode())
    box = (
        "untrusted comment: signature from tauri secret key\n"
        + base64.b64encode(b"ED" + kid + art_sig).decode() + "\n"
        + f"trusted comment: {tc}\n"
        + base64.b64encode(gsig).decode() + "\n"
    )
    return base64.b64encode(box.encode()).decode()

kp1 = ed_keypair(bytes(range(32)))
kp2 = ed_keypair(bytes(range(32, 64)))
key_id = bytes.fromhex("8877665544332211")  # key ID 1122334455667788 (non-retired)
retired_kid = bytes.fromhex("217b523b962b32ed")  # ED322B963B527B21 (retired CI key)
retired_ed25519 = bytes.fromhex("5482c426fb97af310e6cccdb492b20e0fa2fa94dc9bc0424066ecd7e1a144152")
# L2: Torsion-shifted retired key A' = A_retired + T8 (must be rejected by prime-order subgroup check)
t8_pt = None
for y_cand in range(2, 10000):
    pt_cand = vm._ed_decode_point(y_cand.to_bytes(32, "little"))
    if pt_cand is None:
        continue
    q_cand = vm._ed_scalar_mul(pt_cand, vm._L)
    if ed_encode_point(q_cand) != ident_pt and ed_encode_point(vm._ed_scalar_mul(q_cand, 4)) != ident_pt:
        t8_pt = q_cand
        break
assert t8_pt is not None
torsion_retired_ed25519 = ed_encode_point(vm._ed_point_add(vm._ed_decode_point(retired_ed25519), t8_pt))
# Also prove kp1's torsion-shifted A' = A1 + T8 cannot forge a signature accepted by ed25519_verify
a1_shifted = ed_encode_point(vm._ed_point_add(vm._ed_decode_point(kp1[3]), t8_pt))
for trial in range(16):
    r_pt = vm._ed_point_add(vm._ed_scalar_mul(vm._B, trial + 1), vm._ed_scalar_mul(t8_pt, trial % 8))
    r_enc = ed_encode_point(r_pt)
    k_val = int.from_bytes(hashlib.sha512(r_enc + a1_shifted + b"msg").digest(), "little") % vm._L
    s_val = ((trial + 1) + k_val * kp1[1]) % vm._L
    assert not vm.ed25519_verify(a1_shifted, b"msg", r_enc + s_val.to_bytes(32, "little")), "torsion-shifted key must be rejected"

(wk / "tauri.conf.json").write_text(json.dumps({"plugins": {"updater": {"pubkey": make_minisign_pub(key_id, kp1[3])}}}))
(wk / "tauri-retired.conf.json").write_text(json.dumps({"plugins": {"updater": {"pubkey": make_minisign_pub(retired_kid, kp1[3])}}}))
(wk / "tauri-relabelled-retired.conf.json").write_text(json.dumps({"plugins": {"updater": {"pubkey": make_minisign_pub(key_id, retired_ed25519)}}}))
(wk / "tauri-torsion-retired.conf.json").write_text(json.dumps({"plugins": {"updater": {"pubkey": make_minisign_pub(key_id, torsion_retired_ed25519)}}}))
# Test-owned placeholder signing-identities.json (never rely on production docs/release/signing-identities.json staying unpopulated)
(wk / "identities-placeholder.json").write_text(json.dumps({
    "schema": 1,
    "pgp_fingerprint": "REPLACE_WITH_40_HEX_PRIMARY_PGP_FINGERPRINT",
    "apple_team_id": "REPLACE_WITH_10_CHAR_APPLE_TEAM_ID",
    "windows_leaf_sha256": "sha256:REPLACE_WITH_64_HEX_WINDOWS_LEAF_CERT_SHA256",
}, indent=2) + "\n")

# Real Tauri CLI (@tauri-apps/cli 2.12.1) minisign fixture: exercises verify-minisign.py against
# genuine Tauri-emitted prehashing (BLAKE2b-512), envelope framing, trusted-comment signing, and
# pure-Python RFC 8032 Ed25519 verification on BOTH Linux and macOS without needing Node/pnpm in CI.
#
# How this public fixture was generated (throwaway private key discarded immediately after signing):
#   pnpm --dir apps/desktop tauri signer generate --ci -p '' -w /tmp/test.key
#   printf 'tauri-real-fixture-appimage-v0.1.0\n' > /tmp/dash-wallet-0.1.0-linux-x86_64.AppImage
#   printf 'tauri-real-fixture-windows-v0.1.0\n'  > /tmp/dash-wallet-0.1.0-windows-x64-setup.exe
#   printf 'tauri-real-fixture-macos-v0.1.0\n'    > /tmp/dash-wallet-0.1.0-macos-universal.app.tar.gz
#   pnpm --dir apps/desktop tauri signer sign --private-key-path /tmp/test.key --password '' --app-version 0.1.0 <each-file>
tauri_real_dir = wk / "tauri-real-fixture"
tauri_real_dir.mkdir()
tauri_real_pub = (
    "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDNERDExMzAwN0Y1Rjg1QkUK"
    "UldTK2hWOS9BQlBSUFVmS1dEdzQrTGFjWk91N1lPVk5ZQ3JORWFYdFh3b2d3cWZGcmVML096NjMK"
)
tauri_real_files = {
    "dash-wallet-0.1.0-linux-x86_64.AppImage": (
        b"tauri-real-fixture-appimage-v0.1.0\n",
        "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVTK2hWOS9BQlBSUFJxN1RtVTBSN2tOM2t0cmc0ZXYwMGVsdWNiMTVnWUdPQTlzWGtqUkNXRnltT1ZZb01jRkVJU1lJeGVxbGgzVSs1dVRxR01xLzl4TEhEK3YvekJmaHdBPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzk1NTU2Mjg2CWZpbGU6ZGFzaC13YWxsZXQtMC4xLjAtbGludXgteDg2XzY0LkFwcEltYWdlCXZlcnNpb246MC4xLjAKN1ZETUxOdHYwc01vUGVzMnptbENHa0xGY1RONWVqQm5zOVhPVUUvUGloOEgxQnhLT0N6SFliQXJEUW91NGp4eHE1MS91ZUhoUzdkbzdKQ3BkREVSQWc9PQo=",
    ),
    "dash-wallet-0.1.0-windows-x64-setup.exe": (
        b"tauri-real-fixture-windows-v0.1.0\n",
        "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVTK2hWOS9BQlBSUGVtQkNCSUxxTjFtKy82WXFTMFBzMVhzTzJnOHNHWTBNeWthM2gwcXMrV2d2MmdNcTBQN2FTdlFYQWNyL0lSVjJ3SUpZYWlETUNsUWdadWZjdFlZaVF3PQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzk1NTU2Mjg2CWZpbGU6ZGFzaC13YWxsZXQtMC4xLjAtd2luZG93cy14NjQtc2V0dXAuZXhlCXZlcnNpb246MC4xLjAKSGl4SzdWUXU5WVRjUXVyK0x4eU1FRnVqeFg4ZXUvbXJLWjlmRUUwaHZabW5jRzdBdkx0VWZpRGtCN1dlN0tGUy9saFkwVTVGVHBHcnRucGJWbUVERFE9PQo=",
    ),
    "dash-wallet-0.1.0-macos-universal.app.tar.gz": (
        b"tauri-real-fixture-macos-v0.1.0\n",
        "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVTK2hWOS9BQlBSUGNhQkJkdmxpY2orWXRub1NtSjlWRVA1SnBDajkzQVN1SG5KYW05eW9HS3lBUTNBQ2VHL3RwUXZialMvclQ4Vlg3YVNuaDFtSUVhaTF0c0VvODFPMmdNPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzk1NTU2Mjg3CWZpbGU6ZGFzaC13YWxsZXQtMC4xLjAtbWFjb3MtdW5pdmVyc2FsLmFwcC50YXIuZ3oJdmVyc2lvbjowLjEuMApCRTRIMVNkWFcrWEREbElpeEZ6MVp3SGhtNWc4S3JWM04xUlUvSWZ5RUZGUWF1aFE0eURjTDc4cGRwRVkxcE9DbTlFRHN1Yzc2bjFKWEFqVklCc3lBUT09Cg==",
    ),
}
for fname, (fbytes, _) in tauri_real_files.items():
    (tauri_real_dir / fname).write_bytes(fbytes)
real_base_url = f"https://github.com/o/r/releases/download/v{ver}"
(tauri_real_dir / "latest.json").write_text(json.dumps({
    "version": ver,
    "platforms": {
        "linux-x86_64-appimage": {"url": f"{real_base_url}/dash-wallet-{ver}-linux-x86_64.AppImage", "signature": tauri_real_files["dash-wallet-0.1.0-linux-x86_64.AppImage"][1]},
        "windows-x86_64-nsis": {"url": f"{real_base_url}/dash-wallet-{ver}-windows-x64-setup.exe", "signature": tauri_real_files["dash-wallet-0.1.0-windows-x64-setup.exe"][1]},
        "darwin-aarch64-app": {"url": f"{real_base_url}/dash-wallet-{ver}-macos-universal.app.tar.gz", "signature": tauri_real_files["dash-wallet-0.1.0-macos-universal.app.tar.gz"][1]},
        "darwin-x86_64-app": {"url": f"{real_base_url}/dash-wallet-{ver}-macos-universal.app.tar.gz", "signature": tauri_real_files["dash-wallet-0.1.0-macos-universal.app.tar.gz"][1]},
    },
}, indent=2) + "\n")
(tauri_real_dir / "pubkey.b64").write_text(tauri_real_pub)
# Also build a comment-tampered variant of latest.json (1-char change in trusted comment)
tampered_box = base64.b64decode(tauri_real_files["dash-wallet-0.1.0-linux-x86_64.AppImage"][1]).decode()
tampered_box = tampered_box.replace("timestamp:1795556286", "timestamp:1795556287")
tampered_latest = json.loads((tauri_real_dir / "latest.json").read_text())
tampered_latest["platforms"]["linux-x86_64-appimage"]["signature"] = base64.b64encode(tampered_box.encode()).decode()
(tauri_real_dir / "latest-badtc.json").write_text(json.dumps(tampered_latest, indent=2) + "\n")

# Temporary placeholder & populated Windows CA bundle fixtures (never assume docs/release/windows-ca.pem stays unpopulated)
(wk / "windows-ca-placeholder.pem").write_text(
    "# Placeholder template mentioning -----BEGIN CERTIFICATE----- in a comment only.\n"
    "# Replace with real CA certificates before 1.0.\n"
)
(wk / "windows-ca-populated.pem").write_text(
    "# Populated test Windows CA bundle (mirrors release owner committing real CA certs)\n"
    + (wk / "win.crt").read_text()
)

def minisign_file(p: Path, version: str) -> str:
    return minisign_with_kp(kp1, key_id, p, version)

# Blocker #5: Verify fixed golden SHA-256 of pack-app across Python 3.12/3.14/3.15 (compresslevel=9)
g_app = wk / "golden/Dash Wallet.app"
(g_app / "Contents/MacOS").mkdir(parents=True)
(g_app / "Contents/Info.plist").write_bytes(b"<plist>golden-v1</plist>\n")
g_exe = g_app / "Contents/MacOS/dash-wallet"
g_exe.write_bytes(b"\xcf\xfa\xed\xfeGOLDEN-PAYLOAD\n")
g_exe.chmod(0o755)
g_tar = wk / "golden.tar.gz"
subprocess.run(["python3", str(here / "macos-detached-sig.py"), "pack-app", str(g_app), str(g_tar)], check=True, capture_output=True)
got_golden = sha256_file(g_tar)
want_golden = "13bffd244eb69f184e432936b06f90711f8d07606c43155f9df254f7e0686a04"
assert got_golden == want_golden, f"pack-app golden digest mismatch: got {got_golden}, want {want_golden}"

(sigs / "updater").mkdir(parents=True)
latest_path = sigs / "updater/latest.json"
base_url = f"https://github.com/o/r/releases/download/v{ver}"
latest = {
    "version": ver,
    "platforms": {
        "linux-x86_64-appimage": {"url": f"{base_url}/dash-wallet-{ver}-linux-x86_64.AppImage", "signature": minisign_file(unsigned / f"dash-wallet-{ver}-linux-x86_64.AppImage", ver)},
        "windows-x86_64-nsis": {"url": f"{base_url}/{win_name}", "signature": minisign_file(win_signed, ver)},
        "darwin-aarch64-app": {"url": f"{base_url}/{signed_app_tar.name}", "signature": minisign_file(signed_app_tar, ver)},
        "darwin-x86_64-app": {"url": f"{base_url}/{signed_app_tar.name}", "signature": minisign_file(signed_app_tar, ver)},
    },
}
latest_path.write_text(json.dumps(latest, indent=2) + "\n")

# Compute real Windows leaf cert SHA-256 for M4 pinning test + sign a different PE32+ for wrong-.pem test
win_fp_out = subprocess.check_output(["openssl", "x509", "-in", str(wk / "win.crt"), "-noout", "-fingerprint", "-sha256"], text=True)
win_leaf_sha = "sha256:" + win_fp_out.split("=")[1].strip().replace(":", "").lower()
(wk / "win.leaf").write_text(win_leaf_sha)

other_pe = bytearray((unsigned / win_name).read_bytes())
other_pe[-1] ^= 0xFF
other_pe_path = wk / "other.exe"
other_pe_path.write_bytes(bytes(other_pe))
other_signed = wk / "other-signed.exe"
subprocess.run(["osslsigncode", "sign", "-certs", str(wk / "win.crt"), "-key", str(wk / "win.key"),
                "-h", "sha256", "-in", str(other_pe_path), "-out", str(other_signed)], check=True, capture_output=True)
subprocess.run(["osslsigncode", "extract-signature", "-pem", "-in", str(other_signed),
                "-out", str(wk / "other.exe.pem")], check=True, capture_output=True)

# Wrong-version and wrong-key minisign latest.json fixtures
latest_badver = json.loads(latest_path.read_text())
latest_badver["platforms"]["linux-x86_64-appimage"]["signature"] = minisign_file(unsigned / f"dash-wallet-{ver}-linux-x86_64.AppImage", "0.0.9")
(wk / "latest-badver.json").write_text(json.dumps(latest_badver, indent=2) + "\n")

latest_badkey = json.loads(latest_path.read_text())
latest_badkey["platforms"]["linux-x86_64-appimage"]["signature"] = minisign_with_kp(kp2, key_id, unsigned / f"dash-wallet-{ver}-linux-x86_64.AppImage", ver)
(wk / "latest-badkey.json").write_text(json.dumps(latest_badkey, indent=2) + "\n")

latest_badurl = json.loads(latest_path.read_text())
latest_badurl["platforms"]["darwin-aarch64-app"]["url"] = f"{base_url}/{win_name}"
(wk / "latest-badurl.json").write_text(json.dumps(latest_badurl, indent=2) + "\n")

# 5. Write final SHA256SUMS over all 9 files and PGP-sign with throwaway key
final_entries = [
    unsigned / f"dash-wallet-{ver}-linux-amd64.deb",
    unsigned / f"dash-wallet-{ver}-linux-x86_64.AppImage",
    unsigned / f"dash-wallet-{ver}-linux-x86_64.flatpak",
    signed_app_tar,
    signed_dmg,
    unsigned / f"dash-wallet-{ver}-sbom-npm.cdx.json",
    unsigned / f"dash-wallet-{ver}-sbom-rust.cdx.json",
    win_signed,
    latest_path,
]
final_entries.sort(key=lambda p: p.name)
(sigs / "SHA256SUMS").write_text("".join(f"{sha256_file(p)}  {p.name}\n" for p in final_entries))

# Use short /tmp dirs for GNUPGHOME so gpg-agent sockets stay well below macOS's 104-byte UNIX_PATH_MAX.
gpg_root = Path(tempfile.mkdtemp(dir="/tmp", prefix="dwg"))
try:
    gpg_home = gpg_root / "m"
    gpg_home.mkdir(mode=0o700)
    env = {**os.environ, "GNUPGHOME": str(gpg_home)}
    subprocess.run(["gpg", "--batch", "--passphrase", "", "--quick-generate-key", "Release E2E <e2e@example.com>", "ed25519", "sign", "never"], check=True, env=env, capture_output=True)
    fpr = subprocess.check_output(["gpg", "--batch", "--with-colons", "--list-secret-keys"], env=env, text=True)
    primary_fpr = [line.split(":")[9] for line in fpr.splitlines() if line.startswith("fpr:")][0]
    (wk / "pgp.fpr").write_text(primary_fpr)
    subprocess.run(["gpg", "--batch", "--armor", "--output", str(wk / "release-key.asc"), "--export", primary_fpr], check=True, env=env, capture_output=True)
    subprocess.run(["gpg", "--batch", "--yes", "--armor", "--detach-sign", "--digest-algo", "SHA256", "--local-user", primary_fpr,
                    "--output", str(sigs / "SHA256SUMS.asc"), str(sigs / "SHA256SUMS")], check=True, env=env, capture_output=True)

    # Write valid signing-identities.json for the test
    (wk / "identities.json").write_text(json.dumps({
        "schema": 1,
        "pgp_fingerprint": primary_fpr,
        "apple_team_id": "TESTTEAM01",
        "windows_leaf_sha256": win_leaf_sha,
    }, indent=2) + "\n")

    # Create a tampered SHA256SUMS (still validly PGP-signed) to verify SHA256SUMS mismatch detection
    bad_sums = (sigs / "SHA256SUMS").read_text().replace(sha256_file(unsigned / f"dash-wallet-{ver}-linux-amd64.deb"), "0" * 64)
    (wk / "SHA256SUMS.bad").write_text(bad_sums)
    subprocess.run(["gpg", "--batch", "--yes", "--armor", "--detach-sign", "--digest-algo", "SHA256", "--local-user", primary_fpr,
                    "--output", str(wk / "SHA256SUMS.bad.asc"), str(wk / "SHA256SUMS.bad")], check=True, env=env, capture_output=True)
    subprocess.run(["gpgconf", "--kill", "all"], env=env, capture_output=True)

    # L1: Create a revoked PGP key, sign SHA256SUMS with it, revoke it, and export revoked pubkey
    rev_home = gpg_root / "r"
    rev_home.mkdir(mode=0o700)
    rev_env = {**os.environ, "GNUPGHOME": str(rev_home)}
    subprocess.run(["gpg", "--batch", "--passphrase", "", "--quick-generate-key", "Revoked E2E <rev@example.com>", "ed25519", "sign", "never"], check=True, env=rev_env, capture_output=True)
    rev_out = subprocess.check_output(["gpg", "--batch", "--with-colons", "--list-secret-keys"], env=rev_env, text=True)
    rev_fpr = [line.split(":")[9] for line in rev_out.splitlines() if line.startswith("fpr:")][0]
    (wk / "rev.fpr").write_text(rev_fpr)
    subprocess.run(["gpg", "--batch", "--yes", "--armor", "--detach-sign", "--digest-algo", "SHA256", "--local-user", rev_fpr,
                    "--output", str(wk / "SHA256SUMS.rev.asc"), str(sigs / "SHA256SUMS")], check=True, env=rev_env, capture_output=True)
    rev_cert = rev_home / "openpgp-revocs.d" / f"{rev_fpr}.rev"
    # Strip leading ':' gpg adds before -----BEGIN in auto-generated revoc certs
    rev_text = rev_cert.read_text().replace("\n:-----BEGIN", "\n-----BEGIN")
    if rev_text.startswith(":-----BEGIN"):
        rev_text = rev_text[1:]
    rev_cert.write_text(rev_text)
    subprocess.run(["gpg", "--batch", "--import", str(rev_cert)], check=True, env=rev_env, capture_output=True)
    subprocess.run(["gpg", "--batch", "--armor", "--output", str(wk / "revoked-key.asc"), "--export", rev_fpr], check=True, env=rev_env, capture_output=True)
    subprocess.run(["gpgconf", "--kill", "all"], env=rev_env, capture_output=True)

    # H2: Create an attacker PGP key (simulating a key supplied inside sigs_repo)
    atk_home = gpg_root / "a"
    atk_home.mkdir(mode=0o700)
    atk_env = {**os.environ, "GNUPGHOME": str(atk_home)}
    subprocess.run(["gpg", "--batch", "--passphrase", "", "--quick-generate-key", "Attacker <atk@example.com>", "ed25519", "sign", "never"], check=True, env=atk_env, capture_output=True)
    atk_out = subprocess.check_output(["gpg", "--batch", "--with-colons", "--list-secret-keys"], env=atk_env, text=True)
    atk_fpr = [line.split(":")[9] for line in atk_out.splitlines() if line.startswith("fpr:")][0]
    (wk / "atk.fpr").write_text(atk_fpr)
    subprocess.run(["gpg", "--batch", "--armor", "--output", str(wk / "atk-key.asc"), "--export", atk_fpr], check=True, env=atk_env, capture_output=True)
    subprocess.run(["gpg", "--batch", "--yes", "--armor", "--detach-sign", "--digest-algo", "SHA256", "--local-user", atk_fpr,
                    "--output", str(wk / "SHA256SUMS.atk.asc"), str(sigs / "SHA256SUMS")], check=True, env=atk_env, capture_output=True)
    subprocess.run(["gpgconf", "--kill", "all"], env=atk_env, capture_output=True)
finally:
    shutil.rmtree(gpg_root, ignore_errors=True)
PY

  e2e_fpr=$(cat "$e2e/work/pgp.fpr")
  e2e_rev_fpr=$(cat "$e2e/work/rev.fpr")
  e2e_atk_fpr=$(cat "$e2e/work/atk.fpr")
  e2e_leaf=$(cat "$e2e/work/win.leaf")
  attach_path=$PATH
  if [[ "$(uname -s)" == Darwin ]]; then
    # On real macOS runners (script-tests-macos), stub only Apple-notarization tools for our
    # synthetic .app/.dmg fixtures while exercising REAL /bin/bash 3.2, Python 3.15, Homebrew
    # osslsigncode, gpg, BSD grep, and release-attach.sh's Darwin code paths.
    mac_bin=$e2e/mac-bin
    mkdir -p "$mac_bin"
    cat > "$mac_bin/codesign" <<'CS'
#!/usr/bin/env bash
case "$*" in
  *subject.OU*TESTTEAM01*1.2.840.113635.100.6.1.13*|*1.2.840.113635.100.6.1.13*subject.OU*TESTTEAM01*) exit 0 ;;
  *) echo "stub codesign: missing Developer ID OID + Team ID requirement: $*" >&2; exit 1 ;;
esac
CS
    cat > "$mac_bin/spctl" <<'SP'
#!/usr/bin/env bash
exit 0
SP
    cat > "$mac_bin/xcrun" <<'XC'
#!/usr/bin/env bash
exit 0
XC
    cat > "$mac_bin/hdiutil" <<HD
#!/usr/bin/env bash
if [[ "\$1" == attach ]]; then
  mnt=
  prev=
  for a in "\$@"; do
    [[ "\$prev" == -mountpoint ]] && mnt=\$a
    prev=\$a
  done
  cp -Rp "$e2e/work/signed-app/Dash Wallet.app" "\$mnt/"
  ln -s /Applications "\$mnt/Applications"
fi
exit 0
HD
    chmod +x "$mac_bin/codesign" "$mac_bin/spctl" "$mac_bin/xcrun" "$mac_bin/hdiutil"
    attach_path="$mac_bin:$PATH"
  fi
  attach_base=(
    --version 0.1.0
    --unsigned-dir "$e2e/unsigned"
    --sigs-dir "$e2e/sigs"
    --signed-dmg "$e2e/work/dash-wallet-0.1.0-macos-universal.dmg"
    --repo o/r
    --tauri-conf "$e2e/work/tauri.conf.json"
    --identities-file "$e2e/work/identities.json"
    --pgp-pubkey "$e2e/work/release-key.asc"
    --win-ca-file "$e2e/work/win.crt"
    --skip-win-chain-verify
  )
  if [[ "$(uname -s)" != Darwin ]]; then
    attach_base+=(--allow-non-darwin)
  fi

  attach_rc=0
  PATH="$attach_path" "$here/release-attach.sh" "${attach_base[@]}" --out-dir "$e2e/attached" > "$e2e/attach.out" 2>&1 || attach_rc=$?
  check "release-attach: verifies unsigned hashes, attaches Windows+macOS detached sigs, verifies pinned leaf+minisign+PGP+SHA256SUMS" \
    '[[ $attach_rc -eq 0 && -f "$e2e/attached/signed-manifest.json" ]] && jq -e --arg fpr "$e2e_fpr" --arg leaf "$e2e_leaf" ".pgp_fpr == \$fpr and .apple_team_id == \"TESTTEAM01\" and .windows_leaf_sha256 == \$leaf and (.files | length) == 11" "$e2e/attached/signed-manifest.json" >/dev/null'
  if [[ $attach_rc -ne 0 || ! -f "$e2e/attached/signed-manifest.json" ]]; then
    echo "--- release-attach.sh output (rc=$attach_rc) ---" >&2
    cat "$e2e/attach.out" >&2 || true
    echo "--- end release-attach.sh output ---" >&2
  fi

  # Feed release-attach.sh's output directly into release-publish.sh via REAL workflow_dispatch + DW_SIGNED_DISPATCH=1 (Blocker #1)
  fs_reset
  SIGNED_MANIFEST=$e2e/attached/signed-manifest.json FEED=$e2e/attached/feed DIST=$e2e/attached/dist \
    DW_SIGNED_DISPATCH=1 DW_SIGNING_REQUIRED=1 run workflow_dispatch refs/tags/v0.1.0 "$c2" 0.1.0
  check "Blocker #1: release-attach -> release-publish (real workflow_dispatch + DW_SIGNED_DISPATCH=1): publishes all 11 verified attested assets" \
    '[[ $status -eq 0 && $(ls "$OUT/attest" | wc -l) -eq 11 ]] && log_has "release create v0.1.0 " && log_has "/attest/SHA256SUMS.asc" && log_has "/attest/latest.json"'
  if [[ $status -ne 0 ]]; then
    echo "--- release-publish.sh output (status=$status) ---" >&2
    cat "$work/prep.out" "$work/pub.out" >&2 || true
    echo "--- end release-publish.sh output ---" >&2
  fi

  # Real Tauri CLI (@tauri-apps/cli 2.12.1) minisign fixture: known-good, 1-byte-tampered artifact, and tampered trusted comment
  real_ms_pub=$(cat "$e2e/work/tauri-real-fixture/pubkey.b64")
  real_ms_rc=0
  python3 "$here/verify-minisign.py" --pubkey "$real_ms_pub" \
    --latest-json "$e2e/work/tauri-real-fixture/latest.json" --version 0.1.0 \
    --files-dir "$e2e/work/tauri-real-fixture" --repo o/r > "$e2e/real-ms-ok.out" 2>&1 || real_ms_rc=$?
  check "verify-minisign: accepts real Tauri CLI (@tauri-apps/cli 2.12.1) signed 4-platform fixture" \
    '[[ $real_ms_rc -eq 0 ]] && grep -q "3DD113007F5F85BE" "$e2e/real-ms-ok.out"'

  cp -R "$e2e/work/tauri-real-fixture" "$e2e/work/tauri-real-tampered"
  python3 - "$e2e/work/tauri-real-tampered/dash-wallet-0.1.0-linux-x86_64.AppImage" <<'PY'
import sys
from pathlib import Path
p = Path(sys.argv[1]); b = bytearray(p.read_bytes()); b[0] ^= 0x01; p.write_bytes(bytes(b))
PY
  real_ms_bad_rc=0
  python3 "$here/verify-minisign.py" --pubkey "$real_ms_pub" \
    --latest-json "$e2e/work/tauri-real-tampered/latest.json" --version 0.1.0 \
    --files-dir "$e2e/work/tauri-real-tampered" --repo o/r > "$e2e/real-ms-bad.out" 2>&1 || real_ms_bad_rc=$?
  check "verify-minisign: rejects real Tauri CLI fixture after flipping 1 byte of the artifact" \
    '[[ $real_ms_bad_rc -ne 0 ]] && grep -q "artifact signature verification failed" "$e2e/real-ms-bad.out"'

  real_ms_tc_rc=0
  python3 "$here/verify-minisign.py" --pubkey "$real_ms_pub" \
    --latest-json "$e2e/work/tauri-real-fixture/latest-badtc.json" --version 0.1.0 \
    --files-dir "$e2e/work/tauri-real-fixture" --repo o/r > "$e2e/real-ms-tc.out" 2>&1 || real_ms_tc_rc=$?
  check "verify-minisign: rejects real Tauri CLI fixture when trusted comment is modified" \
    '[[ $real_ms_tc_rc -ne 0 ]] && grep -q "global signature over trusted comment failed" "$e2e/real-ms-tc.out"'

  # Windows CA bundle detection: test comment-only placeholder fixture AND populated real (test) CA bundle fixture,
  # plus adaptive validation of production docs/release/windows-ca.pem so committing the real CA bundle keeps tests green.
  check "Blocker #4: comment-only placeholder Windows CA fixture does not match standalone BEGIN CERTIFICATE delimiter" \
    '! grep -qx -- "-----BEGIN CERTIFICATE-----" "$e2e/work/windows-ca-placeholder.pem"'
  check "Windows CA bundle: populated test CA fixture matches standalone BEGIN CERTIFICATE delimiter" \
    'grep -qx -- "-----BEGIN CERTIFICATE-----" "$e2e/work/windows-ca-populated.pem"'
  pop_ca_rc=0
  PATH="$attach_path" "$here/release-attach.sh" "${attach_base[@]}" \
    --win-ca-file "$e2e/work/windows-ca-populated.pem" --out-dir "$e2e/attached-popca" > "$e2e/attach-popca.out" 2>&1 || pop_ca_rc=$?
  check "Windows CA bundle: release-attach succeeds with populated test CA bundle (comment header + PEM cert)" \
    '[[ $pop_ca_rc -eq 0 && -f "$e2e/attached-popca/signed-manifest.json" ]]'
  if grep -qx -- "-----BEGIN CERTIFICATE-----" "$here/../../docs/release/windows-ca.pem"; then
    check "Windows CA bundle: populated production docs/release/windows-ca.pem parses as a valid X.509 PEM certificate" \
      'grep -qx -- "-----END CERTIFICATE-----" "$here/../../docs/release/windows-ca.pem" && openssl x509 -noout -in "$here/../../docs/release/windows-ca.pem" >/dev/null 2>&1'
  else
    check "Windows CA bundle: unpopulated production docs/release/windows-ca.pem is skipped by workflow standalone delimiter check" \
      '! grep -qx -- "-----BEGIN CERTIFICATE-----" "$here/../../docs/release/windows-ca.pem"'
  fi

  # Helper for negative release-attach.sh checks
  expect_attach_fail() {
    local label=$1 pattern=$2 log=$3; shift 3
    local rc=0
    PATH="$attach_path" "$here/release-attach.sh" "$@" > "$log" 2>&1 || rc=$?
    check "$label" "[[ $rc -ne 0 ]] && grep -qE \"$pattern\" \"$log\""
    if [[ $rc -eq 0 ]] || ! grep -qE "$pattern" "$log"; then
      echo "--- $label (rc=$rc) ---" >&2
      cat "$log" >&2 || true
    fi
  }

  expect_attach_fail \
    "Blocker #4: release-attach refuses comment-only placeholder Windows CA fixture when passed as --win-ca-file" \
    "no valid PEM certificate" "$e2e/attach-badca.out" \
    "${attach_base[@]}" --win-ca-file "$e2e/work/windows-ca-placeholder.pem" --out-dir "$e2e/attached-badca"

  expect_attach_fail \
    "Blocker #6: verify-minisign refuses retired CI updater key ED322B963B527B21" \
    "refusing retired updater key .*ED322B963B527B21" "$e2e/attach-retired-ms.out" \
    "${attach_base[@]}" --tauri-conf "$e2e/work/tauri-retired.conf.json" --out-dir "$e2e/attached-retired-ms"

  expect_attach_fail \
    "verify-minisign refuses retired CI Ed25519 pubkey bytes even when relabelled with a new key ID" \
    "refusing retired updater key .*7821a52f7420d388" "$e2e/attach-relabelled-ms.out" \
    "${attach_base[@]}" --tauri-conf "$e2e/work/tauri-relabelled-retired.conf.json" --out-dir "$e2e/attached-relabelled-ms"

  expect_attach_fail \
    "L2: verify-minisign refuses torsion-shifted retired CI Ed25519 pubkey (A' = A + T8)" \
    "non-prime-order" "$e2e/attach-torsion-ms.out" \
    "${attach_base[@]}" --tauri-conf "$e2e/work/tauri-torsion-retired.conf.json" --out-dir "$e2e/attached-torsion-ms"

  norepo_rc=0
  python3 "$here/verify-minisign.py" --pubkey "$real_ms_pub" \
    --latest-json "$e2e/work/tauri-real-fixture/latest.json" --version 0.1.0 \
    --files-dir "$e2e/work/tauri-real-fixture" > "$e2e/real-ms-norepo.out" 2>&1 || norepo_rc=$?
  check "N3: verify-minisign requires --repo when verifying latest.json" \
    '[[ $norepo_rc -ne 0 ]] && grep -q -- "--repo" "$e2e/real-ms-norepo.out"'

  # Unsigned input tampering
  cp -R "$e2e/unsigned" "$e2e/unsigned-bad"
  echo "mutated" >> "$e2e/unsigned-bad/dash-wallet-0.1.0-linux-amd64.deb"
  expect_attach_fail \
    "release-attach: refuses to attach signatures when unsigned CI build does not match unsigned.SHA256SUMS" \
    "FAILED|unsigned\.SHA256SUMS" "$e2e/attach-bad.out" \
    "${attach_base[@]}" --unsigned-dir "$e2e/unsigned-bad" --out-dir "$e2e/attached-bad"

  # Wrong PGP fingerprint
  expect_attach_fail \
    "release-attach: refuses SHA256SUMS.asc signed by a non-matching PGP primary fingerprint" \
    "expected 0000000000000000000000000000000000000000" "$e2e/attach-bad-pgp.out" \
    "${attach_base[@]}" --pgp-fingerprint 0000000000000000000000000000000000000000 --out-dir "$e2e/attached-bad-pgp"

  # N4: Off Darwin, release-attach.sh fails closed unless --allow-non-darwin is explicitly passed
  if [[ "$(uname -s)" != Darwin ]]; then
    expect_attach_fail \
      "N4: release-attach refuses non-Darwin host unless --allow-non-darwin is given" \
      "require macOS|--allow-non-darwin" "$e2e/attach-nondarwin.out" \
      --version 0.1.0 --unsigned-dir "$e2e/unsigned" --sigs-dir "$e2e/sigs" \
      --out-dir "$e2e/attached-nondarwin" --signed-dmg "$e2e/work/dash-wallet-0.1.0-macos-universal.dmg" \
      --repo o/r --tauri-conf "$e2e/work/tauri.conf.json" \
      --identities-file "$e2e/work/identities.json" --pgp-pubkey "$e2e/work/release-key.asc" \
      --win-ca-file "$e2e/work/win.crt" --skip-win-chain-verify
  fi

  # N5: Dropping a line from sigs/unsigned.SHA256SUMS (would pass sha256sum -c alone) is refused by byte-for-byte cmp
  cp -R "$e2e/sigs" "$e2e/sigs-trunc-unsigned"
  head -n 2 "$e2e/sigs/unsigned.SHA256SUMS" > "$e2e/sigs-trunc-unsigned/unsigned.SHA256SUMS"
  expect_attach_fail \
    "N5: release-attach refuses sigs/unsigned.SHA256SUMS that differs byte-for-byte from CI's unsigned.SHA256SUMS" \
    "differs from CI" "$e2e/attach-trunc-unsigned.out" \
    "${attach_base[@]}" --sigs-dir "$e2e/sigs-trunc-unsigned" --out-dir "$e2e/attached-trunc-unsigned"

  # H2 / Decision 5 / r4687084401: test-owned placeholder signing-identities.json fails closed (independent of production repo file),
  # and validate production docs/release/signing-identities.json adaptively whether placeholder or populated.
  expect_attach_fail \
    "H2/Decision 5: release-attach fails closed on placeholder signing-identities.json fixture (no fingerprint)" \
    "pinned PGP primary fingerprint" "$e2e/attach-nofpr.out" \
    --version 0.1.0 --unsigned-dir "$e2e/unsigned" --sigs-dir "$e2e/sigs" \
    --identities-file "$e2e/work/identities-placeholder.json" \
    --out-dir "$e2e/attached-nofpr" --signed-dmg "$e2e/work/dash-wallet-0.1.0-macos-universal.dmg" \
    --allow-non-darwin
  check "signing-identities.json: production repo file is valid schema 1 (placeholder or properly formatted real identities)" \
    'jq -e ".schema == 1 and ((.pgp_fingerprint | startswith(\"REPLACE_\")) or (.pgp_fingerprint | test(\"^[0-9a-fA-F]{40}$\"))) and ((.apple_team_id | startswith(\"REPLACE_\")) or (.apple_team_id | test(\"^[A-Z0-9]{10}$\"))) and ((.windows_leaf_sha256 | contains(\"REPLACE_\")) or (.windows_leaf_sha256 | test(\"^sha256:[0-9a-fA-F]{64}$\")))" "$here/../../docs/release/signing-identities.json" >/dev/null'

  # H2: missing SHA256SUMS.asc at attach time
  cp -R "$e2e/sigs" "$e2e/sigs-noasc"
  rm -f "$e2e/sigs-noasc/SHA256SUMS.asc"
  expect_attach_fail \
    "H2: release-attach refuses missing SHA256SUMS.asc" \
    "missing .*SHA256SUMS\.asc" "$e2e/attach-noasc.out" \
    "${attach_base[@]}" --sigs-dir "$e2e/sigs-noasc" --out-dir "$e2e/attached-noasc"

  # H2: SHA256SUMS.asc signed by attacker key supplied inside sigs repo is refused (both path check and key check)
  cp -R "$e2e/sigs" "$e2e/sigs-atk"
  cp "$e2e/work/atk-key.asc" "$e2e/sigs-atk/release-key.asc"
  cp "$e2e/work/SHA256SUMS.atk.asc" "$e2e/sigs-atk/SHA256SUMS.asc"
  expect_attach_fail \
    "H2: release-attach refuses PGP public key loaded from inside --sigs-dir" \
    "refusing PGP public key loaded from inside --sigs-dir" "$e2e/attach-atk-path.out" \
    "${attach_base[@]}" --sigs-dir "$e2e/sigs-atk" --pgp-pubkey "$e2e/sigs-atk/release-key.asc" \
    --pgp-fingerprint "$e2e_atk_fpr" --out-dir "$e2e/attached-atk-path"
  expect_attach_fail \
    "H2: release-attach refuses SHA256SUMS.asc signed by attacker key when pinned to repo key" \
    "gpg --verify failed|expected" "$e2e/attach-atk-key.out" \
    "${attach_base[@]}" --sigs-dir "$e2e/sigs-atk" --out-dir "$e2e/attached-atk-key"

  # L1: revoked PGP key is refused even though gpg exits 0 with VALIDSIG
  cp -R "$e2e/sigs" "$e2e/sigs-rev"
  cp "$e2e/work/SHA256SUMS.rev.asc" "$e2e/sigs-rev/SHA256SUMS.asc"
  expect_attach_fail \
    "L1: release-attach refuses SHA256SUMS.asc signed by a revoked PGP key" \
    "revoked, expired, or bad" "$e2e/attach-rev.out" \
    "${attach_base[@]}" --sigs-dir "$e2e/sigs-rev" --pgp-pubkey "$e2e/work/revoked-key.asc" \
    --pgp-fingerprint "$e2e_rev_fpr" --out-dir "$e2e/attached-rev"

  # H1: symlink inside sigs dir refused by release-attach.sh AND macos-detached-sig.py apply
  cp -R "$e2e/sigs" "$e2e/sigs-sym"
  ln -s /etc/hosts "$e2e/sigs-sym/osx/Dash Wallet.app/Contents/evil-link"
  expect_attach_fail \
    "H1: release-attach refuses sigs directory containing symlinks" \
    "refusing sigs directory containing symlinks" "$e2e/attach-sym.out" \
    "${attach_base[@]}" --sigs-dir "$e2e/sigs-sym" --out-dir "$e2e/attached-sym"

  sym_py_rc=0
  python3 "$here/macos-detached-sig.py" apply "$e2e/work/unsigned-app/Dash Wallet.app" \
    "$e2e/sigs-sym/osx/Dash Wallet.app" "$e2e/work/out-sym.app" > "$e2e/sym-py.out" 2>&1 || sym_py_rc=$?
  check "H1: macos-detached-sig.py apply refuses symlinks in sigs directory" \
    '[[ $sym_py_rc -ne 0 ]] && grep -qF "refusing symlink" "$e2e/sym-py.out"'

  # H1: path traversal in bundle_sig_files[].path AND L2 untrusted cs_file refused by macos-detached-sig.py apply
  cp -R "$e2e/sigs/osx/Dash Wallet.app" "$e2e/work/sigs-trav"
  jq '.bundle_sig_files[0].path = "Contents/L/../../escaped-victim.py"' "$e2e/sigs/osx/Dash Wallet.app/manifest.json" > "$e2e/work/sigs-trav/manifest.json"
  trav_rc=0
  python3 "$here/macos-detached-sig.py" apply "$e2e/work/unsigned-app/Dash Wallet.app" \
    "$e2e/work/sigs-trav" "$e2e/work/out-trav.app" > "$e2e/trav.out" 2>&1 || trav_rc=$?
  check "H1: macos-detached-sig.py apply refuses bundle_sig_files path traversal outside allowlist" \
    '[[ $trav_rc -ne 0 && ! -e "$e2e/work/escaped-victim.py" ]] && grep -qF "outside allowlist" "$e2e/trav.out"'

  cp -R "$e2e/sigs/osx/Dash Wallet.app" "$e2e/work/sigs-cs"
  jq '.macho_binaries[0].slices[0].cs_file = "../evil.cs"' "$e2e/sigs/osx/Dash Wallet.app/manifest.json" > "$e2e/work/sigs-cs/manifest.json"
  cs_rc=0
  python3 "$here/macos-detached-sig.py" apply "$e2e/work/unsigned-app/Dash Wallet.app" \
    "$e2e/work/sigs-cs" "$e2e/work/out-cs.app" > "$e2e/cs.out" 2>&1 || cs_rc=$?
  check "H1/L2: macos-detached-sig.py apply refuses non-canonical cs_file path" \
    '[[ $cs_rc -ne 0 ]] && grep -qF "Untrusted cs_file" "$e2e/cs.out"'

  # r4687084381: failed extract preserves previously valid sigs_dir intact (atomic temp-dir swap)
  cp -R "$e2e/sigs/osx/Dash Wallet.app" "$e2e/work/sigs-preserve"
  printf 'keep-prior-sigs\n' > "$e2e/work/sigs-preserve/SENTINEL"
  ext_fail_rc=0
  python3 "$here/macos-detached-sig.py" extract "$e2e/work/unsigned-app/Dash Wallet.app" \
    "$e2e/work/tampered-app/Dash Wallet.app" "$e2e/work/sigs-preserve" > "$e2e/ext-preserve.out" 2>&1 || ext_fail_rc=$?
  check "r4687084381: failed macos-detached-sig.py extract preserves previous sigs_dir intact" \
    '[[ $ext_fail_rc -ne 0 && "$(cat "$e2e/work/sigs-preserve/SENTINEL")" == "keep-prior-sigs" ]]'

  # r4687084393: apply refuses overlapping out_app (same as or inside unsigned_app) and preserves prior out_app on error
  cp -R "$e2e/work/unsigned-app/Dash Wallet.app" "$e2e/work/overlap-app.app"
  ov_rc=0
  python3 "$here/macos-detached-sig.py" apply "$e2e/work/overlap-app.app" \
    "$e2e/sigs/osx/Dash Wallet.app" "$e2e/work/overlap-app.app" > "$e2e/ov.out" 2>&1 || ov_rc=$?
  check "r4687084393: macos-detached-sig.py apply refuses out_app overlapping unsigned_app without deleting input" \
    '[[ $ov_rc -ne 0 && -f "$e2e/work/overlap-app.app/Contents/MacOS/dash-wallet" ]] && grep -q "overlapping" "$e2e/ov.out"'
  ov_in_rc=0
  python3 "$here/macos-detached-sig.py" apply "$e2e/work/overlap-app.app" \
    "$e2e/sigs/osx/Dash Wallet.app" "$e2e/work/overlap-app.app/sub.app" > "$e2e/ov-in.out" 2>&1 || ov_in_rc=$?
  check "r4687084393: macos-detached-sig.py apply refuses out_app nested inside unsigned_app" \
    '[[ $ov_in_rc -ne 0 ]] && grep -q "overlapping" "$e2e/ov-in.out"'
  mkdir -p "$e2e/work/prior-out.app"
  printf 'keep-prior-app\n' > "$e2e/work/prior-out.app/SENTINEL"
  app_fail_rc=0
  python3 "$here/macos-detached-sig.py" apply "$e2e/work/unsigned-app/Dash Wallet.app" \
    "$e2e/work/sigs-cs" "$e2e/work/prior-out.app" > "$e2e/app-preserve.out" 2>&1 || app_fail_rc=$?
  check "r4687084393: failed macos-detached-sig.py apply preserves prior out_app intact" \
    '[[ $app_fail_rc -ne 0 && "$(cat "$e2e/work/prior-out.app/SENTINEL")" == "keep-prior-app" ]]'

  # H2/M4: verify-dmg-layout accepts exact {Dash Wallet.app, Applications -> /Applications} and refuses extras/mode tamper
  mkdir -p "$e2e/work/dmg-ok"
  cp -R "$e2e/work/signed-app/Dash Wallet.app" "$e2e/work/dmg-ok/"
  ln -s /Applications "$e2e/work/dmg-ok/Applications"
  dmg_ok_rc=0
  python3 "$here/macos-detached-sig.py" verify-dmg-layout "$e2e/work/dmg-ok" "$e2e/work/signed-app/Dash Wallet.app" > "$e2e/dmg-ok.out" 2>&1 || dmg_ok_rc=$?
  check "H2/M4: verify-dmg-layout accepts exact {Dash Wallet.app, Applications -> /Applications}" \
    '[[ $dmg_ok_rc -eq 0 ]]'
  touch "$e2e/work/dmg-ok/evil.command"
  dmg_extra_rc=0
  python3 "$here/macos-detached-sig.py" verify-dmg-layout "$e2e/work/dmg-ok" "$e2e/work/signed-app/Dash Wallet.app" > "$e2e/dmg-extra.out" 2>&1 || dmg_extra_rc=$?
  check "H2: verify-dmg-layout refuses extra top-level file in .dmg" \
    '[[ $dmg_extra_rc -ne 0 ]] && grep -qF "evil.command" "$e2e/dmg-extra.out"'
  rm -f "$e2e/work/dmg-ok/evil.command" "$e2e/work/dmg-ok/Applications"
  ln -s /tmp "$e2e/work/dmg-ok/Applications"
  dmg_link_rc=0
  python3 "$here/macos-detached-sig.py" verify-dmg-layout "$e2e/work/dmg-ok" "$e2e/work/signed-app/Dash Wallet.app" > "$e2e/dmg-link.out" 2>&1 || dmg_link_rc=$?
  check "H2: verify-dmg-layout refuses Applications symlink not pointing to /Applications" \
    '[[ $dmg_link_rc -ne 0 ]] && grep -qF "/Applications" "$e2e/dmg-link.out"'

  # M4: wrong Windows leaf hash AND Windows .pem taken from a different executable
  expect_attach_fail \
    "M4: release-attach refuses Windows binary whose leaf cert does not match pinned windows_leaf_sha256" \
    "Failed|leaf|hash|mismatch" "$e2e/attach-badleaf.out" \
    "${attach_base[@]}" --win-leaf-hash "sha256:0000000000000000000000000000000000000000000000000000000000000000" --out-dir "$e2e/attached-badleaf"

  cp -R "$e2e/sigs" "$e2e/sigs-otherwin"
  cp "$e2e/work/other.exe.pem" "$e2e/sigs-otherwin/win/dash-wallet-0.1.0-windows-x64-setup.exe.pem"
  expect_attach_fail \
    "M4: release-attach refuses Windows .pem detached signature taken from another executable" \
    "Failed|mismatch|digest|invalid" "$e2e/attach-otherwin.out" \
    "${attach_base[@]}" --sigs-dir "$e2e/sigs-otherwin" --out-dir "$e2e/attached-otherwin"

  # Minisign negative tests (wrong key, wrong version in trusted comment, L6 wrong platform URL)
  for case_spec in \
    "badkey:latest-badkey.json:verification failed|key ID" \
    "badver:latest-badver.json:trusted comment version" \
    "badurl:latest-badurl.json:!= expected"; do
    IFS=: read -r cname cfile cpat <<<"$case_spec"
    cp -R "$e2e/sigs" "$e2e/sigs-$cname"
    cp "$e2e/work/$cfile" "$e2e/sigs-$cname/updater/latest.json"
    expect_attach_fail \
      "verify-minisign ($cname): release-attach refuses invalid updater feed" \
      "$cpat" "$e2e/attach-$cname.out" \
      "${attach_base[@]}" --sigs-dir "$e2e/sigs-$cname" --out-dir "$e2e/attached-$cname"
  done

  # SHA256SUMS mismatch at attach (validly PGP-signed SHA256SUMS with a tampered asset hash)
  cp -R "$e2e/sigs" "$e2e/sigs-badsums"
  cp "$e2e/work/SHA256SUMS.bad" "$e2e/sigs-badsums/SHA256SUMS"
  cp "$e2e/work/SHA256SUMS.bad.asc" "$e2e/sigs-badsums/SHA256SUMS.asc"
  expect_attach_fail \
    "release-attach: refuses SHA256SUMS mismatch for a final asset" \
    "SHA-256 mismatch" "$e2e/attach-badsums.out" \
    "${attach_base[@]}" --sigs-dir "$e2e/sigs-badsums" --out-dir "$e2e/attached-badsums"

  # Blocker #7: ci/release/fetch-unsigned.sh pins locally verified --tag-sha, verifies run + attestations + checksums before exposing files
  fu=$e2e/fetch-unsigned-test
  mkdir -p "$fu/bin"
  cat > "$fu/bin/gh" <<'FGH'
#!/usr/bin/env bash
set -euo pipefail
case "$1 ${2:-}" in
  "api repos/"*"/commits/"*)
    echo "${FU_REMOTE_SHA}"
    ;;
  "api repos/"*"/actions/runs/"*)
    printf '{"path":".github/workflows/release.yml","event":"push","head_branch":"%s","head_sha":"%s","head_repository":{"full_name":"o/r"},"conclusion":"success"}\n' \
      "${FU_RUN_BRANCH:-v0.1.0}" "${FU_RUN_SHA:-$FU_REMOTE_SHA}"
    ;;
  "run download")
    dest=
    prev=
    for a in "$@"; do
      [[ "$prev" == -D ]] && dest=$a
      prev=$a
    done
    cp "$FU_SRC_DIR"/* "$dest/"
    ;;
  "attestation verify")
    [[ -z "${FU_FAIL_ATTEST:-}" ]] || { echo "attestation mismatch" >&2; exit 1; }
    echo "ok $3"
    ;;
  *)
    echo "unexpected fake gh call: $*" >&2
    exit 1
    ;;
esac
FGH
  chmod +x "$fu/bin/gh"
  fu_sha=1111111111111111111111111111111111111111
  fu_moved=2222222222222222222222222222222222222222
  fu_rc=0
  PATH="$fu/bin:$PATH" FU_SRC_DIR="$e2e/unsigned" FU_REMOTE_SHA="$fu_sha" \
    "$here/../release/fetch-unsigned.sh" --tag v0.1.0 --tag-sha "$fu_sha" --run-id 555 --repo o/r \
    --unsigned-dir "$fu/out-ok" --sigs-dir "$fu/sigs-ok" > "$fu/ok.out" 2>&1 || fu_rc=$?
  check "Blocker #7: fetch-unsigned.sh verifies tag SHA, run metadata, attestations, and unsigned.SHA256SUMS" \
    '[[ $fu_rc -eq 0 && -f "$fu/out-ok/unsigned.SHA256SUMS" && -f "$fu/sigs-ok/unsigned.SHA256SUMS" ]]'
  fu_rc=0
  PATH="$fu/bin:$PATH" FU_SRC_DIR="$e2e/unsigned" FU_REMOTE_SHA="$fu_moved" \
    "$here/../release/fetch-unsigned.sh" --tag v0.1.0 --tag-sha "$fu_sha" --run-id 555 --repo o/r \
    --unsigned-dir "$fu/out-moved" --sigs-dir "$fu/sigs-moved" > "$fu/moved.out" 2>&1 || fu_rc=$?
  check "Blocker #7: fetch-unsigned.sh refuses moved remote tag before exposing any files" \
    '[[ $fu_rc -ne 0 && ! -d "$fu/out-moved" ]] && grep -q "tag moved" "$fu/moved.out"'
  fu_rc=0
  PATH="$fu/bin:$PATH" FU_SRC_DIR="$e2e/unsigned" FU_REMOTE_SHA="$fu_sha" FU_RUN_SHA="$fu_moved" \
    "$here/../release/fetch-unsigned.sh" --tag v0.1.0 --tag-sha "$fu_sha" --run-id 555 --repo o/r \
    --unsigned-dir "$fu/out-badrun" --sigs-dir "$fu/sigs-badrun" > "$fu/badrun.out" 2>&1 || fu_rc=$?
  check "Blocker #7: fetch-unsigned.sh refuses Actions run built from a different commit" \
    '[[ $fu_rc -ne 0 && ! -d "$fu/out-badrun" ]] && grep -q "expected locally verified" "$fu/badrun.out"'
  fu_rc=0
  PATH="$fu/bin:$PATH" FU_SRC_DIR="$e2e/unsigned" FU_REMOTE_SHA="$fu_sha" FU_FAIL_ATTEST=1 \
    "$here/../release/fetch-unsigned.sh" --tag v0.1.0 --tag-sha "$fu_sha" --run-id 555 --repo o/r \
    --unsigned-dir "$fu/out-badatt" --sigs-dir "$fu/sigs-badatt" > "$fu/badatt.out" 2>&1 || fu_rc=$?
  check "Blocker #7: fetch-unsigned.sh refuses attestation failure without exposing staged files" \
    '[[ $fu_rc -ne 0 && ! -d "$fu/out-badatt" ]]'

  # Safety: fetch-unsigned.sh refuses non-empty --unsigned-dir / --sigs-dir or symlinks, leaving existing files untouched
  mkdir -p "$fu/reused-unsigned"
  printf 'precious-user-file\n' > "$fu/reused-unsigned/keep-me.txt"
  fu_rc=0
  PATH="$fu/bin:$PATH" FU_SRC_DIR="$e2e/unsigned" FU_REMOTE_SHA="$fu_sha" \
    "$here/../release/fetch-unsigned.sh" --tag v0.1.0 --tag-sha "$fu_sha" --run-id 555 --repo o/r \
    --unsigned-dir "$fu/reused-unsigned" --sigs-dir "$fu/sigs-reused1" > "$fu/reused-u.out" 2>&1 || fu_rc=$?
  check "fetch-unsigned.sh refuses non-empty --unsigned-dir and leaves unrelated files untouched" \
    '[[ $fu_rc -ne 0 && "$(cat "$fu/reused-unsigned/keep-me.txt")" == "precious-user-file" && ! -d "$fu/sigs-reused1" ]] && grep -q "empty directory" "$fu/reused-u.out"'

  mkdir -p "$fu/reused-sigs"
  printf 'victim-original\n' > "$fu/victim.txt"
  ln -s "$fu/victim.txt" "$fu/reused-sigs/unsigned.SHA256SUMS"
  fu_rc=0
  PATH="$fu/bin:$PATH" FU_SRC_DIR="$e2e/unsigned" FU_REMOTE_SHA="$fu_sha" \
    "$here/../release/fetch-unsigned.sh" --tag v0.1.0 --tag-sha "$fu_sha" --run-id 555 --repo o/r \
    --unsigned-dir "$fu/out-reused2" --sigs-dir "$fu/reused-sigs" > "$fu/reused-s.out" 2>&1 || fu_rc=$?
  check "fetch-unsigned.sh refuses non-empty --sigs-dir with symlink and leaves target file untouched" \
    '[[ $fu_rc -ne 0 && "$(cat "$fu/victim.txt")" == "victim-original" && ! -d "$fu/out-reused2" ]] && grep -q "empty directory" "$fu/reused-s.out"'

  mkdir -p "$fu/sym-target"
  ln -s "$fu/sym-target" "$fu/sym-unsigned"
  fu_rc=0
  PATH="$fu/bin:$PATH" FU_SRC_DIR="$e2e/unsigned" FU_REMOTE_SHA="$fu_sha" \
    "$here/../release/fetch-unsigned.sh" --tag v0.1.0 --tag-sha "$fu_sha" --run-id 555 --repo o/r \
    --unsigned-dir "$fu/sym-unsigned" --sigs-dir "$fu/sigs-sym" > "$fu/sym-u.out" 2>&1 || fu_rc=$?
  check "fetch-unsigned.sh refuses symlink --unsigned-dir" \
    '[[ $fu_rc -ne 0 && -z "$(ls -A "$fu/sym-target")" ]] && grep -q "must not be a symlink" "$fu/sym-u.out"'

  # L3: fetch-unsigned.sh refuses group- or world-writable existing empty output dirs
  mkdir -p "$fu/gw-unsigned" "$fu/ww-sigs"
  chmod 775 "$fu/gw-unsigned"
  chmod 777 "$fu/ww-sigs"
  fu_rc=0
  PATH="$fu/bin:$PATH" FU_SRC_DIR="$e2e/unsigned" FU_REMOTE_SHA="$fu_sha" \
    "$here/../release/fetch-unsigned.sh" --tag v0.1.0 --tag-sha "$fu_sha" --run-id 555 --repo o/r \
    --unsigned-dir "$fu/gw-unsigned" --sigs-dir "$fu/sigs-gw" > "$fu/gw.out" 2>&1 || fu_rc=$?
  check "L3: fetch-unsigned.sh refuses group-writable existing --unsigned-dir" \
    '[[ $fu_rc -ne 0 && -z "$(ls -A "$fu/gw-unsigned")" ]] && grep -q "group- or world-writable" "$fu/gw.out"'
  fu_rc=0
  PATH="$fu/bin:$PATH" FU_SRC_DIR="$e2e/unsigned" FU_REMOTE_SHA="$fu_sha" \
    "$here/../release/fetch-unsigned.sh" --tag v0.1.0 --tag-sha "$fu_sha" --run-id 555 --repo o/r \
    --unsigned-dir "$fu/out-ww" --sigs-dir "$fu/ww-sigs" > "$fu/ww.out" 2>&1 || fu_rc=$?
  check "L3: fetch-unsigned.sh refuses world-writable existing --sigs-dir" \
    '[[ $fu_rc -ne 0 && ! -d "$fu/out-ww" ]] && grep -q "group- or world-writable" "$fu/ww.out"'

  # L3: Local signing scripts re-verify unsigned inputs against unsigned.SHA256SUMS before signing
  l3=$e2e/l3-tamper
  mkdir -p "$l3/unsigned" "$l3/sigs" "$l3/dist" "$l3/feed" "$l3/out"
  cp "$e2e/unsigned"/* "$l3/unsigned/"
  cp "$e2e/unsigned/unsigned.SHA256SUMS" "$l3/sigs/unsigned.SHA256SUMS"
  cp "$e2e/unsigned"/* "$l3/dist/"
  cp "$e2e/work/dash-wallet-0.1.0-macos-universal.app.tar.gz" "$l3/feed/"
  printf 'tampered' >> "$l3/unsigned/dash-wallet-0.1.0-windows-x64-setup.exe"
  printf 'tampered' >> "$l3/dist/dash-wallet-0.1.0-linux-x86_64.AppImage"
  touch "$l3/dummy-mod" "$l3/dummy-key"
  sw_rc=0
  "$here/../release/sign-windows.sh" --version 0.1.0 --unsigned-dir "$l3/unsigned" --sigs-dir "$l3/sigs" --out-dir "$l3/out" \
    --pkcs11-module "$l3/dummy-mod" --pkcs11-cert "pkcs11:id=%01" --pkcs11-key "pkcs11:id=%01" > "$l3/sw.out" 2>&1 || sw_rc=$?
  check "L3: sign-windows.sh refuses unsigned .exe modified since fetch before invoking osslsigncode" \
    '[[ $sw_rc -ne 0 ]] && grep -q "changed since fetch" "$l3/sw.out"'
  su_rc=0
  TAURI_SIGNING_PRIVATE_KEY_PASSWORD="" "$here/../release/sign-updater.sh" --version 0.1.0 --dist-dir "$l3/dist" \
    --feed-dir "$l3/feed" --sigs-dir "$l3/sigs" --key-file "$l3/dummy-key" > "$l3/su.out" 2>&1 || su_rc=$?
  check "L3: sign-updater.sh refuses Linux AppImage modified since fetch before invoking tauri signer" \
    '[[ $su_rc -ne 0 ]] && grep -q "changed since fetch" "$l3/su.out"'
  sc_rc=0
  "$here/../release/sign-checksums.sh" --version 0.1.0 --dist-dir "$l3/dist" --feed-dir "$l3/feed" \
    --sigs-dir "$l3/sigs" --gpg-key "$e2e_fpr" > "$l3/sc.out" 2>&1 || sc_rc=$?
  check "L3: sign-checksums.sh refuses unchanged CI artifact modified since fetch before PGP signing" \
    '[[ $sc_rc -ne 0 ]] && grep -q "changed since fetch" "$l3/sc.out"'
  if [[ "$(uname -s)" == Darwin ]]; then
    printf 'tampered' >> "$l3/unsigned/dash-wallet-0.1.0-macos-unsigned.tar.gz"
    sm_rc=0
    "$here/../release/sign-macos.sh" --version 0.1.0 --unsigned-dir "$l3/unsigned" --sigs-dir "$l3/sigs" --out-dir "$l3/out" \
      --identity "Developer ID Application: Test" --notary-profile "test" > "$l3/sm.out" 2>&1 || sm_rc=$?
    check "L3: sign-macos.sh refuses macos-unsigned.tar.gz modified since fetch before invoking codesign" \
      '[[ $sm_rc -ne 0 ]] && grep -q "changed since fetch" "$l3/sm.out"'
  fi
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
