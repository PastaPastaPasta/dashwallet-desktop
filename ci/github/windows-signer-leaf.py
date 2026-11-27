#!/usr/bin/env python3
"""Find the leaf certificate a pinned Windows signer identity selects in a detached Authenticode signature.

  windows-signer-leaf.py <signature.pem> <organization> <EKU OID>

Prints `sha256:<64 hex>` of the one certificate in the PKCS#7 that is not a CA, names exactly that
organization (its subject's one O attribute), and lists both Code Signing and the given OID as extended
key usages. Exits 1 when no
certificate matches, or more than one does. This only selects a certificate: release-attach.sh then has
osslsigncode require that leaf as the signer and verify its chain to the pinned roots.
"""
import re
import subprocess
import sys

CODE_SIGNING = {"Code Signing", "1.3.6.1.5.5.7.3.3"}


def openssl(args, data):
    return subprocess.run(["openssl", *args], input=data, capture_output=True, check=True).stdout


def main():
    if len(sys.argv) != 4:
        raise SystemExit(__doc__)
    sig_path, want_org, want_eku = sys.argv[1:4]
    with open(sig_path, "rb") as f:
        bundle = openssl(["pkcs7", "-print_certs"], f.read()).decode("ascii", "replace")
    certs = re.findall(r"-----BEGIN CERTIFICATE-----.+?-----END CERTIFICATE-----\n?", bundle, re.S)
    matches = set()
    for pem in certs:
        raw = pem.encode()
        subject = openssl(["x509", "-noout", "-subject", "-nameopt", "multiline,utf8,-esc_msb"], raw).decode("utf-8", "replace")
        orgs = re.findall(r"^\s*organizationName\s*= ?(.*)$", subject, re.M)
        text = openssl(["x509", "-noout", "-text"], raw).decode("utf-8", "replace")
        if re.search(r"^\s*CA:TRUE", text, re.M):
            continue
        eku = re.search(r"X509v3 Extended Key Usage:[^\n]*\n\s*([^\n]+)", text)
        usages = {u.strip() for u in eku.group(1).split(",")} if eku else set()
        if orgs == [want_org] and want_eku in usages and usages & CODE_SIGNING:
            fp = openssl(["x509", "-noout", "-fingerprint", "-sha256"], raw).decode()
            matches.add(fp.strip().split("=", 1)[1].replace(":", "").lower())
    if len(matches) != 1:
        print(f"windows-signer-leaf: {len(matches)} of {len(certs)} certificates match, need exactly 1", file=sys.stderr)
        return 1
    print("sha256:" + matches.pop())
    return 0


if __name__ == "__main__":
    sys.exit(main())
