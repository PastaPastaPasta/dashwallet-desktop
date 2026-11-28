#!/usr/bin/env bash
# release-sign.yml: take the Windows installer's detached signature, made on the release signer's YubiKey with
# sign-windows.sh, from one commit of the detached-signatures repository, and attach it to CI's unsigned
# installer. It runs in a job that later holds keys, and the repository is public and not this one, so:
#   - the commit is named by its full hash, and it must be the tip of that repository's branch named after
#     the tag, where the runbook pushes it. GitHub also serves commits of a repository's forks through its
#     URL, so the hash alone would not say whose commit it is. The branch is fetched once and what that
#     fetch returned is compared with the hash, so there is no moment between a lookup and a fetch;
#   - nothing is checked out: the two files are read as blobs, and only as small regular files (no link, no
#     submodule), from <tag>/ in that commit or, when it has no <tag> entry, from its root;
#   - the signature must have been made over this run's unsigned build (unsigned.SHA256SUMS, byte for byte).
# Whose signature it is, is not decided here: ci/github/windows-verify-pinned.sh checks that next.
#
#   ci/release/fetch-windows-signature.sh \
#     --repo-url <https://github.com/owner/name.git> --commit <40-hex> --tag <vX.Y.Z-rc.N> \
#     --unsigned-dir <dir> --sigs-dir <dir> --out-dir <dir> [--ca-file <roots.pem>]
#
# --ca-file: osslsigncode <= 2.13 verifies the chain inside attach-signature and needs the roots there.
set -euo pipefail

repo_url=
commit=
tag=
unsigned_dir=
sigs_dir=
out_dir=
ca_file=

while (($#)); do
  case "$1" in
    --repo-url) repo_url=${2:?}; shift 2 ;;
    --commit) commit=${2:?}; shift 2 ;;
    --tag) tag=${2:?}; shift 2 ;;
    --unsigned-dir) unsigned_dir=${2:?}; shift 2 ;;
    --sigs-dir) sigs_dir=${2:?}; shift 2 ;;
    --out-dir) out_dir=${2:?}; shift 2 ;;
    --ca-file) ca_file=${2:?}; shift 2 ;;
    *) echo "fetch-windows-signature: unknown option: $1" >&2; exit 1 ;;
  esac
done

die() { echo "fetch-windows-signature: $*" >&2; exit 1; }

[[ -n "$repo_url" ]] || die "--repo-url is required"
[[ "$commit" =~ ^[0-9a-f]{40}$ ]] ||
  die "--commit must be the full 40-hex commit that holds the signature, not a branch or a tag (got '${commit:-<empty>}')"
[[ "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-rc\.[0-9]+)?$ ]] || die "--tag must be vX.Y.Z or vX.Y.Z-rc.N"
[[ -d "$unsigned_dir" && -d "$sigs_dir" && -n "$out_dir" ]] || die "--unsigned-dir, --sigs-dir and --out-dir are required"
command -v osslsigncode >/dev/null 2>&1 || die "osslsigncode is required"
if [[ -n "$ca_file" ]]; then
  [[ -f "$ca_file" && ! -L "$ca_file" ]] || die "--ca-file not found or is a symlink: $ca_file"
fi

version=${tag#v}
exe="dash-wallet-$version-windows-x64-setup.exe"
[[ -f "$unsigned_dir/$exe" && ! -L "$unsigned_dir/$exe" ]] || die "missing unsigned installer $unsigned_dir/$exe"
[[ -s "$sigs_dir/unsigned.SHA256SUMS" && ! -L "$sigs_dir/unsigned.SHA256SUMS" ]] ||
  die "missing $sigs_dir/unsigned.SHA256SUMS (run ci/release/fetch-unsigned.sh first)"

export GIT_TERMINAL_PROMPT=0
max_blob=1048576

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
repo() { git -C "$work/repo" "$@"; }
git init --quiet "$work/repo"

# One fetch, of the branch by its full ref name: what it returns is that branch's tip.
repo fetch --quiet --depth 1 -- "$repo_url" "refs/heads/$tag:refs/remotes/signature/$tag" ||
  die "could not fetch branch $tag from $repo_url; push the signature to that branch"
tip=$(repo rev-parse --verify --quiet "refs/remotes/signature/$tag^{commit}") ||
  die "branch $tag of $repo_url is not at a commit"
[[ "$tip" == "$commit" ]] ||
  die "$commit is not the tip of branch $tag in $repo_url (which is at $tip); push the signature there and pass that commit"

prefix=
tag_entry=$(repo ls-tree "$commit" -- "$tag" | awk '{ print $2 }')
if [[ "$tag_entry" == tree ]]; then
  prefix="$tag/"
elif [[ -n "$tag_entry" ]]; then
  die "$tag in $commit is not a directory"
fi

read_blob() { # read_blob <path in the commit> <destination>: the content of a small regular file, or the script stops
  local path=$1 dest=$2 entry mode kind oid size
  entry=$(repo ls-tree "$commit" -- "$path")
  [[ "$entry" != *$'\n'* ]] || die "$path names more than one entry in $commit"
  read -r mode kind oid _ <<<"$entry" || true
  [[ "$mode" == 100644 && "$kind" == blob && "$oid" =~ ^[0-9a-f]{40,64}$ ]] || die "$path is not a regular file in $commit"
  size=$(repo cat-file -s "$oid")
  [[ "$size" =~ ^[0-9]+$ && "$size" -gt 0 && "$size" -le "$max_blob" ]] || die "$path in $commit is empty or larger than $max_blob bytes"
  repo cat-file blob "$oid" > "$dest"
}

read_blob "${prefix}unsigned.SHA256SUMS" "$work/unsigned.SHA256SUMS"
cmp -s "$work/unsigned.SHA256SUMS" "$sigs_dir/unsigned.SHA256SUMS" ||
  die "$commit was signed over another build: its unsigned.SHA256SUMS differs from this run's"
read_blob "${prefix}win/$exe.pem" "$work/$exe.pem"

attach_args=(attach-signature -in "$unsigned_dir/$exe" -sigin "$work/$exe.pem" -out "$work/$exe")
if [[ -n "$ca_file" ]]; then
  attach_help=$(osslsigncode attach-signature --help 2>&1 || true)
  [[ "$attach_help" != *"-CAfile"* ]] || attach_args+=(-CAfile "$ca_file")
fi
osslsigncode "${attach_args[@]}" || die "the signature in $commit does not attach to $exe"

# Only a signature that attached reaches the directories the next steps read.
mkdir -p "$sigs_dir/win" "$out_dir/dist"
cp "$work/$exe.pem" "$sigs_dir/win/$exe.pem"
cp "$work/$exe" "$out_dir/dist/$exe"
echo "fetch-windows-signature: attached ${prefix}win/$exe.pem of $commit to $exe"
