#!/usr/bin/env python3
"""Verify Tauri updater latest.json minisign signatures against a public key and version.

Uses only Python 3 stdlib (pure-Python RFC 8032 Ed25519 verification + hashlib SHA-512 /
BLAKE2b-512), so it runs identically on macOS (regardless of LibreSSL / OpenSSL 1.1.1w)
and Linux with zero external CLI dependencies. Enforces `requireSignedVersion`: every
platform signature's trusted comment must carry `version:<VERSION>`, pins exact platform
URLs, and refuses retired CI updater key IDs.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
from pathlib import Path

# Retired / dead CI updater key IDs and Ed25519 public-key SHA-256 fingerprints that must NEVER sign a stable feed.
RETIRED_UPDATER_KEY_IDS = frozenset({
    "ED322B963B527B21",  # former GitHub Actions `release` environment key (PK-03)
    "7A4629E8E5438F3C",  # earlier dead test key
})
RETIRED_UPDATER_PUBKEY_SHA256 = frozenset({
    # SHA-256 of the 32-byte Ed25519 public key for retired CI key ED322B963B527B21
    "7821a52f7420d38804e03530e867ed5b2a50402b5c75ba9751ed9746fc8dc689",
})

# RFC 8032 Ed25519 curve parameters (verification only; public values).
_P = (1 << 255) - 19
_L = (1 << 252) + 27742317777372353535851937790883648493
_D = (-121665 * pow(121666, _P - 2, _P)) % _P
_I = pow(2, (_P - 1) // 4, _P)


def _ed_recover_x(y: int, sign: int) -> int | None:
    if y >= _P:
        return None
    u = (y * y - 1) % _P
    v = (_D * y * y + 1) % _P
    v3 = (v * v * v) % _P
    v7 = (v3 * v3 * v) % _P
    x = (u * v3 * pow((u * v7) % _P, (_P - 5) // 8, _P)) % _P
    vx2 = (v * x * x) % _P
    if vx2 == u:
        pass
    elif vx2 == (-u) % _P:
        x = (x * _I) % _P
    else:
        return None
    if x == 0 and sign == 1:
        return None
    if (x & 1) != sign:
        x = _P - x
    return x


def _ed_decode_point(enc: bytes) -> tuple[int, int, int, int] | None:
    if len(enc) != 32:
        return None
    y = int.from_bytes(enc, "little")
    sign = y >> 255
    y &= (1 << 255) - 1
    x = _ed_recover_x(y, sign)
    if x is None:
        return None
    return (x, y, 1, (x * y) % _P)


def _ed_point_add(
    p1: tuple[int, int, int, int], p2: tuple[int, int, int, int]
) -> tuple[int, int, int, int]:
    x1, y1, z1, t1 = p1
    x2, y2, z2, t2 = p2
    a = ((y1 - x1) * (y2 - x2)) % _P
    b = ((y1 + x1) * (y2 + x2)) % _P
    c = (2 * _D * t1 * t2) % _P
    d = (2 * z1 * z2) % _P
    e = (b - a) % _P
    f = (d - c) % _P
    g = (d + c) % _P
    h = (b + a) % _P
    return ((e * f) % _P, (g * h) % _P, (f * g) % _P, (e * h) % _P)


def _ed_scalar_mul(pt: tuple[int, int, int, int], s: int) -> tuple[int, int, int, int]:
    res = (0, 1, 1, 0)
    addend = pt
    while s > 0:
        if s & 1:
            res = _ed_point_add(res, addend)
        addend = _ed_point_add(addend, addend)
        s >>= 1
    return res


_BY = (4 * pow(5, _P - 2, _P)) % _P
_BX = _ed_recover_x(_BY, 0)
assert _BX is not None
_B = (_BX, _BY, 1, (_BX * _BY) % _P)

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
    if (
        len(lines) != 4
        or not lines[0].startswith("untrusted comment:")
        or not lines[2].startswith("trusted comment: ")
    ):
        raise ValueError("Invalid minisign signature box format")
    sig_line = base64.b64decode(lines[1])
    if len(sig_line) != 74:
        raise ValueError(f"Invalid minisign signature length {len(sig_line)}")
    alg = sig_line[:2]
    if alg not in (b"ED", b"Ed"):
        raise ValueError(f"Unsupported minisign algorithm {alg!r}")
    key_id = sig_line[2:10]
    sig_bytes = sig_line[10:74]
    trusted_comment = lines[2][len("trusted comment: ") :]
    global_sig = base64.b64decode(lines[3])
    if len(global_sig) != 64:
        raise ValueError(f"Invalid minisign global signature length {len(global_sig)}")
    return alg, key_id, sig_bytes, trusted_comment, global_sig


def _ed_is_identity(pt: tuple[int, int, int, int]) -> bool:
    return (pt[0] % _P == 0) and ((pt[1] - pt[2]) % _P == 0)


def _ed_is_prime_order(pt: tuple[int, int, int, int]) -> bool:
    """True iff pt lies in the prime-order L subgroup and is not small-order (8*pt != O and L*pt == O)."""
    if _ed_is_identity(_ed_scalar_mul(pt, 8)):
        return False
    return _ed_is_identity(_ed_scalar_mul(pt, _L))


def validate_updater_pubkey(pubkey_b64: str, source: str = "pubkey") -> tuple[bytes, bytes]:
    """Decode a minisign Ed25519 pubkey, enforce prime-order subgroup membership, and reject retired CI keys."""
    pub_key_id, pub_ed25519 = decode_minisign_pubkey(pubkey_b64)
    a_pt = _ed_decode_point(pub_ed25519)
    if a_pt is None or not _ed_is_prime_order(a_pt):
        raise SystemExit(
            f"verify-minisign: refusing non-prime-order or small-order Ed25519 public key in {source} "
            "(public key must satisfy [8]A != O and [L]A == O)"
        )
    key_id_hex = pub_key_id[::-1].hex().upper()
    pub_sha256 = hashlib.sha256(pub_ed25519).hexdigest()
    if key_id_hex in RETIRED_UPDATER_KEY_IDS or pub_sha256 in RETIRED_UPDATER_PUBKEY_SHA256:
        raise SystemExit(
            f"verify-minisign: refusing retired updater key (key ID {key_id_hex}, "
            f"ed25519 sha256:{pub_sha256[:16]}...) in {source} "
            "(rotate to a fresh local-only stable updater key in tauri.conf.json before tagging 1.0)"
        )
    return pub_key_id, pub_ed25519


def load_pubkey_from_tauri(conf_path: Path, extra_configs: list[str] | None = None) -> str:
    """Read plugins.updater.pubkey from tauri.conf.json and refuse pubkey overrides in any overlay config."""
    if not conf_path.is_file():
        raise SystemExit(f"verify-minisign: missing tauri.conf.json at {conf_path}")
    conf = json.loads(conf_path.read_text())
    pubkey_b64 = conf.get("plugins", {}).get("updater", {}).get("pubkey", "")
    if not isinstance(pubkey_b64, str) or not pubkey_b64.strip():
        raise SystemExit(f"verify-minisign: missing plugins.updater.pubkey in {conf_path}")

    # L1: Refuse plugins.updater.pubkey in any sibling Tauri overlay/platform config (tauri.*.conf.json)
    # or extra merged --config file/JSON so no overlay can smuggle an unchecked or retired key.
    for sib in sorted(conf_path.parent.glob("tauri.*.conf.json")):
        if sib.resolve() == conf_path.resolve() or not sib.is_file():
            continue
        sib_data = json.loads(sib.read_text())
        if isinstance(sib_data, dict) and "pubkey" in sib_data.get("plugins", {}).get("updater", {}):
            raise SystemExit(
                f"verify-minisign: refusing plugins.updater.pubkey override in overlay config {sib} "
                f"(keep plugins.updater.pubkey exclusively in {conf_path.name})"
            )
    for extra in extra_configs or []:
        extra_str = extra.strip()
        if extra_str.startswith("{"):
            extra_data = json.loads(extra_str)
            label = "inline --config JSON"
        else:
            p = Path(extra)
            if not p.is_file():
                continue
            if p.resolve() == conf_path.resolve():
                continue
            extra_data = json.loads(p.read_text())
            label = str(p)
        if isinstance(extra_data, dict) and "pubkey" in extra_data.get("plugins", {}).get("updater", {}):
            raise SystemExit(
                f"verify-minisign: refusing plugins.updater.pubkey override in {label} "
                f"(keep plugins.updater.pubkey exclusively in {conf_path.name})"
            )
    return pubkey_b64


def ed25519_verify(pub_ed25519: bytes, message: bytes, signature: bytes) -> bool:
    """RFC 8032 §5.1.7 strict Ed25519 verification with prime-order pubkey check (S < L, [S]B == R + [k]A)."""
    if len(pub_ed25519) != 32 or len(signature) != 64:
        return False
    a_pt = _ed_decode_point(pub_ed25519)
    r_pt = _ed_decode_point(signature[:32])
    if a_pt is None or r_pt is None:
        return False
    if not _ed_is_prime_order(a_pt):
        return False
    s = int.from_bytes(signature[32:], "little")
    if s >= _L:
        return False
    k = int.from_bytes(hashlib.sha512(signature[:32] + pub_ed25519 + message).digest(), "little") % _L
    lhs = _ed_scalar_mul(_B, s)
    rhs = _ed_point_add(r_pt, _ed_scalar_mul(a_pt, k))
    return (
        (lhs[0] * rhs[2] - rhs[0] * lhs[2]) % _P == 0
        and (lhs[1] * rhs[2] - rhs[1] * lhs[2]) % _P == 0
    )


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
    parser.add_argument(
        "--extra-config",
        action="append",
        default=[],
        help="Optional extra Tauri --config file path or JSON string to check for pubkey overrides",
    )
    parser.add_argument(
        "--check-key-only",
        action="store_true",
        help="Validate the updater public key (format, prime-order subgroup, non-retired, no overlay overrides) and exit",
    )
    parser.add_argument("--latest-json", help="Path to latest.json")
    parser.add_argument("--version", help="Expected release version (X.Y.Z)")
    parser.add_argument(
        "--files-dir",
        action="append",
        default=[],
        help="Directory containing signed updater artifacts (may be passed multiple times)",
    )
    parser.add_argument("--repo", help="Expected GitHub owner/repo in download URLs (required unless --check-key-only)")
    args = parser.parse_args()

    if args.pubkey_from_tauri:
        source = args.pubkey_from_tauri
        pubkey_b64 = load_pubkey_from_tauri(Path(args.pubkey_from_tauri), args.extra_config)
    else:
        source = "--pubkey"
        pubkey_b64 = args.pubkey

    pub_key_id, pub_ed25519 = validate_updater_pubkey(pubkey_b64, source)
    if args.check_key_only:
        print(f"verify-minisign: validated updater public key (key ID {pub_key_id[::-1].hex().upper()})")
        return

    if not args.latest_json or not args.version or not args.files_dir or not args.repo:
        parser.error("--latest-json, --version, --files-dir, and --repo are required unless --check-key-only is set")

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

    url_prefix = f"https://github.com/{args.repo}/releases/download/v{args.version}/"
    search_dirs = [Path(d) for d in args.files_dir]

    for plat in EXPECTED_PLATFORMS:
        entry = platforms[plat]
        url = entry.get("url", "")
        sig = entry.get("signature", "")
        expected_filename = EXPECTED_PLATFORM_FILES[plat].format(version=args.version)
        expected_url = f"{url_prefix}{expected_filename}"
        if url != expected_url:
            raise SystemExit(f"verify-minisign: {plat} url {url!r} != expected {expected_url!r}")
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
