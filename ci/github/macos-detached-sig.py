#!/usr/bin/env python3
"""Extract and re-apply detached macOS code signatures and stapled notarization tickets.

Mirrors Dash Core's detached-sigs flow for Apple `codesign` + `notarytool` + `stapler`
without requiring a .p12 export:
  - `extract <unsigned.app> <signed.app> <sigs-dir>`
      Verifies that every non-signature file and every non-signature byte of every Mach-O
      binary in <unsigned.app> is byte-for-byte identical in <signed.app>, then writes only
      the small signature payloads (`Contents/_CodeSignature/CodeResources`, stapled
      `Contents/CodeResources`, per-slice `LC_CODE_SIGNATURE` SuperBlobs, and `manifest.json`)
      into <sigs-dir>.
  - `apply <unsigned.app> <sigs-dir> <out-signed.app>`
      Verifies <unsigned.app> matches `manifest.json`, patches only the Mach-O signature
      metadata/SuperBlobs and code-signature/stapler files, and asserts every output Mach-O
      binary matches its recorded `signed_sha256` byte-for-byte.
  - `verify-signed <signed.app>`
      Quick structural check (works on Linux and macOS) that <signed.app> carries a non-ad-hoc
      (CMS-signed) `LC_CODE_SIGNATURE` SuperBlob, `_CodeSignature/CodeResources`, and a stapled
      `Contents/CodeResources` ticket.
"""

from __future__ import annotations

import argparse
import gzip
import hashlib
import json
import os
import shutil
import struct
import sys
import tarfile
from pathlib import Path

FAT_MAGIC = 0xCAFEBABE
FAT_CIGAM = 0xBEBAFECA
FAT_MAGIC_64 = 0xCAFEBABF
FAT_CIGAM_64 = 0xBFBAFECA
MH_MAGIC_64 = 0xFEEDFACF
LC_SEGMENT_64 = 0x19
LC_CODE_SIGNATURE = 0x1D
CSMAGIC_EMBEDDED_SIGNATURE = 0xFADE0CC0
CSSLOT_SIGNATURESLOT = 0x00010000

CPU_NAMES = {
    0x01000007: "x86_64",
    0x0100000C: "arm64",
}

SKIP_BUNDLE_FILES = {
    "Contents/_CodeSignature/CodeResources",
    "Contents/CodeResources",
}


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def is_macho(path: Path) -> bool:
    if path.is_symlink() or not path.is_file():
        return False
    try:
        head = path.read_bytes()[:4]
    except OSError:
        return False
    if len(head) < 4:
        return False
    magic_be = struct.unpack(">I", head)[0]
    magic_le = struct.unpack("<I", head)[0]
    return magic_be in (FAT_MAGIC, FAT_MAGIC_64) or magic_le == MH_MAGIC_64


def parse_thin_slice(buf: bytes) -> dict:
    """Parse a 64-bit little-endian Mach-O slice and locate __LINKEDIT and LC_CODE_SIGNATURE."""
    if len(buf) < 32:
        raise ValueError("Mach-O slice too short")
    magic, cputype, cpusubtype, _ft, ncmds, sizeofcmds, _flags, _res = struct.unpack_from(
        "<8I", buf, 0
    )
    if magic != MH_MAGIC_64:
        raise ValueError(f"Unsupported Mach-O magic 0x{magic:08x} (expected 64-bit LE 0x{MH_MAGIC_64:08x})")
    if 32 + sizeofcmds > len(buf):
        raise ValueError("Mach-O load commands extend past slice end")

    linkedit_cmd_off = None
    linkedit_vmsize = None
    linkedit_filesize = None
    linkedit_fileoff = None
    codesig_cmd_off = None
    codesig_dataoff = None
    codesig_datasize = None

    off = 32
    for _ in range(ncmds):
        cmd, cmdsize = struct.unpack_from("<2I", buf, off)
        if cmdsize < 8 or off + cmdsize > 32 + sizeofcmds:
            raise ValueError(f"Invalid Mach-O load command size {cmdsize} at {off}")
        if cmd == LC_SEGMENT_64:
            segname = buf[off + 8 : off + 24].split(b"\x00", 1)[0].decode("ascii", errors="replace")
            if segname == "__LINKEDIT":
                _vmaddr, vmsize, fileoff, filesize = struct.unpack_from("<4Q", buf, off + 24)
                linkedit_cmd_off = off
                linkedit_vmsize = vmsize
                linkedit_fileoff = fileoff
                linkedit_filesize = filesize
        elif cmd == LC_CODE_SIGNATURE:
            dataoff, datasize = struct.unpack_from("<2I", buf, off + 8)
            codesig_cmd_off = off
            codesig_dataoff = dataoff
            codesig_datasize = datasize
        off += cmdsize

    if linkedit_cmd_off is None:
        raise ValueError("Mach-O slice has no __LINKEDIT segment")
    if codesig_cmd_off is None:
        raise ValueError("Mach-O slice has no LC_CODE_SIGNATURE load command")
    if codesig_dataoff + codesig_datasize > len(buf):
        raise ValueError("LC_CODE_SIGNATURE extends past slice end")

    # Zero the mutable signature-size header fields so we can hash every non-signature byte.
    normalized = bytearray(buf[:codesig_dataoff])
    struct.pack_into("<Q", normalized, linkedit_cmd_off + 32, 0)  # __LINKEDIT.vmsize
    struct.pack_into("<Q", normalized, linkedit_cmd_off + 48, 0)  # __LINKEDIT.filesize
    struct.pack_into("<I", normalized, codesig_cmd_off + 12, 0)   # LC_CODE_SIGNATURE.datasize

    # Check if there is any trailing padding after the signature blob inside the slice.
    tail = buf[codesig_dataoff + codesig_datasize :]
    if any(b != 0 for b in tail):
        raise ValueError("Non-zero bytes found after LC_CODE_SIGNATURE payload")

    return {
        "cputype": cputype,
        "cpusubtype": cpusubtype,
        "arch": CPU_NAMES.get(cputype, f"cpu_{cputype:x}"),
        "slice_size": len(buf),
        "linkedit_cmd_off": linkedit_cmd_off,
        "linkedit_fileoff": linkedit_fileoff,
        "linkedit_vmsize": linkedit_vmsize,
        "linkedit_filesize": linkedit_filesize,
        "codesig_cmd_off": codesig_cmd_off,
        "codesig_dataoff": codesig_dataoff,
        "codesig_datasize": codesig_datasize,
        "trailing_zeros": len(tail),
        "non_sig_sha256": sha256_bytes(bytes(normalized)),
        "cs_blob": buf[codesig_dataoff : codesig_dataoff + codesig_datasize],
    }


def parse_macho(buf: bytes) -> dict:
    """Parse a FAT or thin 64-bit Mach-O binary into its constituent slices."""
    if len(buf) < 8:
        raise ValueError("Mach-O binary too short")
    magic_be = struct.unpack_from(">I", buf, 0)[0]
    if magic_be == FAT_MAGIC:
        nfat = struct.unpack_from(">I", buf, 4)[0]
        slices = []
        for i in range(nfat):
            cputype, cpusubtype, offset, size, align = struct.unpack_from(">5I", buf, 8 + i * 20)
            s = parse_thin_slice(buf[offset : offset + size])
            s.update({"fat_offset": offset, "fat_size": size, "fat_align": align})
            slices.append(s)
        return {"format": "fat32", "slices": slices}
    if magic_be == FAT_MAGIC_64:
        nfat = struct.unpack_from(">I", buf, 4)[0]
        slices = []
        for i in range(nfat):
            cputype, cpusubtype, offset, size, align, _res = struct.unpack_from(
                ">2I2Q2I", buf, 8 + i * 32
            )
            s = parse_thin_slice(buf[offset : offset + size])
            s.update({"fat_offset": offset, "fat_size": size, "fat_align": align})
            slices.append(s)
        return {"format": "fat64", "slices": slices}
    return {"format": "thin", "slices": [parse_thin_slice(buf)]}


def has_cms_signature(cs_blob: bytes) -> bool:
    """Return True if a Mach-O LC_CODE_SIGNATURE SuperBlob contains a non-empty CMS slot."""
    if len(cs_blob) < 12:
        return False
    magic, length, count = struct.unpack_from(">3I", cs_blob, 0)
    if magic != CSMAGIC_EMBEDDED_SIGNATURE or length > len(cs_blob):
        return False
    for i in range(count):
        idx_off = 12 + i * 8
        if idx_off + 8 > len(cs_blob):
            return False
        slot_type, slot_off = struct.unpack_from(">2I", cs_blob, idx_off)
        if slot_type == CSSLOT_SIGNATURESLOT:
            if slot_off + 8 > len(cs_blob):
                return False
            _blob_magic, blob_len = struct.unpack_from(">2I", cs_blob, slot_off)
            # Ad-hoc signatures have an 8-byte empty BlobWrapper (blob_len == 8).
            return blob_len > 8
    return False


def patch_thin_slice(unsigned_slice: bytes, sig_meta: dict, cs_blob: bytes) -> bytes:
    parsed = parse_thin_slice(unsigned_slice)
    if parsed["non_sig_sha256"] != sig_meta["non_sig_sha256"]:
        raise ValueError(
            f"Slice {parsed['arch']} non-signature bytes differ from signed reference "
            f"(got {parsed['non_sig_sha256']}, expected {sig_meta['non_sig_sha256']})"
        )
    if parsed["codesig_dataoff"] != sig_meta["codesig_dataoff"]:
        raise ValueError(
            f"Slice {parsed['arch']} LC_CODE_SIGNATURE dataoff differs "
            f"(got {parsed['codesig_dataoff']}, expected {sig_meta['codesig_dataoff']})"
        )
    if len(cs_blob) != sig_meta["codesig_datasize"]:
        raise ValueError(
            f"Slice {parsed['arch']} detached signature blob length mismatch "
            f"(got {len(cs_blob)}, expected {sig_meta['codesig_datasize']})"
        )

    out = bytearray(unsigned_slice[: parsed["codesig_dataoff"]])
    struct.pack_into("<Q", out, parsed["linkedit_cmd_off"] + 32, sig_meta["linkedit_vmsize"])
    struct.pack_into("<Q", out, parsed["linkedit_cmd_off"] + 48, sig_meta["linkedit_filesize"])
    struct.pack_into("<I", out, parsed["codesig_cmd_off"] + 12, sig_meta["codesig_datasize"])
    out.extend(cs_blob)
    if sig_meta["trailing_zeros"]:
        out.extend(b"\x00" * sig_meta["trailing_zeros"])
    if len(out) != sig_meta["slice_size"]:
        raise ValueError(
            f"Patched slice {parsed['arch']} size {len(out)} != expected {sig_meta['slice_size']}"
        )
    return bytes(out)


def patch_macho(unsigned_bytes: bytes, meta: dict, sig_dir: Path) -> bytes:
    parsed = parse_macho(unsigned_bytes)
    if parsed["format"] != meta["format"]:
        raise ValueError(f"Mach-O format mismatch: {parsed['format']} != {meta['format']}")
    if len(parsed["slices"]) != len(meta["slices"]):
        raise ValueError("Mach-O slice count mismatch")

    patched_slices = []
    for u_slice, s_meta in zip(parsed["slices"], meta["slices"]):
        cs_blob = (sig_dir / s_meta["cs_file"]).read_bytes()
        if parsed["format"] == "thin":
            u_raw = unsigned_bytes
        else:
            u_raw = unsigned_bytes[u_slice["fat_offset"] : u_slice["fat_offset"] + u_slice["fat_size"]]
        patched_slices.append((s_meta, patch_thin_slice(u_raw, s_meta, cs_blob)))

    if parsed["format"] == "thin":
        out = patched_slices[0][1]
    elif parsed["format"] == "fat32":
        total_size = max(m["fat_offset"] + m["fat_size"] for m, _ in patched_slices)
        buf = bytearray(total_size)
        struct.pack_into(">2I", buf, 0, FAT_MAGIC, len(patched_slices))
        for i, (m, data) in enumerate(patched_slices):
            struct.pack_into(
                ">5I", buf, 8 + i * 20, m["cputype"], m["cpusubtype"], m["fat_offset"], m["fat_size"], m["fat_align"]
            )
            buf[m["fat_offset"] : m["fat_offset"] + len(data)] = data
        out = bytes(buf)
    else:
        total_size = max(m["fat_offset"] + m["fat_size"] for m, _ in patched_slices)
        buf = bytearray(total_size)
        struct.pack_into(">2I", buf, 0, FAT_MAGIC_64, len(patched_slices))
        for i, (m, data) in enumerate(patched_slices):
            struct.pack_into(
                ">2I2Q2I",
                buf,
                8 + i * 32,
                m["cputype"],
                m["cpusubtype"],
                m["fat_offset"],
                m["fat_size"],
                m["fat_align"],
                0,
            )
            buf[m["fat_offset"] : m["fat_offset"] + len(data)] = data
        out = bytes(buf)

    got_sha = sha256_bytes(out)
    if got_sha != meta["signed_sha256"]:
        raise ValueError(
            f"Re-attached Mach-O SHA-256 mismatch: got {got_sha}, expected {meta['signed_sha256']}"
        )
    return out


def walk_bundle(root: Path) -> dict[str, Path]:
    files: dict[str, Path] = {}
    for p in sorted(root.rglob("*")):
        if p.is_file() or p.is_symlink():
            rel = p.relative_to(root).as_posix()
            files[rel] = p
    return files


def cmd_extract(args: argparse.Namespace) -> None:
    unsigned_app = Path(args.unsigned_app)
    signed_app = Path(args.signed_app)
    sigs_dir = Path(args.sigs_dir)

    if not unsigned_app.is_dir() or not signed_app.is_dir():
        raise SystemExit("extract: both unsigned_app and signed_app must be .app directories")

    u_files = walk_bundle(unsigned_app)
    s_files = walk_bundle(signed_app)

    if "Contents/_CodeSignature/CodeResources" not in s_files:
        raise SystemExit("extract: signed .app is missing Contents/_CodeSignature/CodeResources")
    if not args.allow_unstapled and "Contents/CodeResources" not in s_files:
        raise SystemExit(
            "extract: signed .app is missing stapled ticket Contents/CodeResources "
            "(run `xcrun stapler staple` first, or pass --allow-unstapled for tests)"
        )

    u_non_sig = set(u_files) - SKIP_BUNDLE_FILES
    s_non_sig = set(s_files) - SKIP_BUNDLE_FILES
    if u_non_sig != s_non_sig:
        extra = sorted(s_non_sig - u_non_sig)
        missing = sorted(u_non_sig - s_non_sig)
        raise SystemExit(f"extract: bundle file list differs (extra={extra}, missing={missing})")

    if sigs_dir.exists():
        shutil.rmtree(sigs_dir)
    sigs_dir.mkdir(parents=True)

    # Copy bundle-level code signature and stapled ticket files.
    bundle_sig_files = []
    for rel in sorted(SKIP_BUNDLE_FILES):
        if rel in s_files:
            dst = sigs_dir / rel
            dst.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(s_files[rel], dst)
            bundle_sig_files.append({"path": rel, "sha256": sha256_file(dst)})

    unchanged_files: dict[str, str] = {}
    macho_entries = []

    for rel in sorted(u_non_sig):
        up = u_files[rel]
        sp = s_files[rel]
        if up.is_symlink() or sp.is_symlink():
            if not (up.is_symlink() and sp.is_symlink() and os.readlink(up) == os.readlink(sp)):
                raise SystemExit(f"extract: symlink differs at {rel}")
            continue

        if is_macho(up):
            u_bytes = up.read_bytes()
            s_bytes = sp.read_bytes()
            u_parsed = parse_macho(u_bytes)
            s_parsed = parse_macho(s_bytes)
            if u_parsed["format"] != s_parsed["format"] or len(u_parsed["slices"]) != len(s_parsed["slices"]):
                raise SystemExit(f"extract: Mach-O format/slice count changed for {rel}")

            slice_metas = []
            for idx, (us, ss) in enumerate(zip(u_parsed["slices"], s_parsed["slices"])):
                if us["cputype"] != ss["cputype"] or us["cpusubtype"] != ss["cpusubtype"]:
                    raise SystemExit(f"extract: slice {idx} architecture changed for {rel}")
                if us["non_sig_sha256"] != ss["non_sig_sha256"]:
                    raise SystemExit(
                        f"extract: non-signature code/data bytes changed in {rel} ({us['arch']}); refusing"
                    )
                if not args.allow_adhoc and not has_cms_signature(ss["cs_blob"]):
                    raise SystemExit(
                        f"extract: {rel} ({ss['arch']}) appears ad-hoc signed (no CMS signature in LC_CODE_SIGNATURE)"
                    )
                cs_rel = f"{rel}.{ss['arch']}.cs"
                cs_dst = sigs_dir / cs_rel
                cs_dst.parent.mkdir(parents=True, exist_ok=True)
                cs_dst.write_bytes(ss["cs_blob"])
                meta = {k: v for k, v in ss.items() if k != "cs_blob"}
                meta["cs_file"] = cs_rel
                meta["cs_sha256"] = sha256_bytes(ss["cs_blob"])
                slice_metas.append(meta)

            macho_meta = {
                "path": rel,
                "format": s_parsed["format"],
                "unsigned_sha256": sha256_bytes(u_bytes),
                "signed_sha256": sha256_bytes(s_bytes),
                "slices": slice_metas,
            }
            macho_entries.append(macho_meta)
        else:
            u_sha = sha256_file(up)
            s_sha = sha256_file(sp)
            if u_sha != s_sha:
                raise SystemExit(f"extract: non-Mach-O file {rel} modified during signing ({u_sha} != {s_sha})")
            unchanged_files[rel] = u_sha

    if not macho_entries:
        raise SystemExit("extract: no Mach-O binaries found in .app bundle")

    manifest = {
        "schema": 1,
        "bundle_sig_files": bundle_sig_files,
        "macho_binaries": macho_entries,
        "unchanged_files": unchanged_files,
    }
    (sigs_dir / "manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    print(
        f"macos-detached-sig: extracted {len(macho_entries)} Mach-O binary signature(s) and "
        f"{len(bundle_sig_files)} bundle signature file(s) into {sigs_dir}"
    )


def cmd_apply(args: argparse.Namespace) -> None:
    unsigned_app = Path(args.unsigned_app)
    sigs_dir = Path(args.sigs_dir)
    out_app = Path(args.out_app)

    manifest_path = sigs_dir / "manifest.json"
    if not manifest_path.is_file():
        raise SystemExit(f"apply: missing {manifest_path}")
    manifest = json.loads(manifest_path.read_text())
    if manifest.get("schema") != 1:
        raise SystemExit(f"apply: unsupported manifest schema {manifest.get('schema')}")

    u_files = walk_bundle(unsigned_app)
    expected_unchanged = manifest["unchanged_files"]
    expected_macho = {m["path"]: m for m in manifest["macho_binaries"]}

    # Verify every regular non-signature file in unsigned_app matches the manifest.
    for rel, want_sha in expected_unchanged.items():
        if rel not in u_files:
            raise SystemExit(f"apply: unsigned .app is missing expected file {rel}")
        got_sha = sha256_file(u_files[rel])
        if got_sha != want_sha:
            raise SystemExit(f"apply: unsigned .app file {rel} hash mismatch ({got_sha} != {want_sha})")

    for rel, m in expected_macho.items():
        if rel not in u_files:
            raise SystemExit(f"apply: unsigned .app is missing expected Mach-O binary {rel}")
        got_sha = sha256_file(u_files[rel])
        if got_sha != m["unsigned_sha256"]:
            raise SystemExit(
                f"apply: unsigned Mach-O {rel} hash mismatch ({got_sha} != {m['unsigned_sha256']}); "
                "detached signature does not match this unsigned build"
            )

    if out_app.exists():
        shutil.rmtree(out_app)
    shutil.copytree(unsigned_app, out_app, symlinks=True)

    # Remove any ad-hoc _CodeSignature before installing signed files.
    for rel in SKIP_BUNDLE_FILES:
        p = out_app / rel
        if p.exists():
            p.unlink()

    for rel, m in expected_macho.items():
        target = out_app / rel
        mode = target.stat().st_mode
        patched = patch_macho(target.read_bytes(), m, sigs_dir)
        target.write_bytes(patched)
        os.chmod(target, mode)

    for entry in manifest["bundle_sig_files"]:
        rel = entry["path"]
        src = sigs_dir / rel
        if not src.is_file():
            raise SystemExit(f"apply: missing detached bundle signature file {src}")
        got_sha = sha256_file(src)
        if got_sha != entry["sha256"]:
            raise SystemExit(f"apply: detached signature file {rel} corrupted ({got_sha} != {entry['sha256']})")
        dst = out_app / rel
        dst.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(src, dst)

    print(f"macos-detached-sig: applied detached signatures to {out_app} (hashes verified)")


def cmd_verify_signed(args: argparse.Namespace) -> None:
    app = Path(args.signed_app)
    if not (app / "Contents/_CodeSignature/CodeResources").is_file():
        raise SystemExit("verify-signed: missing Contents/_CodeSignature/CodeResources")
    if not args.allow_unstapled and not (app / "Contents/CodeResources").is_file():
        raise SystemExit("verify-signed: missing stapled ticket Contents/CodeResources")

    checked = 0
    for rel, path in walk_bundle(app).items():
        if rel in SKIP_BUNDLE_FILES or not is_macho(path):
            continue
        parsed = parse_macho(path.read_bytes())
        for s in parsed["slices"]:
            if not has_cms_signature(s["cs_blob"]):
                raise SystemExit(
                    f"verify-signed: {rel} ({s['arch']}) has no CMS signature (ad-hoc or unsigned)"
                )
        checked += 1
    if checked == 0:
        raise SystemExit("verify-signed: no Mach-O binaries found in bundle")
    print(f"macos-detached-sig: verified CMS code signatures on {checked} Mach-O binary(ies)")


def cmd_pack_app(args: argparse.Namespace) -> None:
    """Create a deterministic .app.tar.gz (identical on macOS and Linux) for the Tauri updater."""
    app = Path(args.app_dir)
    out = Path(args.out_tar_gz)
    if not app.is_dir():
        raise SystemExit(f"pack-app: {app} is not a directory")
    out.parent.mkdir(parents=True, exist_ok=True)
    app_name = app.name

    def norm_info(ti: tarfile.TarInfo) -> tarfile.TarInfo:
        ti.uid = 0
        ti.gid = 0
        ti.uname = ""
        ti.gname = ""
        ti.mtime = 0
        return ti

    entries = [app] + sorted(app.rglob("*"), key=lambda p: p.relative_to(app).as_posix())
    with out.open("wb") as raw, gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as gz:
        with tarfile.open(fileobj=gz, mode="w", format=tarfile.USTAR_FORMAT) as tf:
            for p in entries:
                arcname = app_name if p == app else f"{app_name}/{p.relative_to(app).as_posix()}"
                ti = tf.gettarinfo(str(p), arcname=arcname)
                ti = norm_info(ti)
                if p.is_file() and not p.is_symlink():
                    with p.open("rb") as f:
                        tf.addfile(ti, f)
                else:
                    tf.addfile(ti)
    print(f"macos-detached-sig: packed deterministic {out} ({sha256_file(out)})")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="cmd", required=True)

    p_ext = sub.add_parser("extract", help="Extract detached signatures from a signed .app")
    p_ext.add_argument("unsigned_app")
    p_ext.add_argument("signed_app")
    p_ext.add_argument("sigs_dir")
    p_ext.add_argument("--allow-unstapled", action="store_true", help="Allow missing Contents/CodeResources (tests)")
    p_ext.add_argument("--allow-adhoc", action="store_true", help="Allow non-CMS signatures (tests)")
    p_ext.set_defaults(func=cmd_extract)

    p_app = sub.add_parser("apply", help="Apply detached signatures onto an unsigned .app")
    p_app.add_argument("unsigned_app")
    p_app.add_argument("sigs_dir")
    p_app.add_argument("out_app")
    p_app.set_defaults(func=cmd_apply)

    p_ver = sub.add_parser("verify-signed", help="Verify a signed .app carries CMS signatures and stapled ticket")
    p_ver.add_argument("signed_app")
    p_ver.add_argument("--allow-unstapled", action="store_true")
    p_ver.set_defaults(func=cmd_verify_signed)

    p_pack = sub.add_parser("pack-app", help="Create a deterministic .app.tar.gz from a .app bundle")
    p_pack.add_argument("app_dir")
    p_pack.add_argument("out_tar_gz")
    p_pack.set_defaults(func=cmd_pack_app)

    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
