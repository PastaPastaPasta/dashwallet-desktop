#!/usr/bin/env bash
# release-sign.yml: take the Windows installer's detached signature, made on the release signer's YubiKey with
# sign-windows.sh, from one commit of the detached-signatures repository, and attach it to CI's unsigned
# installer. It runs in a job that later holds keys, and the repository is public and not this one, so:
#   - the commit is named by its full hash, which a later push cannot change;
#   - nothing is checked out: the two files are read as blobs, and only as regular files (no link, no
#     submodule), from <tag>/ in that commit or from its root;
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

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
git init --quiet "$work/repo"
git -C "$work/repo" fetch --quiet --depth 1 "$repo_url" "$commit" || die "could not fetch $commit from $repo_url"
[[ "$(git -C "$work/repo" rev-parse FETCH_HEAD)" == "$commit" ]] || die "$repo_url did not return commit $commit"

prefix=
if [[ "$(git -C "$work/repo" ls-tree FETCH_HEAD -- "$tag" | awk '{ print $2 }')" == tree ]]; then
  prefix="$tag/"
fi

read_blob() { # read_blob <path in the commit> <destination>: a regular file's content, or nothing
  local path=$1 dest=$2 entry
  entry=$(git -C "$work/repo" ls-tree FETCH_HEAD -- "$path")
  [[ "$entry" == "100644 blob "* ]] || die "$path is not a regular file in $commit"
  git -C "$work/repo" cat-file blob "FETCH_HEAD:$path" > "$dest"
}

read_blob "${prefix}unsigned.SHA256SUMS" "$work/unsigned.SHA256SUMS"
cmp -s "$work/unsigned.SHA256SUMS" "$sigs_dir/unsigned.SHA256SUMS" ||
  die "$commit was signed over another build: its unsigned.SHA256SUMS differs from this run's"

mkdir -p "$sigs_dir/win" "$out_dir/dist"
read_blob "${prefix}win/$exe.pem" "$sigs_dir/win/$exe.pem"

attach_args=(attach-signature -in "$unsigned_dir/$exe" -sigin "$sigs_dir/win/$exe.pem" -out "$out_dir/dist/$exe")
if [[ -n "$ca_file" ]]; then
  attach_help=$(osslsigncode attach-signature --help 2>&1 || true)
  [[ "$attach_help" != *"-CAfile"* ]] || attach_args+=(-CAfile "$ca_file")
fi
rm -f "$out_dir/dist/$exe"
osslsigncode "${attach_args[@]}"
echo "fetch-windows-signature: attached ${prefix}win/$exe.pem of $commit to $exe"
