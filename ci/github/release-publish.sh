#!/usr/bin/env bash
# GitHub Actions (release.yml, publish job): turn a run's artifacts into GitHub releases.
#
#   ci/github/release-publish.sh classify  print main, tag or skip for this trigger (needs only the event and ref)
#   ci/github/release-publish.sh prepare   check what this run publishes, stage and copy the files
#   ci/github/release-publish.sh publish   create / update the releases
#
# Needs: DIST (the downloaded dist-* artifacts: five bundles, two SBOMs, SHA256SUMS, and optionally
# SHA256SUMS.asc on signed tag builds), VERSION (release-version.sh), GITHUB_EVENT_NAME, GITHUB_REF,
# GITHUB_SHA, GITHUB_REPOSITORY, GITHUB_RUN_NUMBER, GITHUB_RUN_ID, GITHUB_SERVER_URL, GH_TOKEN, and a
# checkout whose `origin/main` is fetched with full history.
# FEED (optional) is the update feed (latest.json and the macOS app archive). Only a stable release
# tag vX.Y.Z publishes one; a feed on any other trigger is refused.
# Final vX.Y.Z tags fail closed by default: publishing is refused unless verified attached signatures
# are present (SHA256SUMS.asc, signed-manifest.json from release-attach.sh with pinned identities and
# all 11 file hashes, and the signed updater feed latest.json). Set DW_ALLOW_UNSIGNED_STABLE=1 only
# for an explicit unsigned override (DW_SIGNING_REQUIRED=1 still overrides that escape hatch).
# Release candidates (-rc.N) are keyless unless DW_SIGN_RC=1 opts them in (docs/release/RC-SIGNING.md):
# then a pushed candidate is refused here (release.yml waits instead) and only release-publish-signed.yml
# publishes it, with SHA256SUMS.asc and a signed-manifest.json of 9 files (no updater feed on a candidate).
# OUT defaults to $DIST/../publish. DRY_RUN=1 runs the reads (release and tag lookups) and prints each
# write instead of doing it, so the logic can be run by hand against the real repository.
#
# What publishes (the only two cases; everything else exits before it touches anything: 3 for a tag
# that is not a release tag, 1 for the rest):
#   push to refs/heads/main      an immutable prerelease v<version>-dev.<run>+<short sha> with the run's
#                                files, and the rolling prerelease `continuous` with stable names;
#   push of a tag vX.Y.Z[-rc.N]  that tag's release: a full release for vX.Y.Z, a prerelease for -rc.N;
#                                notes from docs/release/<X.Y.Z>-notes.md when it exists.
# A commit that is not on origin/main never publishes, whatever the trigger or the tag says.
#
# `continuous` is replaced as one generation, with the previous one kept until everything has worked:
#   0. read everything that decides what to write (release lookups, the tag, the asset list) before the
#      first write of the run; a failed read stops the script there. Recovery and rollback read again later;
#      a failure there stops the rest of that step, never a guess;
#   1. recover from an interrupted earlier run, in every run, also an older one that will then leave
#      `continuous` alone: leftover <name>.new / <name>.old assets, a canonical name missing, binaries that
#      don't match SHA256SUMS: finish or roll back, never delete the only copy. The transaction record
#      (asset txn.json: old and new commit, old and new notes, the new SHA256SUMS hash) then decides which
#      generation the assets are, and the tag and notes are set to match it;
#   2. upload txn.json, then every new file as <name>.new (a failure here changes nothing a user sees);
#   3. for each name, SHA256SUMS last: rename the old asset <name>.old, then the new one <name>;
#   4. move the tag and rewrite the notes;
#   5. only then delete the <name>.old assets and txn.json.
# Any failure in 3 or 4, and SIGINT / SIGTERM (a cancelled run), roll the whole generation back: every
# canonical name is restored from its .old (a name the old generation lacked is deleted), the tag and the
# notes go back (also when the API call that failed had in fact taken effect), then txn.json goes. The end state is the old generation, complete.
# The Releases API is not atomic. While step 3 runs (a few seconds per asset, tens of seconds in all) a
# download of one name can fail for the two calls between its renames, and bundles can be the new build
# while SHA256SUMS is still the old one: check a download against the SHA256SUMS you fetch at the same
# time, and retry on a mismatch. A hard kill or an API outage during 2-5 can leave canonical names missing,
# mixed, or the tag and notes out of step with the assets, until the next run, whose step 1 repairs all of
# it (by hand: rename each <name>.old over the missing or new <name>, delete stray <name>.new and
# txn.json, set the tag and notes to the commit of the assets, or re-run the publish job). The tag moves
# after the assets and never backwards: if the current `continuous` commit is a descendant of this
# run's, the run publishes its immutable release and leaves `continuous` alone (after step 1). The immutable
# release is created after the reads of step 0 and before step 1, so a read failure in step 1 or later can
# follow that one creation, and nothing else.
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
signed_dispatch=0
if [[ "$event" == push ]]; then
  if [[ "$ref" == refs/heads/main ]]; then
    kind=main
  elif [[ "$ref" =~ ^refs/tags/v[0-9]+\.[0-9]+\.[0-9]+(-rc\.[0-9]+)?$ ]]; then
    kind=tag
  fi
elif [[ "$event" == workflow_dispatch && "${DW_SIGNED_DISPATCH:-}" == 1 && "$ref" =~ ^refs/tags/v[0-9]+\.[0-9]+\.[0-9]+(-rc\.[0-9]+)?$ ]]; then
  # Blocker #1: GitHub Actions preserves GITHUB_EVENT_NAME=workflow_dispatch over step env;
  # release-publish-signed.yml opts into this explicit signed-dispatch mode for vX.Y.Z and vX.Y.Z-rc.N tags.
  kind=tag
  signed_dispatch=1
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
[[ ${#files[@]} -eq 7 ]] || die "expected 5 bundles and 2 SBOMs in $DIST, found ${#files[@]}"
cont_prefix=DashWallet-continuous

if [[ "$kind" == main ]]; then
  base=${VERSION%%-dev.*}
  [[ "$base" != "$VERSION" ]] || die "VERSION $VERSION is not a -dev version"
  tag="v$base-dev.${GITHUB_RUN_NUMBER:?}+$short"
else
  tag=${ref#refs/tags/}
fi
asc=()
if [[ -f "$DIST/SHA256SUMS.asc" ]]; then
  [[ -s "$DIST/SHA256SUMS.asc" ]] || die "$DIST/SHA256SUMS.asc is empty"
  [[ "$kind" == tag ]] || die "SHA256SUMS.asc on $ref: only a release tag publishes a PGP signature"
  asc=("$DIST/SHA256SUMS.asc")
fi
feed=()
if [[ -n "${FEED:-}" && -d "$FEED" ]]; then
  while IFS= read -r f; do feed+=("$f"); done < <(find "$FEED" -maxdepth 1 -type f | LC_ALL=C sort)
fi
if ((${#feed[@]})); then
  [[ "$kind" == tag && "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "an update feed on $ref: only a stable release tag publishes one"
  [[ ${#feed[@]} -eq 2 && -f "$FEED/latest.json" && -f "$FEED/dash-wallet-$VERSION-macos-universal.app.tar.gz" ]] ||
    die "the feed must be latest.json and dash-wallet-$VERSION-macos-universal.app.tar.gz"
  jq -e --arg v "$VERSION" --arg p "https://github.com/$repo/releases/download/$tag/" \
    '.version == $v
     and (.platforms | keys | sort) == ["darwin-aarch64-app","darwin-x86_64-app","linux-x86_64-appimage","windows-x86_64-nsis"]
     and .platforms["linux-x86_64-appimage"].url == ($p + "dash-wallet-" + $v + "-linux-x86_64.AppImage")
     and .platforms["windows-x86_64-nsis"].url == ($p + "dash-wallet-" + $v + "-windows-x64-setup.exe")
     and .platforms["darwin-aarch64-app"].url == ($p + "dash-wallet-" + $v + "-macos-universal.app.tar.gz")
     and .platforms["darwin-x86_64-app"].url == ($p + "dash-wallet-" + $v + "-macos-universal.app.tar.gz")' \
    "$FEED/latest.json" > /dev/null ||
    die "$FEED/latest.json does not pin exact release URLs for $tag"
fi
# Decision 1: Final stable tags vX.Y.Z fail closed and always require verified attached signatures
# unless DW_ALLOW_UNSIGNED_STABLE=1 is explicitly set (and neither DW_SIGNING_REQUIRED=1 nor
# DW_SIGNED_DISPATCH=1 is set). A release candidate (-rc.N) requires them only on a signed dispatch or when
# DW_SIGN_RC=1 opted candidates in; main/continuous are never required to be signed.
require_signed=0
rc_tag=0
signed_n=11
if [[ "$kind" == tag && "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  if ((signed_dispatch)) || [[ "${DW_ALLOW_UNSIGNED_STABLE:-}" != 1 || "${DW_SIGNING_REQUIRED:-}" == 1 ]]; then
    require_signed=1
  fi
elif [[ "$kind" == tag ]]; then
  # A signed candidate has no updater feed (latest.json, .app.tar.gz): 9 files instead of 11.
  rc_tag=1
  signed_n=9
  if ((signed_dispatch)) || [[ "${DW_SIGN_RC:-}" == 1 ]]; then
    require_signed=1
  fi
fi
sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}
verify_signed_manifest() {
  local base_dir=$1 feed_dir=$2 manifest=$3
  jq -e --arg v "$VERSION" --argjson n "$signed_n" \
    '.schema == 1
     and .version == $v
     and (.pgp_fpr | type == "string" and test("^[0-9A-Fa-f]{40}$"))
     and (.apple_team_id | type == "string" and test("^[A-Z0-9]{10}$"))
     and (.windows_leaf_sha256 | type == "string" and test("^sha256:[0-9a-f]{64}$"))
     and (.files | type == "object" and length == $n)' \
    "$manifest" >/dev/null ||
    die "$tag: signed-manifest.json is missing required pinned identities (pgp_fpr/apple_team_id/windows_leaf_sha256) or $signed_n file hashes"
  local expected_all=(
    "SHA256SUMS"
    "SHA256SUMS.asc"
    "dash-wallet-$VERSION-linux-amd64.deb"
    "dash-wallet-$VERSION-linux-x86_64.AppImage"
    "dash-wallet-$VERSION-linux-x86_64.flatpak"
    "dash-wallet-$VERSION-macos-universal.dmg"
    "dash-wallet-$VERSION-sbom-npm.cdx.json"
    "dash-wallet-$VERSION-sbom-rust.cdx.json"
    "dash-wallet-$VERSION-windows-x64-setup.exe"
  )
  ((rc_tag)) || expected_all+=("dash-wallet-$VERSION-macos-universal.app.tar.gz" "latest.json")
  local fname want_h src got_h
  for fname in "${expected_all[@]}"; do
    want_h=$(jq -r --arg k "$fname" '.files[$k] // empty' "$manifest")
    [[ "$want_h" =~ ^[0-9a-f]{64}$ ]] || die "$tag: signed-manifest.json missing valid sha256 for $fname"
    if [[ -f "$base_dir/$fname" ]]; then
      src="$base_dir/$fname"
    elif [[ -n "$feed_dir" && -f "$feed_dir/$fname" ]]; then
      src="$feed_dir/$fname"
    else
      die "$tag: missing file $fname listed in signed-manifest.json"
    fi
    got_h=$(sha256_of "$src")
    [[ "$got_h" == "$want_h" ]] ||
      die "$tag: signed-manifest.json hash mismatch for $fname (got $got_h, expected $want_h; re-run ci/github/release-attach.sh)"
  done
}
if [[ "$kind" == tag && "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ && "${DW_UPDATER_REQUIRED:-}" == 1 ]] && ((!${#feed[@]})); then
  die "$tag: DW_UPDATER_REQUIRED=1 is set but no updater feed (latest.json + macos-universal.app.tar.gz) was provided"
fi
if ((require_signed)); then
  missing_sigs=()
  ((${#asc[@]})) || missing_sigs+=("SHA256SUMS.asc")
  ((rc_tag || ${#feed[@]})) || missing_sigs+=("updater feed (latest.json + macos-universal.app.tar.gz)")
  manifest=${SIGNED_MANIFEST:-$DIST/../signed-manifest.json}
  [[ -f "$manifest" ]] || missing_sigs+=("attached macOS/Windows signatures (signed-manifest.json from release-attach.sh)")
  if ((${#missing_sigs[@]})); then
    missing_str=$(IFS=', '; echo "${missing_sigs[*]}")
    if ((rc_tag)); then
      die "$tag requires attached signatures (a signed dispatch, or DW_SIGN_RC=1 opted candidates in), missing: $missing_str (sign locally and run release-publish-signed.yml; see docs/release/RC-SIGNING.md)"
    fi
    die "$tag requires attached signatures (stable tags fail closed; set DW_ALLOW_UNSIGNED_STABLE=1 only for an explicit unsigned override), missing: $missing_str (sign locally and run ci/github/release-attach.sh; see docs/release/SIGNING.md)"
  fi
  verify_signed_manifest "$DIST" "${FEED:-}" "$manifest"
fi
run_url=${GITHUB_SERVER_URL:-https://github.com}/$repo/actions/runs/${GITHUB_RUN_ID:-0}

case "$mode" in
prepare)
  rm -rf "$out"
  mkdir -p "$out/attest" "$out/continuous"
  # Everything attested sits flat in attest/. The continuous files have the same bytes as the run's
  # (so one attestation covers both copies), except SHA256SUMS, which lists the continuous names.
  cp "${files[@]}" "$DIST/SHA256SUMS" "$out/attest/"
  if ((${#asc[@]})); then cp "${asc[@]}" "$out/attest/"; fi
  if ((${#feed[@]})); then cp "${feed[@]}" "$out/attest/"; fi
  if [[ "$kind" == main ]]; then
    for f in "${files[@]}"; do
      n=${f##*/}
      cp "$f" "$out/continuous/$cont_prefix-${n#dash-wallet-"$VERSION"-}"
    done
    (cd "$out/continuous" && sha256sum "$cont_prefix"-* > SHA256SUMS && sha256sum -c SHA256SUMS)
    cp "$out/continuous"/"$cont_prefix"-* "$out/attest/"
    cp "$out/continuous/SHA256SUMS" "$out/attest/SHA256SUMS.continuous"
  fi
  jq -n \
    --arg repo "$repo" \
    --arg kind "$kind" \
    --arg tag "$tag" \
    --arg version "$VERSION" \
    --arg sha "$sha" \
    '{repo: $repo, kind: $kind, tag: $tag, version: $version, sha: $sha}' > "$out/prepare-meta.json"
  echo "release-publish: kind=$kind tag=$tag"
  ls -l "$out/attest"
  ;;

publish)
  [[ -d "$out/attest" && -f "$out/prepare-meta.json" ]] || die "run prepare first"
  if [[ "$kind" == tag ]]; then
    jq -e \
      --arg repo "$repo" \
      --arg kind "$kind" \
      --arg tag "$tag" \
      --arg version "$VERSION" \
      --arg sha "$sha" \
      '.repo == $repo and .kind == $kind and .tag == $tag and .version == $version and .sha == $sha' \
      "$out/prepare-meta.json" >/dev/null ||
      die "$tag: staged $out/attest was prepared for a different repo/tag/version/commit (re-run prepare for $tag @ $sha)"
  else
    jq -e --arg repo "$repo" --arg kind "$kind" '.repo == $repo and .kind == $kind' \
      "$out/prepare-meta.json" >/dev/null ||
      die "$tag: staged $out was prepared for a different repo or trigger kind"
  fi
  verify_note="Attested by GitHub: \`gh attestation verify <file> --repo $repo\`. See docs/release/verifying.md."
  if [[ "$kind" == tag ]]; then
    version=${tag#v}
    notes=(--notes "Release $tag. $verify_note")
    [[ ! -f "$here/../../docs/release/${version%%-*}-notes.md" ]] || notes=(--notes-file "$here/../../docs/release/${version%%-*}-notes.md")
    if complete_release "$tag" $((8 + ${#asc[@]} + ${#feed[@]})); then
      if ((require_signed)); then
        verify_signed_manifest "$out/attest" "" "$manifest"
        IFS=$'\t' read -r _ existing_id _ _ <<<"$ROW"
        remote_assets=$(gh api "repos/$repo/releases/$existing_id/assets" --paginate --jq '.[]|[.id,.name,(.digest // "")]|@tsv') ||
          die "$tag: cannot list assets of existing release $existing_id to verify retry hashes"
        seen=0
        seen_names=$'\n'
        dl_tmp=$(mktemp)
        while IFS=$'\t' read -r aid aname adigest; do
          [[ -n "$aname" ]] || continue
          if [[ "$seen_names" == *$'\n'"$aname"$'\n'* ]]; then
            rm -f "$dl_tmp"
            die "$tag: existing public release has duplicate asset '$aname'"
          fi
          seen_names+="$aname"$'\n'
          want_h=$(jq -r --arg k "$aname" '.files[$k] // empty' "$manifest")
          if [[ -z "$want_h" ]]; then
            rm -f "$dl_tmp"
            die "$tag: existing public release has unexpected asset '$aname' not in signed-manifest.json"
          fi
          if [[ "$adigest" == sha256:* ]]; then
            got_h=${adigest#sha256:}
          else
            gh api -H 'Accept: application/octet-stream' "repos/$repo/releases/assets/$aid" > "$dl_tmp" || {
              rm -f "$dl_tmp"
              die "$tag: cannot download existing asset '$aname' ($aid) to verify its hash"
            }
            got_h=$(sha256_of "$dl_tmp")
          fi
          if [[ "$got_h" != "$want_h" ]]; then
            rm -f "$dl_tmp"
            die "$tag: existing public release asset '$aname' has sha256 $got_h != $want_h from signed-manifest.json; refusing to report success or delete staging"
          fi
          seen=$((seen + 1))
        done <<<"$remote_assets"
        rm -f "$dl_tmp"
        [[ $seen -eq $signed_n ]] ||
          die "$tag: existing public release verified $seen/$signed_n assets against signed-manifest.json"
        echo "release-publish: release $tag exists and all $signed_n published assets match signed-manifest.json; leaving it as it is"
      else
        echo "release-publish: release $tag exists and is complete; leaving it as it is"
      fi
    else
      # The tag exists (it triggered this run). A version (vX.Y.Z) is a full release, and GitHub picks
      # Latest by version; a release candidate (-rc.N) is a prerelease, never Latest.
      # L4: Upload the exact attested copies from $out/attest/ (re-verified against signed-manifest.json
      # when require_signed=1) so no TOCTOU change between prepare/attest and publish can slip through.
      if ((require_signed)); then
        verify_signed_manifest "$out/attest" "" "$manifest"
      fi
      attest_files=()
      while IFS= read -r f; do attest_files+=("$f"); done < <(find "$out/attest" -maxdepth 1 -type f | LC_ALL=C sort)
      want_n=$((8 + ${#asc[@]} + ${#feed[@]}))
      [[ ${#attest_files[@]} -eq $want_n ]] ||
        die "$tag: expected $want_n attested files in $out/attest, found ${#attest_files[@]}"
      flavour=()
      if [[ "$version" == *-rc.* ]]; then
        flavour=(--prerelease --latest=false)
      elif ((!${#feed[@]})); then
        # An unsigned stable override (DW_ALLOW_UNSIGNED_STABLE=1 without latest.json) must never
        # become Latest and displace a signed release's /releases/latest/download/latest.json feed.
        flavour=(--latest=false)
      fi
      w gh release create "$tag" "${attest_files[@]}" --repo "$repo" --verify-tag \
        ${flavour[@]+"${flavour[@]}"} --title "Dash Wallet Desktop $version" "${notes[@]}"
    fi
    exit 0
  fi

  cont_notes="Latest build of main: commit $sha ([run ${GITHUB_RUN_NUMBER}]($run_url)), also published as $tag. Ad-hoc signed on macOS, unsigned on Windows and Linux; not for real funds. $verify_note"
  cont_files=("$out/continuous"/"$cont_prefix"-* "$out/continuous/SHA256SUMS")
  names=()
  for f in "${cont_files[@]}"; do names+=("${f##*/}"); done # SHA256SUMS last
  marker=txn.json # the transaction record of an update (see the header)

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
  drop_marker() { # the transaction record, once the assets, tag and notes agree again
    local id
    ASSETS=$(gh api "repos/$repo/releases/$cont_id/assets" --paginate --jq '.[]|[.id,.name,(.digest // "")]|@tsv') || return 1
    id=$(aid "$marker")
    if [[ -n "$id" ]]; then delete_asset "$id"; fi
  }
  # Step 1: what an interrupted earlier run left. Assets first (leftover <name>.new / <name>.old), then the
  # tag and notes are set to match whichever generation the assets ended up as, using the transaction record.
  recover() {
    local n id oid nid mid txn left=0 want_sums t nn
    load_assets
    for n in "${names[@]}"; do
      [[ -z "$(aid "$n.new")$(aid "$n.old")" ]] || left=1
    done
    mid=$(aid "$marker")
    ((left)) || [[ -n "$mid" ]] || return 0
    echo "release-publish: continuous has leftovers of an interrupted update; recovering"
    if ((left)); then
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
    fi
    [[ -z "$dry" ]] || return 0
    if [[ -n "$mid" ]]; then
      load_assets
      txn=$(asset_body "$mid") || die "recovery: cannot read the transaction record"
      want_sums=$(jq -r .new_sums <<<"$txn")
      if [[ -n "$(aid SHA256SUMS)" && "$(asset_sha SHA256SUMS)" == "$want_sums" ]]; then
        t=$(jq -r .new_sha <<<"$txn"); nn=$(jq -r .new_notes <<<"$txn")
      else
        t=$(jq -r .old_sha <<<"$txn"); nn=$(jq -r .old_notes <<<"$txn")
      fi
      echo "release-publish: setting the continuous tag and notes to match its assets (${t:-no tag})"
      if [[ -n "$t" ]]; then
        w gh api -X PATCH "repos/$repo/git/refs/tags/continuous" -f sha="$t" -F force=true --silent || die "recovery: cannot set the tag"
      fi
      w gh release edit continuous --repo "$repo" --notes "$nn" --prerelease --latest=false || die "recovery: cannot set the notes"
      delete_asset "$mid" || die "recovery: cannot delete the transaction record"
    fi
  }

  # ---- the reads that decide what to write come first: a failed one stops the script before any write
  if complete_release "$tag" 8; then imm=1; else imm=0; fi
  if probe gh api "repos/$repo/git/ref/tags/continuous" --jq .object.sha; then cur=$PROBE_OUT; else cur=; fi
  release_row continuous
  cont_id=
  if [[ -n "$ROW" ]]; then
    IFS=$'\t' read -r _ cont_id cont_draft _ <<<"$ROW"
    [[ "$cont_draft" == false ]] || die "the continuous release is a draft: publish or delete it by hand"
    load_assets
  fi

  # main: the immutable per-run prerelease. Never edited once it exists.
  if ((imm)); then
    echo "release-publish: release $tag exists and is complete; immutable, leaving it as it is"
  else
    w gh release create "$tag" "${files[@]}" "$DIST/SHA256SUMS" --repo "$repo" --target "$sha" \
      --prerelease --latest=false --title "Preview build $VERSION" \
      --notes "Automated preview build of commit $sha ([run ${GITHUB_RUN_NUMBER}]($run_url)). Ad-hoc signed on macOS, unsigned on Windows and Linux; not for real funds. $verify_note"
  fi

  # main: the rolling prerelease. An interrupted earlier update is repaired first, whichever run this is.
  if [[ -n "$cont_id" ]]; then
    recover
    if probe gh api "repos/$repo/git/ref/tags/continuous" --jq .object.sha; then cur=$PROBE_OUT; else cur=; fi
  fi
  if [[ -n "$cur" && "$cur" != "$sha" ]] && git cat-file -e "$cur^{commit}" 2>/dev/null \
    && git merge-base --is-ancestor "$sha" "$cur"; then
    echo "release-publish: continuous is at $cur, newer than $sha; not moving it back"
    exit 0
  fi
  if [[ -z "$cont_id" ]]; then
    # No release yet; a tag may still be there from an earlier one.
    [[ -z "$cur" || "$cur" == "$sha" ]] || w gh api -X PATCH "repos/$repo/git/refs/tags/continuous" -f sha="$sha" -F force=true --silent
    w gh release create continuous "${cont_files[@]}" --repo "$repo" --target "$sha" \
      --prerelease --latest=false --title "Latest build of main" --notes "$cont_notes"
    exit 0
  fi

  # Step 2: the transaction record, then the new generation under temporary names.
  old_notes=$(gh api "repos/$repo/releases/$cont_id" --jq '.body // ""') || die "cannot read the continuous notes"
  stage=$(mktemp -d)
  jq -n --arg old_sha "$cur" --arg old_notes "$old_notes" --arg new_sha "$sha" --arg new_notes "$cont_notes" \
    --arg new_sums "$(sha256sum < "$out/continuous/SHA256SUMS" | cut -d' ' -f1)" \
    '{old_sha: $old_sha, old_notes: $old_notes, new_sha: $new_sha, new_notes: $new_notes, new_sums: $new_sums}' > "$stage/$marker"
  w gh release upload continuous "$stage/$marker" --repo "$repo" || { drop_marker || true; die "cannot record the update"; }
  for n in "${names[@]}"; do
    cp "$out/continuous/$n" "$stage/$n.new"
    w gh release upload continuous "$stage/$n.new" --repo "$repo" || { restore_olds && drop_marker || true; die "upload of $n.new failed"; }
  done
  if [[ -n "$dry" ]]; then
    echo "DRY-RUN: then, for each of ${names[*]} (SHA256SUMS last): rename <name> to <name>.old, <name>.new to <name>; move the tag to $sha; edit the notes; delete the .old assets and the record"
    exit 0
  fi
  # Steps 3-5, with rollback on any failure and on a cancel.
  tag_moved=
  notes_touched=
  added=() # names the old generation lacks: a rollback deletes them
  fail() {
    local ok=1
    echo "release-publish: $* ; rolling back the continuous release" >&2
    trap '' INT TERM
    if [[ -n "$tag_moved" && -n "$cur" ]]; then
      w gh api -X PATCH "repos/$repo/git/refs/tags/continuous" -f sha="$cur" -F force=true --silent || { ok=0; echo "release-publish: could not move the tag back to $cur" >&2; }
    fi
    if [[ -n "$notes_touched" ]]; then
      w gh release edit continuous --repo "$repo" --notes "$old_notes" --prerelease --latest=false || { ok=0; echo "release-publish: could not restore the notes" >&2; }
    fi
    if ((${#added[@]})); then
      local n id
      ASSETS=$(gh api "repos/$repo/releases/$cont_id/assets" --paginate --jq '.[]|[.id,.name,(.digest // "")]|@tsv') || ok=0
      for n in "${added[@]}"; do
        id=$(aid "$n")
        if [[ -n "$id" ]]; then delete_asset "$id" || { ok=0; echo "release-publish: could not delete the new $n" >&2; }; fi
      done
    fi
    restore_olds || { ok=0; echo "release-publish: ROLLBACK INCOMPLETE: rename each <name>.old over <name> (delete <name> first if present) and delete <name>.new, or re-run this job" >&2; }
    # The record stays when anything above failed: the next run's recovery finishes the job with it.
    if ((ok)); then drop_marker || echo "release-publish: could not delete $marker; the next run removes it" >&2; fi
    exit 1
  }
  trap 'fail "interrupted"' INT TERM
  load_assets
  for n in "${names[@]}"; do
    [[ -n "$(aid "$n.new")" ]] || fail "$n.new is not on the release"
  done
  for n in "${names[@]}"; do # SHA256SUMS is last in names
    id=$(aid "$n"); nid=$(aid "$n.new")
    if [[ -n "$id" ]]; then rename_asset "$id" "$n.old" || fail "cannot rename $n to $n.old"; else added+=("$n"); fi
    rename_asset "$nid" "$n" || fail "cannot promote $n.new"
  done
  if [[ -n "$cur" ]]; then
    tag_moved=1 # before the call: a PATCH that landed but reported an error is moved back too
    w gh api -X PATCH "repos/$repo/git/refs/tags/continuous" -f sha="$sha" -F force=true --silent || fail "cannot move the tag"
  else
    w gh api -X POST "repos/$repo/git/refs" -f ref=refs/tags/continuous -f sha="$sha" --silent || fail "cannot create the tag"
  fi
  notes_touched=1 # likewise: a notes edit that landed but reported an error is undone too
  w gh release edit continuous --repo "$repo" --notes "$cont_notes" --prerelease --latest=false || fail "cannot edit the notes"
  trap - INT TERM
  # Step 5: the old generation goes only now; a failure here leaves harmless .old files for the next recovery.
  load_assets
  for n in "${names[@]}"; do
    oid=$(aid "$n.old")
    if [[ -n "$oid" ]]; then delete_asset "$oid" || echo "release-publish: warning: could not delete $n.old; the next run removes it" >&2; fi
  done
  drop_marker || echo "release-publish: warning: could not delete $marker; the next run removes it" >&2
  ;;
*) die "unknown mode $mode" ;;
esac
