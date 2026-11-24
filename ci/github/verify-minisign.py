#!/usr/bin/env python3
"""Verify Tauri updater latest.json minisign signatures against a public key and version.

Uses only Python 3 stdlib + `openssl` (Ed25519 via `openssl pkeyutl -verify -rawin`), so it
runs identically on macOS and Linux with zero extra packages. Enforces `requireSignedVersion`:
every platform signature's trusted comment must carry `version:<VERSION>`.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import subprocess
import tempfile
from pathlib import Path
from urllib.parse import urlparse

ED25519_SPKI_PREFIX = bytes.fromhex("302a300506032b6570032100")

EXPECTED_PLATFORM_FILES = {
    "darwin-aarch64-app": "dash-wallet-{version}-macos-universal.app.tar.gz",
    "darwin-x86_64-app": "dash-wallet-{version}-macos-universal.app.tar.gz",
    "linux-x86_64-appimage": "dash-wallet-{version}-linux-x86_64.AppImage",
    "windows-x86_64-nsis": "dash-wallet-{version}-windows-x64-setup.exe",
}
EXPECTED_PLATFORMS = tuple(EXPECTED_PLATFORM_FILES.keys())


def decode_minisign_pubkey(b64_pubkey: str) -> tuple[bytes, bytes]:
    raw_text = base64.b64decode(b64_pubkey.strip()).decode("utf-8")
    lines = [line.strip() for line in raw_text.splitlines() if line.strip()]
    if len(lines) < 2 or not lines[0].startswith("untrusted comment:"):
        raise ValueError("Invalid minisign public key format")
    key_bytes = base64.b64decode(lines[1])
    if len(key_bytes) != 42 or key_bytes[:2] != b"Ed":
        raise ValueError(f"Unsupported minisign public key header (len={len(key_bytes)})")
    key_id = key_bytes[2:10]
    pub_ed25519 = key_bytes[10:42]
    return key_id, pub_ed25519


def decode_minisign_sig(b64_sig: str) -> tuple[bytes, bytes, bytes, str, bytes]:
    raw_text = base64.b64decode(b64_sig.strip()).decode("utf-8")
    lines = [line.rstrip("\r\n") for line in raw_text.splitlines() if line.strip()]
    if len(lines) != 4 or not lines[0].startswith("untrusted comment:") or not lines[2].startswith("trusted comment:"):
        raise ValueError("Invalid minisign signature box format")
    sig_line = base64.b64decode(lines[1])
    if len(sig_line) != 74:
        raise ValueError(f"Invalid minisign signature length {len(sig_line)}")
    alg = sig_line[:2]
    if alg not in (b"ED", b"Ed"):
        raise ValueError(f"Unsupported minisign algorithm {alg!r}")
    key_id = sig_line[2:10]
    sig_bytes = sig_line[10:74]
    trusted_comment = lines[2][len("trusted comment:") :].lstrip(" ")
    global_sig = base64.b64decode(lines[3])
    if len(global_sig) != 64:
        raise ValueError(f"Invalid minisign global signature length {len(global_sig)}")
    return alg, key_id, sig_bytes, trusted_comment, global_sig


def ed25519_verify(pub_ed25519: bytes, message: bytes, signature: bytes) -> bool:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = Path(tmp)
        pub_der = tmp_path / "pub.der"
        msg_file = tmp_path / "msg.bin"
        sig_file = tmp_path / "sig.bin"
        pub_der.write_bytes(ED25519_SPKI_PREFIX + pub_ed25519)
        msg_file.write_bytes(message)
        sig_file.write_bytes(signature)
        res = subprocess.run(
            [
                "openssl",
                "pkeyutl",
                "-verify",
                "-pubin",
                "-inkey",
                str(pub_der),
                "-keyform",
                "DER",
                "-rawin",
                "-in",
                str(msg_file),
                "-sigfile",
                str(sig_file),
            ],
            capture_output=True,
            check=False,
        )
        return res.returncode == 0


def parse_trusted_comment_version(trusted_comment: str) -> str | None:
    for part in trusted_comment.split("\t"):
        if part.startswith("version:"):
            return part[len("version:") :]
    return None


def verify_file_signature(
    pub_key_id: bytes,
    pub_ed25519: bytes,
    file_path: Path,
    b64_sig: str,
    want_version: str,
) -> None:
    alg, sig_key_id, sig_bytes, trusted_comment, global_sig = decode_minisign_sig(b64_sig)
    if sig_key_id != pub_key_id:
        raise ValueError(
            f"{file_path.name}: signature key ID {sig_key_id[::-1].hex().upper()} "
            f"!= expected {pub_key_id[::-1].hex().upper()}"
        )
    if alg == b"ED":
        h = hashlib.blake2b(digest_size=64)
        with file_path.open("rb") as f:
            for chunk in iter(lambda: f.read(1 << 20), b""):
                h.update(chunk)
        payload = h.digest()
    else:
        payload = file_path.read_bytes()

    if not ed25519_verify(pub_ed25519, payload, sig_bytes):
        raise ValueError(f"{file_path.name}: minisign artifact signature verification failed")

    global_msg = sig_bytes + trusted_comment.encode("utf-8")
    if not ed25519_verify(pub_ed25519, global_msg, global_sig):
        raise ValueError(f"{file_path.name}: minisign global signature over trusted comment failed")

    signed_version = parse_trusted_comment_version(trusted_comment)
    if signed_version != want_version:
        raise ValueError(
            f"{file_path.name}: signed trusted comment version {signed_version!r} != expected {want_version!r}"
        )


def find_file(dirs: list[Path], filename: str) -> Path:
    for d in dirs:
        candidate = d / filename
        if candidate.is_file():
            return candidate
    raise FileNotFoundError(f"Could not find {filename} in {[str(d) for d in dirs]}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--pubkey-from-tauri", help="Path to tauri.conf.json")
    group.add_argument("--pubkey", help="Base64-encoded minisign public key")
    parser.add_argument("--latest-json", required=True, help="Path to latest.json")
    parser.add_argument("--version", required=True, help="Expected release version (X.Y.Z)")
    parser.add_argument(
        "--files-dir",
        action="append",
        required=True,
        help="Directory containing signed updater artifacts (may be passed multiple times)",
    )
    parser.add_argument("--repo", help="Optional expected GitHub owner/repo in download URLs")
    args = parser.parse_args()

    if args.pubkey_from_tauri:
        conf = json.loads(Path(args.pubkey_from_tauri).read_text())
        pubkey_b64 = conf["plugins"]["updater"]["pubkey"]
    else:
        pubkey_b64 = args.pubkey

    pub_key_id, pub_ed25519 = decode_minisign_pubkey(pubkey_b64)
    latest = json.loads(Path(args.latest_json).read_text())

    if latest.get("version") != args.version:
        raise SystemExit(
            f"verify-minisign: latest.json version {latest.get('version')!r} != expected {args.version!r}"
        )

    platforms = latest.get("platforms")
    if not isinstance(platforms, dict) or sorted(platforms.keys()) != sorted(EXPECTED_PLATFORMS):
        raise SystemExit(
            f"verify-minisign: latest.json platforms {sorted(platforms.keys()) if isinstance(platforms, dict) else platforms!r} "
            f"!= expected {list(EXPECTED_PLATFORMS)}"
        )

    url_prefix = f"https://github.com/{args.repo}/releases/download/v{args.version}/" if args.repo else None
    search_dirs = [Path(d) for d in args.files_dir]

    for plat in EXPECTED_PLATFORMS:
        entry = platforms[plat]
        url = entry.get("url", "")
        sig = entry.get("signature", "")
        expected_filename = EXPECTED_PLATFORM_FILES[plat].format(version=args.version)
        if url_prefix:
            expected_url = f"{url_prefix}{expected_filename}"
            if url != expected_url:
                raise SystemExit(f"verify-minisign: {plat} url {url!r} != expected {expected_url!r}")
        else:
            parsed_url = urlparse(url)
            if ".." in parsed_url.path.split("/") or Path(parsed_url.path).name != expected_filename:
                raise SystemExit(
                    f"verify-minisign: {plat} url {url!r} does not match expected filename {expected_filename!r}"
                )
        file_path = find_file(search_dirs, expected_filename)
        try:
            verify_file_signature(pub_key_id, pub_ed25519, file_path, sig, args.version)
        except ValueError as e:
            raise SystemExit(f"verify-minisign: {plat}: {e}") from None

    print(
        f"verify-minisign: verified all {len(EXPECTED_PLATFORMS)} platform signatures in "
        f"{args.latest_json} for version {args.version} (key ID {pub_key_id[::-1].hex().upper()})"
    )


if __name__ == "__main__":
    main()
