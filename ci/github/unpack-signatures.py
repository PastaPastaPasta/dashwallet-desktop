#!/usr/bin/env python3
"""Unpack the detached signatures release-sign.yml's signing job packed.

  unpack-signatures.py <signatures.tar> <empty-dir>

Extracts only regular files and directories under `sigs/`. Any other member (a link, a device, an
absolute path, a `..` component, anything outside `sigs/`) refuses the whole archive before a byte is
written. What the signatures say is checked afterwards by release-attach.sh.
"""
import os
import sys
import tarfile
from pathlib import Path, PurePosixPath


def main():
    if len(sys.argv) != 3:
        raise SystemExit(__doc__)
    archive, dest = sys.argv[1], Path(sys.argv[2])
    if not dest.is_dir() or dest.is_symlink() or any(dest.iterdir()):
        raise SystemExit(f"unpack-signatures: {dest} must be an empty directory")
    with tarfile.open(archive, "r:") as tar:
        members = tar.getmembers()
        for m in members:
            parts = PurePosixPath(m.name).parts
            if not (m.isreg() or m.isdir()):
                raise SystemExit(f"unpack-signatures: refusing {m.name!r}: not a regular file or a directory")
            if m.name.startswith("/") or ".." in parts or not parts or parts[0] != "sigs":
                raise SystemExit(f"unpack-signatures: refusing {m.name!r}: outside sigs/")
        if not any(m.isreg() for m in members):
            raise SystemExit("unpack-signatures: the archive holds no file")
        for m in members:
            target = dest.joinpath(*PurePosixPath(m.name).parts)
            if m.isdir():
                target.mkdir(parents=True, exist_ok=True)
                continue
            target.parent.mkdir(parents=True, exist_ok=True)
            src = tar.extractfile(m)
            fd = os.open(target, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o644)
            with os.fdopen(fd, "wb") as out:
                while chunk := src.read(1 << 16):
                    out.write(chunk)
    print(f"unpack-signatures: unpacked {sum(m.isreg() for m in members)} files into {dest / 'sigs'}")


if __name__ == "__main__":
    main()
