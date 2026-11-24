#!/usr/bin/env bash
# Fail-closed downloader for unsigned CI release artifacts on the release signer's Mac.
#
# Pins the download strictly to the commit SHA peeled from the locally verified signed tag
# (--tag-sha, NEVER re-queried as ground truth from GitHub). Only exposes files to --unsigned-dir
# and --sigs-dir AFTER:
#   1. GitHub's current ref for --tag agrees with --tag-sha;
#   2. Actions run --run-id is a successful push of --tag on --repo building --tag-sha in release.yml;
#   3. Every downloaded file passes `gh attestation verify` against release.yml @ refs/tags/<tag> + <tag-sha>;
#   4. Every file passes `unsigned.SHA256SUMS`.
#
#   ci/release/fetch-unsigned.sh \
#     --tag v1.0.0 \
#     --tag-sha <40-hex-commit-from-local-signed-tag> \
#     --run-id <actions-run-id> \
#     --repo PastaPastaPasta/dashwallet-desktop \
#     --unsigned-dir <dir> \
#     --sigs-dir <dir>
set -euo pipefail
umask 022

tag=
tag_sha=
run_id=
repo=PastaPastaPasta/dashwallet-desktop
unsigned_dir=
sigs_dir=

while (($#)); do
  case "$1" in
    --tag) tag=${2:?}; shift 2 ;;
    --tag-sha) tag_sha=${2:?}; shift 2 ;;
    --run-id) run_id=${2:?}; shift 2 ;;
    --repo) repo=${2:?}; shift 2 ;;
    --unsigned-dir) unsigned_dir=${2:?}; shift 2 ;;
    --sigs-dir) sigs_dir=${2:?}; shift 2 ;;
    *) echo "fetch-unsigned: unknown option: $1" >&2; exit 1 ;;
  esac
done

die() { echo "fetch-unsigned: $*" >&2; exit 1; }

[[ "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "--tag must be vX.Y.Z"
[[ "$tag_sha" =~ ^[0-9a-fA-F]{40}$ ]] || die "--tag-sha must be a 40-hex commit SHA from your locally verified signed tag"
[[ "$run_id" =~ ^[0-9]+$ ]] || die "--run-id must be numeric"
[[ -n "$unsigned_dir" && -n "$sigs_dir" ]] || die "--unsigned-dir and --sigs-dir are required"

command -v gh >/dev/null 2>&1 || die "gh CLI is required"
command -v jq >/dev/null 2>&1 || die "jq is required"

want_sha=$(printf '%s' "$tag_sha" | tr '[:upper:]' '[:lower:]')
remote_sha=$(gh api "repos/$repo/commits/$tag" --jq .sha | tr '[:upper:]' '[:lower:]')
[[ "$remote_sha" == "$want_sha" ]] ||
  die "remote tag $tag on $repo resolves to $remote_sha, expected locally verified $want_sha (tag moved!)"

run_json=$(gh api "repos/$repo/actions/runs/$run_id")
run_workflow=$(jq -r '.path // empty' <<<"$run_json")
run_event=$(jq -r '.event // empty' <<<"$run_json")
run_branch=$(jq -r '.head_branch // empty' <<<"$run_json")
run_sha=$(jq -r '.head_sha // empty' <<<"$run_json" | tr '[:upper:]' '[:lower:]')
run_repo=$(jq -r '.head_repository.full_name // empty' <<<"$run_json")
run_conclusion=$(jq -r '.conclusion // empty' <<<"$run_json")

[[ "$run_workflow" == ".github/workflows/release.yml" ]] || die "run $run_id workflow is '$run_workflow', expected .github/workflows/release.yml"
[[ "$run_event" == "push" ]] || die "run $run_id event is '$run_event', expected 'push'"
[[ "$run_branch" == "$tag" ]] || die "run $run_id head_branch is '$run_branch', expected '$tag'"
[[ "$run_sha" == "$want_sha" ]] || die "run $run_id built commit $run_sha, expected locally verified $want_sha"
[[ "$run_repo" == "$repo" ]] || die "run $run_id head_repository is '$run_repo', expected '$repo'"
[[ "$run_conclusion" == "success" ]] || die "run $run_id conclusion is '$run_conclusion', expected 'success'"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
stage="$work/stage"
mkdir -p "$stage"

echo "fetch-unsigned: downloading release-$tag and unsigned-signing-inputs-$tag from run $run_id into staging dir"
gh run download "$run_id" --repo "$repo" -n "release-$tag" -D "$stage"
gh run download "$run_id" --repo "$repo" -n "unsigned-signing-inputs-$tag" -D "$stage"

[[ -s "$stage/unsigned.SHA256SUMS" ]] || die "downloaded artifacts missing unsigned.SHA256SUMS"

echo "fetch-unsigned: verifying Sigstore build provenance attestation on every downloaded file"
count=0
for f in "$stage"/*; do
  [[ -f "$f" ]] || continue
  gh attestation verify "$f" \
    --repo "$repo" \
    --signer-workflow "$repo/.github/workflows/release.yml" \
    --source-ref "refs/tags/$tag" \
    --source-digest "$want_sha"
  count=$((count + 1))
done
[[ $count -ge 9 ]] || die "expected at least 9 attested unsigned files, verified $count"

echo "fetch-unsigned: verifying unsigned.SHA256SUMS"
if command -v sha256sum >/dev/null 2>&1; then
  (cd "$stage" && sha256sum -c unsigned.SHA256SUMS)
else
  (cd "$stage" && shasum -a 256 -c unsigned.SHA256SUMS)
fi

rm -rf "$unsigned_dir"
mkdir -p "$unsigned_dir" "$sigs_dir"
cp "$stage"/* "$unsigned_dir/"
cp "$unsigned_dir/unsigned.SHA256SUMS" "$sigs_dir/unsigned.SHA256SUMS"
echo "fetch-unsigned: verified $count attested unsigned files for $tag ($want_sha) into $unsigned_dir"
