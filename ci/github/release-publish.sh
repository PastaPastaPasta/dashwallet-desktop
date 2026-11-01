#!/usr/bin/env bash
# GitHub Actions (release.yml, publish job): turn a run's artifacts into GitHub releases.
#
#   ci/github/release-publish.sh classify  print main, tag or skip for this trigger (needs only the event and ref)
#   ci/github/release-publish.sh prepare   check what this run publishes, stage and copy the files
#   ci/github/release-publish.sh publish   create / update the releases
#
# Needs: DIST (the downloaded dist-* artifacts and their SHA256SUMS), VERSION (release-version.sh),
# GITHUB_EVENT_NAME, GITHUB_REF, GITHUB_SHA, GITHUB_REPOSITORY, GITHUB_RUN_NUMBER, GITHUB_RUN_ID,
# GITHUB_SERVER_URL, GH_TOKEN, and a checkout whose `origin/main` is fetched with full history.
# OUT defaults to $DIST/../publish. DRY_RUN=1 runs the reads (release and tag lookups) and prints each
# write instead of doing it, so the logic can be run by hand against the real repository.
#
# What publishes (the only two cases; everything else exits before it touches anything: 3 for a tag
# that is not a release tag, 1 for the rest):
#   push to refs/heads/main      an immutable prerelease v<version>-dev.<run>+<short sha> with the run's
#                                files, and the rolling prerelease `continuous` with stable names;
#   push of a tag vX.Y.Z[-rc.N]  that tag's release (a prerelease: every 0.x and every -rc is one),
#                                notes from docs/release/<X.Y.Z>-notes.md when it exists.
# A commit that is not on origin/main never publishes, whatever the trigger or the tag says.
#
# `continuous` is replaced as one generation, with the previous one kept until everything has worked:
#   1. recover from an interrupted earlier run (leftover <name>.new / <name>.old assets, a canonical name
#      missing, binaries that don't match SHA256SUMS): finish or roll back, never delete the only copy;
#   2. upload every new file as <name>.new (a failure here changes nothing a user sees);
#   3. for each name, SHA256SUMS last: rename the old asset <name>.old, then the new one <name>;
#   4. move the tag and rewrite the notes;
#   5. only then delete the <name>.old assets.
# Any failure in 3 or 4, and SIGINT / SIGTERM (a cancelled run), roll the whole generation back: every
# canonical name is restored from its .old, so the end state is the old generation, complete.
# The Releases API is not atomic. While step 3 runs (a few seconds per asset, tens of seconds in all) a
# download of one name can fail for the two calls between its renames, and bundles can be the new build
# while SHA256SUMS is still the old one: check a download against the SHA256SUMS you fetch at the same
# time, and retry on a mismatch. A hard kill or an API outage during 3-5 can leave canonical names missing
# or mixed until the next run, whose step 1 repairs it (by hand: rename each <name>.old over the missing or
# new <name>, delete stray <name>.new, or re-run the publish job). The tag moves after the assets and never
# backwards: if the current `continuous` commit is a descendant of this run's, the run publishes its
# immutable release and leaves `continuous` alone. Every read is checked: a failed listing stops the script
# before any write (an API error is never read as "nothing there").
#
# GitHub keeps one pending job per concurrency group and cancels the older pending one when another
# arrives. The workflow's group is per ref (main has one, each tag its own), so a tag's release is
# never dropped for a main push. On main a burst of pushes can cancel the pending publish of a middle
# commit (no immutable release for it), and builds finish out of order, so `continuous` can end one
# commit behind main until the next push publishes. It never goes backwards.
set -euo pipefail

mode=${1:?usage: $0 classify|prepare|publish}
event=${GITHUB_EVENT_NAME:?}
ref=${GITHUB_REF:?}

# --- what this trigger publishes ----------------------------------------------------------------
kind=skip
if [[ "$event" == push ]]; then
  if [[ "$ref" == refs/heads/main ]]; then
    kind=main
  elif [[ "$ref" =~ ^refs/tags/v[0-9]+\.[0-9]+\.[0-9]+(-rc\.[0-9]+)?$ ]]; then
    kind=tag
  fi
fi
if [[ "$mode" == classify ]]; then echo "$kind"; exit 0; fi

: "${DIST:?set DIST}" "${VERSION:?set VERSION}" "${GITHUB_SHA:?}" "${GITHUB_REPOSITORY:?}"
repo=$GITHUB_REPOSITORY
sha=$GITHUB_SHA
out=${OUT:-$DIST/../publish}
dry=${DRY_RUN:-}
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

die() { echo "release-publish: $*" >&2; exit 1; }
# A write: printed in a dry run, done otherwise.
w() { if [[ -n "$dry" ]]; then printf 'DRY-RUN:'; printf ' %q' "$@"; echo; else "$@"; fi; }

# A read of one thing that may legitimately be absent: returns 0 with the output in $PROBE_OUT, 44 on an
# HTTP 404, and ends the script on any other error (a rate limit or a 5xx is not "absent").
PROBE_OUT=
probe() {
  local err rc=0
  err=$(mktemp)
  PROBE_OUT=$("$@" 2>"$err") || rc=$?
  if ((rc == 0)); then rm -f "$err"; return 0; fi
  if grep -qiE 'HTTP 404' "$err"; then rm -f "$err"; return 44; fi
  cat "$err" >&2; rm -f "$err"
  die "failed: $*"
}
# The release of a tag, from one checked listing of every release (drafts included; gh release view's
# "not found" can hide a failed half of its lookup). Sets ROW to "tag<TAB>id<TAB>draft<TAB>assets", or empty.
ROW=
release_row() {
  local rows
  rows=$(gh api "repos/$repo/releases" --paginate --jq '.[]|[.tag_name,.id,.draft,(.assets|length)]|@tsv') ||
    die "cannot list the releases of $repo"
  ROW=$(awk -F'\t' -v t="$1" '$1==t{print; exit}' <<<"$rows")
}
# A release that exists must be complete: published, with every file. Returns 1 when there is none.
complete_release() {
  local tag=$1 want=$2 id draft n
  release_row "$tag"
  [[ -n "$ROW" ]] || return 1
  IFS=$'\t' read -r _ id draft n <<<"$ROW"
  [[ "$draft" == false && "$n" == "$want" ]] ||
    die "release $tag exists but is not complete (draft=$draft, assets=$n, want false/$want): finish or delete it by hand"
}

# A tag that is not a release tag (a dry run such as v0.0.0-rel8) is skipped, not an error: exit 3.
if [[ "$kind" == skip && "$event" == push && "$ref" == refs/tags/* ]]; then
  echo "release-publish: $ref is not a release tag (vX.Y.Z[-rc.N]); nothing to publish" >&2
  exit 3
fi
[[ "$kind" != skip ]] || die "nothing to publish for $event on $ref (only a push to main or a tag vX.Y.Z[-rc.N])"
git rev-parse --verify -q refs/remotes/origin/main >/dev/null || die "origin/main is not fetched"
git merge-base --is-ancestor "$sha" refs/remotes/origin/main || die "$sha is not on origin/main: refusing to publish"

short=${sha:0:7}
files=("$DIST"/dash-wallet-"$VERSION"-*)
[[ -e "${files[0]}" && -f "$DIST/SHA256SUMS" ]] || die "no bundles or no SHA256SUMS in $DIST"
[[ ${#files[@]} -eq 4 ]] || die "expected 4 bundles in $DIST, found ${#files[@]}"
cont_prefix=DashWallet-continuous

if [[ "$kind" == main ]]; then
  base=${VERSION%%-dev.*}
  [[ "$base" != "$VERSION" ]] || die "VERSION $VERSION is not a -dev version"
  tag="v$base-dev.${GITHUB_RUN_NUMBER:?}+$short"
else
  tag=${ref#refs/tags/}
fi
run_url=${GITHUB_SERVER_URL:-https://github.com}/$repo/actions/runs/${GITHUB_RUN_ID:-0}

case "$mode" in
prepare)
  rm -rf "$out"
  mkdir -p "$out/attest" "$out/continuous"
  # Everything attested sits flat in attest/. The continuous files have the same bytes as the run's
  # (so one attestation covers both copies), except SHA256SUMS, which lists the continuous names.
  cp "${files[@]}" "$DIST/SHA256SUMS" "$out/attest/"
  if [[ "$kind" == main ]]; then
    for f in "${files[@]}"; do
      n=${f##*/}
      cp "$f" "$out/continuous/$cont_prefix-${n#dash-wallet-"$VERSION"-}"
    done
    (cd "$out/continuous" && sha256sum "$cont_prefix"-* > SHA256SUMS && sha256sum -c SHA256SUMS)
    cp "$out/continuous"/"$cont_prefix"-* "$out/attest/"
    cp "$out/continuous/SHA256SUMS" "$out/attest/SHA256SUMS.continuous"
  fi
  echo "release-publish: kind=$kind tag=$tag"
  ls -l "$out/attest"
  ;;

publish)
  [[ -d "$out/attest" ]] || die "run prepare first"
  verify_note="Attested by GitHub: \`gh attestation verify <file> --repo $repo\`. See docs/release/verifying.md."
  if [[ "$kind" == tag ]]; then
    version=${tag#v}
    notes=(--notes "Release $tag. $verify_note")
    [[ ! -f "$here/../../docs/release/${version%%-*}-notes.md" ]] || notes=(--notes-file "$here/../../docs/release/${version%%-*}-notes.md")
    if complete_release "$tag" 5; then
      echo "release-publish: release $tag exists and is complete; leaving it as it is"
    else
      # The tag exists (it triggered this run); a prerelease until 1.0.
      w gh release create "$tag" "${files[@]}" "$DIST/SHA256SUMS" --repo "$repo" --verify-tag \
        --prerelease --latest=false --title "Dash Wallet Desktop $version" "${notes[@]}"
    fi
    exit 0
  fi

  # main: the immutable per-run prerelease. Never edited once it exists.
  if complete_release "$tag" 5; then
    echo "release-publish: release $tag exists and is complete; immutable, leaving it as it is"
  else
    w gh release create "$tag" "${files[@]}" "$DIST/SHA256SUMS" --repo "$repo" --target "$sha" \
      --prerelease --latest=false --title "Preview build $VERSION" \
      --notes "Automated preview build of commit $sha ([run ${GITHUB_RUN_NUMBER}]($run_url)). Ad-hoc signed on macOS, unsigned on Windows and Linux; not for real funds. $verify_note"
  fi

  # main: the rolling prerelease.
  if probe gh api "repos/$repo/git/ref/tags/continuous" --jq .object.sha; then cur=$PROBE_OUT; else cur=; fi
  if [[ -n "$cur" && "$cur" != "$sha" ]] && git cat-file -e "$cur^{commit}" 2>/dev/null \
    && git merge-base --is-ancestor "$sha" "$cur"; then
    echo "release-publish: continuous is at $cur, newer than $sha; not moving it back"
    exit 0
  fi
  cont_notes="Latest build of main: commit $sha ([run ${GITHUB_RUN_NUMBER}]($run_url)), also published as $tag. Ad-hoc signed on macOS, unsigned on Windows and Linux; not for real funds. $verify_note"
  cont_files=("$out/continuous"/"$cont_prefix"-* "$out/continuous/SHA256SUMS")
  names=()
  for f in "${cont_files[@]}"; do names+=("${f##*/}"); done # SHA256SUMS last

  release_row continuous
  if [[ -z "$ROW" ]]; then
    # No release yet; a tag may still be there from an earlier one.
    [[ -z "$cur" || "$cur" == "$sha" ]] || w gh api -X PATCH "repos/$repo/git/refs/tags/continuous" -f sha="$sha" -F force=true --silent
    w gh release create continuous "${cont_files[@]}" --repo "$repo" --target "$sha" \
      --prerelease --latest=false --title "Latest build of main" --notes "$cont_notes"
    exit 0
  fi
  IFS=$'\t' read -r _ cont_id cont_draft _ <<<"$ROW"
  [[ "$cont_draft" == false ]] || die "the continuous release is a draft: publish or delete it by hand"

  # ---- asset helpers (all reads checked; a failed write in a rollback is reported, not fatal)
  ASSETS=
  load_assets() { # id, name, sha256 digest (empty when the API has none)
    ASSETS=$(gh api "repos/$repo/releases/$cont_id/assets" --paginate --jq '.[]|[.id,.name,(.digest // "")]|@tsv') ||
      die "cannot list the assets of the continuous release"
  }
  aid() { awk -F'\t' -v n="$1" '$2==n{print $1; exit}' <<<"$ASSETS"; }
  adigest() { awk -F'\t' -v n="$1" '$2==n{print $3; exit}' <<<"$ASSETS"; }
  rename_asset() { w gh api -X PATCH "repos/$repo/releases/assets/$1" -f name="$2" --silent; }
  delete_asset() { w gh api -X DELETE "repos/$repo/releases/assets/$1" --silent; }
  asset_body() { gh api -H 'Accept: application/octet-stream' "repos/$repo/releases/assets/$1"; }
  asset_sha() { # the sha256 of an asset: the API's digest, or the downloaded bytes
    local d id
    d=$(adigest "$1")
    if [[ "$d" == sha256:* ]]; then echo "${d#sha256:}"; return; fi
    id=$(aid "$1")
    asset_body "$id" | sha256sum | cut -d' ' -f1
  }
  # 0 when every binary canonical name matches the SHA256SUMS next to it.
  consistent() {
    local sums_id sums n want have
    sums_id=$(aid SHA256SUMS)
    [[ -n "$sums_id" ]] || return 1
    sums=$(asset_body "$sums_id") || die "cannot read the continuous SHA256SUMS"
    for n in "${names[@]}"; do
      [[ "$n" != SHA256SUMS ]] || continue
      [[ -n "$(aid "$n")" ]] || return 1
      want=$(awk -v n="$n" '$2==n{print $1; exit}' <<<"$sums")
      have=$(asset_sha "$n") || die "cannot hash $n"
      [[ -n "$want" && "$want" == "$have" ]] || return 1
    done
  }
  # Put the old generation back: each <name>.old replaces <name> (present or missing); stray <name>.new go.
  # Keeps going after a failed step and returns 1 if any failed.
  restore_olds() {
    local rc=0 n id oid nid
    ASSETS=$(gh api "repos/$repo/releases/$cont_id/assets" --paginate --jq '.[]|[.id,.name,(.digest // "")]|@tsv') || return 1
    for n in "${names[@]}"; do
      id=$(aid "$n"); oid=$(aid "$n.old"); nid=$(aid "$n.new")
      if [[ -n "$oid" ]]; then
        if [[ -n "$id" ]]; then delete_asset "$id" || rc=1; fi
        rename_asset "$oid" "$n" || rc=1
      fi
      if [[ -n "$nid" ]]; then delete_asset "$nid" || rc=1; fi
    done
    return $rc
  }
  # Step 1: what an interrupted earlier run left.
  recover() {
    local n id oid nid left=0
    load_assets
    for n in "${names[@]}"; do
      [[ -z "$(aid "$n.new")$(aid "$n.old")" ]] || left=1
    done
    ((left)) || return 0
    echo "release-publish: continuous has leftovers of an interrupted update; recovering"
    for n in "${names[@]}"; do # a new file that was never promoted is not authoritative
      nid=$(aid "$n.new")
      if [[ -n "$nid" ]]; then delete_asset "$nid" || die "recovery: cannot delete $n.new"; fi
    done
    load_assets
    for n in "${names[@]}"; do # a promotion cut off between its two renames: put the old file back
      id=$(aid "$n"); oid=$(aid "$n.old")
      if [[ -z "$id" && -n "$oid" ]]; then rename_asset "$oid" "$n" || die "recovery: cannot restore $n"; fi
    done
    if [[ -n "$dry" ]]; then return 0; fi
    load_assets
    if consistent; then
      echo "release-publish: the generation is consistent; dropping the .old copies"
      for n in "${names[@]}"; do
        oid=$(aid "$n.old")
        if [[ -n "$oid" ]]; then delete_asset "$oid" || die "recovery: cannot delete $n.old"; fi
      done
    else
      echo "release-publish: binaries and SHA256SUMS disagree; rolling back to the old generation"
      restore_olds || die "recovery: rollback failed; see the repair procedure in release-publish.sh"
    fi
  }

  recover
  # Step 2: the new generation, under temporary names.
  stage=$(mktemp -d)
  for n in "${names[@]}"; do
    cp "$out/continuous/$n" "$stage/$n.new"
    w gh release upload continuous "$stage/$n.new" --repo "$repo" || { restore_olds || true; die "upload of $n.new failed"; }
  done
  if [[ -n "$dry" ]]; then
    echo "DRY-RUN: then, for each of ${names[*]} (SHA256SUMS last): rename <name> to <name>.old, <name>.new to <name>; move the tag to $sha; edit the notes; delete the .old assets"
    exit 0
  fi
  # Steps 3-5, with rollback on any failure and on a cancel.
  tag_moved=
  fail() {
    echo "release-publish: $* ; rolling back the continuous release" >&2
    trap '' INT TERM
    if [[ -n "$tag_moved" && -n "$cur" ]]; then
      w gh api -X PATCH "repos/$repo/git/refs/tags/continuous" -f sha="$cur" -F force=true --silent || echo "release-publish: could not move the tag back to $cur" >&2
    fi
    restore_olds || echo "release-publish: ROLLBACK INCOMPLETE: rename each <name>.old over <name> (delete <name> first if present) and delete <name>.new, or re-run this job" >&2
    exit 1
  }
  trap 'fail "interrupted"' INT TERM
  load_assets
  for n in "${names[@]}"; do
    [[ -n "$(aid "$n.new")" ]] || fail "$n.new is not on the release"
  done
  for n in "${names[@]}"; do # SHA256SUMS is last in names
    id=$(aid "$n"); nid=$(aid "$n.new")
    if [[ -n "$id" ]]; then rename_asset "$id" "$n.old" || fail "cannot rename $n to $n.old"; fi
    rename_asset "$nid" "$n" || fail "cannot promote $n.new"
  done
  if [[ -n "$cur" ]]; then
    tag_moved=1 # before the call: a PATCH that landed but reported an error is moved back too
    w gh api -X PATCH "repos/$repo/git/refs/tags/continuous" -f sha="$sha" -F force=true --silent || fail "cannot move the tag"
  else
    w gh api -X POST "repos/$repo/git/refs" -f ref=refs/tags/continuous -f sha="$sha" --silent || fail "cannot create the tag"
  fi
  w gh release edit continuous --repo "$repo" --notes "$cont_notes" --prerelease --latest=false || fail "cannot edit the notes"
  trap - INT TERM
  # Step 5: the old generation goes only now; a failure here leaves harmless .old files for the next recovery.
  load_assets
  for n in "${names[@]}"; do
    oid=$(aid "$n.old")
    if [[ -n "$oid" ]]; then delete_asset "$oid" || echo "release-publish: warning: could not delete $n.old; the next run removes it" >&2; fi
  done
  ;;
*) die "unknown mode $mode" ;;
esac
