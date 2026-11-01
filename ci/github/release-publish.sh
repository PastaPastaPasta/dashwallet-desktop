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
# `continuous` is replaced asset by asset: the new file is uploaded as <name>.new, then the old one is
# renamed <name>.old, the new one <name>, and the old one deleted. A download of <name> fails only
# between the two renames, two API calls, well under a second (and a retry succeeds). SHA256SUMS is
# swapped last, so for those seconds it can describe the previous build. The tag moves after the assets.
# It never moves backwards: if the current `continuous` commit is a descendant of this run's, the run
# publishes its immutable release and leaves `continuous` alone.
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
# A read that distinguishes "not there" from a failure: returns 0 with the output in $PROBE_OUT, 44 on a
# 404 / "release not found", and ends the script on any other error (a rate limit or a 5xx must not look
# like "absent" and send the script on to a write).
PROBE_OUT=
probe() {
  local err rc=0
  err=$(mktemp)
  PROBE_OUT=$("$@" 2>"$err") || rc=$?
  if ((rc == 0)); then rm -f "$err"; return 0; fi
  if grep -qiE 'HTTP 404|release not found' "$err"; then rm -f "$err"; return 44; fi
  cat "$err" >&2; rm -f "$err"
  die "failed: $*"
}
# A release that exists must be complete: published, with every file.
complete_release() {
  local tag=$1 want=$2
  probe gh release view "$tag" --repo "$repo" --json isDraft,assets --jq '[.isDraft,(.assets|length)]|@tsv' || return 1
  [[ "$PROBE_OUT" == "false"$'\t'"$want" ]] || die "release $tag exists but is not complete (isDraft/assets: $PROBE_OUT, want false/$want): finish or delete it by hand"
}
# A write: printed in a dry run, done otherwise.
w() { if [[ -n "$dry" ]]; then printf 'DRY-RUN:'; printf ' %q' "$@"; echo; else "$@"; fi; }

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
  if ! probe gh release view continuous --repo "$repo"; then
    # No release yet; a tag may still be there from an earlier one.
    [[ -z "$cur" || "$cur" == "$sha" ]] || w gh api -X PATCH "repos/$repo/git/refs/tags/continuous" -f sha="$sha" -F force=true
    w gh release create continuous "${cont_files[@]}" --repo "$repo" --target "$sha" \
      --prerelease --latest=false --title "Latest build of main" --notes "$cont_notes"
    exit 0
  fi
  rid=$(gh api "repos/$repo/releases/tags/continuous" --jq .id)
  assets() { gh api "repos/$repo/releases/$rid/assets" --paginate --jq '.[]|[.id,.name]|@tsv'; }
  # Leftovers of an interrupted swap.
  while IFS=$'\t' read -r id name; do
    [[ "$name" == *.new || "$name" == *.old ]] && w gh api -X DELETE "repos/$repo/releases/assets/$id"
  done < <(assets)
  # Upload every new file under a temporary name first.
  stage=$(mktemp -d)
  for f in "${cont_files[@]}"; do
    cp "$f" "$stage/${f##*/}.new"
    w gh release upload continuous "$stage/${f##*/}.new" --repo "$repo"
  done
  # Swap, SHA256SUMS last.
  swap() {
    local name=$1 old_id new_id
    old_id=$(assets | awk -F'\t' -v n="$name" '$2==n{print $1}')
    new_id=$(assets | awk -F'\t' -v n="$name.new" '$2==n{print $1}')
    if [[ -n "$dry" ]]; then echo "DRY-RUN: swap $name (old asset '${old_id:-none}', new '${new_id:-<uploaded>}')"; return; fi
    [[ -n "$new_id" ]] || die "uploaded $name.new not found"
    [[ -z "$old_id" ]] || gh api -X PATCH "repos/$repo/releases/assets/$old_id" -f name="$name.old" >/dev/null
    gh api -X PATCH "repos/$repo/releases/assets/$new_id" -f name="$name" >/dev/null
    [[ -z "$old_id" ]] || gh api -X DELETE "repos/$repo/releases/assets/$old_id"
  }
  for f in "${cont_files[@]}"; do
    [[ "${f##*/}" == SHA256SUMS ]] || swap "${f##*/}"
  done
  swap SHA256SUMS
  # The tag, then the text.
  if [[ -n "$cur" ]]; then
    w gh api -X PATCH "repos/$repo/git/refs/tags/continuous" -f sha="$sha" -F force=true
  else
    w gh api -X POST "repos/$repo/git/refs" -f ref=refs/tags/continuous -f sha="$sha"
  fi
  w gh release edit continuous --repo "$repo" --notes "$cont_notes" --prerelease --latest=false
  ;;
*) die "unknown mode $mode" ;;
esac
