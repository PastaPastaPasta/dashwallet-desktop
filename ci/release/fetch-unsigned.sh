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
umask 077

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

check_fresh_dir() {
  local label=$1 dir=$2
  [[ ! -L "$dir" ]] || die "$label must not be a symlink: $dir"
  if [[ -e "$dir" ]]; then
    [[ -d "$dir" ]] || die "$label exists and is not a directory: $dir"
    [[ -z "$(ls -A "$dir")" ]] ||
      die "$label must be absent or an empty directory (refusing to overwrite/delete existing files in $dir)"
    # L3: Refuse an existing directory not owned by the current user or writable by group/others.
    python3 - "$label" "$dir" <<'PY' || exit 1
import os, stat, sys
label, path = sys.argv[1], sys.argv[2]
st = os.lstat(path)
if stat.S_ISLNK(st.st_mode) or not stat.S_ISDIR(st.st_mode):
    raise SystemExit(f"fetch-unsigned: {label} is not a real directory: {path}")
if st.st_uid != os.getuid():
    raise SystemExit(f"fetch-unsigned: {label} is not owned by current user (uid {st.st_uid} != {os.getuid()}): {path}")
if st.st_mode & 0o022:
    raise SystemExit(f"fetch-unsigned: {label} must not be group- or world-writable (mode {oct(st.st_mode & 0o777)}): {path}")
PY
  fi
}

check_fresh_dir "--unsigned-dir" "$unsigned_dir"
check_fresh_dir "--sigs-dir" "$sigs_dir"
[[ "$unsigned_dir" != "$sigs_dir" ]] || die "--unsigned-dir and --sigs-dir must be distinct paths"

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

# Build a single validated inventory of every entry in $stage (including dotfiles) BEFORE attestation
# and copy, refusing hidden entries, symlinks, subdirectories, or non-regular files so nothing can
# slip past `gh attestation verify` into --unsigned-dir.
inventory="$work/inventory.txt"
python3 - "$stage" "$inventory" <<'PY' || exit 1
import os, stat, sys
from pathlib import Path

stage, inv_path = Path(sys.argv[1]), Path(sys.argv[2])
names = []
for entry in sorted(stage.iterdir(), key=lambda p: p.name):
    name = entry.name
    if name.startswith("."):
        raise SystemExit(f"fetch-unsigned: refusing unexpected hidden file in downloaded artifacts: {name}")
    if "\n" in name or "\r" in name or "/" in name:
        raise SystemExit(f"fetch-unsigned: refusing invalid filename in downloaded artifacts: {name!r}")
    st = os.lstat(entry)
    if stat.S_ISLNK(st.st_mode) or not stat.S_ISREG(st.st_mode):
        raise SystemExit(f"fetch-unsigned: refusing non-regular or symlink entry in downloaded artifacts: {name}")
    names.append(name)
if "unsigned.SHA256SUMS" not in names:
    raise SystemExit("fetch-unsigned: downloaded artifacts missing unsigned.SHA256SUMS")
if len(names) < 9:
    raise SystemExit(f"fetch-unsigned: expected at least 9 regular unsigned files, found {len(names)}")
inv_path.write_text("\n".join(names) + "\n")
PY

echo "fetch-unsigned: verifying Sigstore build provenance attestation on every downloaded file"
count=0
while IFS= read -r fname; do
  [[ -n "$fname" ]] || continue
  gh attestation verify "$stage/$fname" \
    --repo "$repo" \
    --signer-workflow "$repo/.github/workflows/release.yml" \
    --source-ref "refs/tags/$tag" \
    --source-digest "$want_sha"
  count=$((count + 1))
done < "$inventory"
[[ $count -ge 9 ]] || die "expected at least 9 attested unsigned files, verified $count"

echo "fetch-unsigned: verifying unsigned.SHA256SUMS"
if command -v sha256sum >/dev/null 2>&1; then
  (cd "$stage" && sha256sum -c unsigned.SHA256SUMS)
else
  (cd "$stage" && shasum -a 256 -c unsigned.SHA256SUMS)
fi

# Re-verify destinations are still fresh/empty/owner-only right before writing (never rm -rf user
# directories and open destinations with O_CREAT|O_EXCL|O_NOFOLLOW to prevent symlink TOCTOU).
check_fresh_dir "--unsigned-dir" "$unsigned_dir"
check_fresh_dir "--sigs-dir" "$sigs_dir"
mkdir -p "$unsigned_dir" "$sigs_dir"
chmod 700 "$unsigned_dir" "$sigs_dir"
[[ "$(cd "$unsigned_dir" && pwd -P)" != "$(cd "$sigs_dir" && pwd -P)" ]] ||
  die "--unsigned-dir and --sigs-dir resolve to the same directory"
python3 - "$stage" "$inventory" "$unsigned_dir" "$sigs_dir" <<'PY'
import os, shutil, stat, sys
from pathlib import Path

stage, inv_path, u_dir, s_dir = Path(sys.argv[1]), Path(sys.argv[2]), Path(sys.argv[3]), Path(sys.argv[4])
names = [line for line in inv_path.read_text().splitlines() if line]
actual = sorted(p.name for p in stage.iterdir())
if actual != names:
    raise SystemExit(f"fetch-unsigned: staged directory contents changed after attestation ({actual!r} != {names!r})")

flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0)

def safe_copy(src: Path, dst: Path) -> None:
    st = os.lstat(src)
    if stat.S_ISLNK(st.st_mode) or not stat.S_ISREG(st.st_mode):
        raise SystemExit(f"fetch-unsigned: unexpected non-regular entry in staged download: {src}")
    mode = 0o700 if (st.st_mode & 0o100) else 0o600
    fd = os.open(str(dst), flags, mode)
    with os.fdopen(fd, "wb") as out_f, src.open("rb") as in_f:
        shutil.copyfileobj(in_f, out_f)

for name in names:
    safe_copy(stage / name, u_dir / name)
safe_copy(stage / "unsigned.SHA256SUMS", s_dir / "unsigned.SHA256SUMS")
PY
echo "fetch-unsigned: verified $count attested unsigned files for $tag ($want_sha) into $unsigned_dir"
