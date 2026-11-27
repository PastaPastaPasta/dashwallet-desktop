#!/usr/bin/env python3
"""Find the leaf certificate a pinned Windows signer identity selects in a detached Authenticode signature.

  windows-signer-leaf.py <signature.pem> <organization> <EKU OID>

Prints `sha256:<64 hex>` of the one certificate in the PKCS#7 that is not a CA, names exactly that
organization (its subject's one O attribute), and lists both Code Signing and the given OID as extended
key usages. Exits 1 when no certificate matches, or more than one does. This only selects a certificate:
release-attach.sh then has osslsigncode require that leaf as the signer and verify its chain to the pinned
roots.

The certificates are read from their encoding, never from text a tool printed: text can be forged by a
string inside a certificate. An indefinite length, a multi-byte tag, a repeated extension or trailing data
refuses the whole signature. This is not a strict DER validator: osslsigncode parses the same bytes again,
and it is the hash of exactly these bytes that it requires of the signer.
"""
import base64
import hashlib
import re
import sys

OID_SIGNED_DATA = "1.2.840.113549.1.7.2"
OID_ORGANIZATION = "2.5.4.10"
OID_BASIC_CONSTRAINTS = "2.5.29.19"
OID_EXT_KEY_USAGE = "2.5.29.37"
OID_CODE_SIGNING = "1.3.6.1.5.5.7.3.3"

SEQUENCE, SET, OID, OCTET_STRING, BOOLEAN = 0x30, 0x31, 0x06, 0x04, 0x01
CONTEXT_0, CONTEXT_3 = 0xA0, 0xA3
STRING_CODECS = {0x0C: "utf-8", 0x13: "ascii", 0x16: "ascii", 0x14: "latin-1", 0x1E: "utf-16-be", 0x1C: "utf-32-be"}


class Malformed(Exception):
    pass


def read(data, pos):
    """One DER element at data[pos:]: (tag, content, end)."""
    if pos + 2 > len(data):
        raise Malformed("truncated element")
    tag, first = data[pos], data[pos + 1]
    if tag & 0x1F == 0x1F:
        raise Malformed("multi-byte tag")
    pos += 2
    if first < 0x80:
        length = first
    else:
        n = first & 0x7F
        if n == 0 or n > 4 or pos + n > len(data):
            raise Malformed("indefinite or oversized length")
        length = int.from_bytes(data[pos:pos + n], "big")
        pos += n
    if pos + length > len(data):
        raise Malformed("element runs past its parent")
    return tag, data[pos:pos + length], pos + length


def children(data):
    """Every element of a constructed value: [(tag, content, whole element)]."""
    out, pos = [], 0
    while pos < len(data):
        tag, content, end = read(data, pos)
        out.append((tag, content, data[pos:end]))
        pos = end
    return out


def expect(element, tag):
    if element[0] != tag:
        raise Malformed(f"expected tag {tag:#x}, found {element[0]:#x}")
    return element[1]


def oid(content):
    if not content:
        raise Malformed("empty OID")
    arcs, value = [], 0
    for i, byte in enumerate(content):
        value = (value << 7) | (byte & 0x7F)
        if not byte & 0x80:
            arcs.append(value)
            value = 0
        elif i == len(content) - 1:
            raise Malformed("unterminated OID arc")
    first = min(arcs[0] // 40, 2)
    return ".".join(str(a) for a in [first, arcs[0] - 40 * first] + arcs[1:])


def certificates(pkcs7):
    top = children(pkcs7)
    if len(top) != 1:
        raise Malformed("trailing data after the PKCS#7")
    content_info = children(expect(top[0], SEQUENCE))
    if len(content_info) != 2 or oid(expect(content_info[0], OID)) != OID_SIGNED_DATA:
        raise Malformed("not a PKCS#7 SignedData")
    signed_data = children(expect(children(expect(content_info[1], CONTEXT_0))[0], SEQUENCE))
    bags = [e for e in signed_data if e[0] == CONTEXT_0]
    if len(bags) != 1:
        raise Malformed("no certificate set")
    certs = children(bags[0][1])
    for cert in certs:
        expect(cert, SEQUENCE)
    return [cert[2] for cert in certs]


def describe(cert):
    """(organizations, extended key usages, is a CA) of one DER certificate."""
    tbs = children(expect(children(expect(children(cert)[0], SEQUENCE))[0], SEQUENCE))
    if tbs and tbs[0][0] == CONTEXT_0:
        tbs = tbs[1:]
    if len(tbs) < 6:
        raise Malformed("short TBSCertificate")
    orgs = []
    for rdn in children(expect(tbs[4], SEQUENCE)):
        for attribute in children(expect(rdn, SET)):
            kind, value = children(expect(attribute, SEQUENCE))[:2]
            if oid(expect(kind, OID)) == OID_ORGANIZATION:
                if value[0] not in STRING_CODECS:
                    raise Malformed("organization is not a string")
                orgs.append(value[1].decode(STRING_CODECS[value[0]]))
    usages, is_ca, seen = [], False, set()
    for element in tbs[6:]:
        if element[0] != CONTEXT_3:
            continue
        for extension in children(expect(children(element[1])[0], SEQUENCE)):
            fields = children(expect(extension, SEQUENCE))
            ext_id, value = oid(expect(fields[0], OID)), expect(fields[-1], OCTET_STRING)
            if ext_id in seen:
                raise Malformed(f"extension {ext_id} appears twice")
            seen.add(ext_id)
            if ext_id == OID_EXT_KEY_USAGE:
                usages = [oid(expect(u, OID)) for u in children(expect(children(value)[0], SEQUENCE))]
            elif ext_id == OID_BASIC_CONSTRAINTS:
                inner = children(expect(children(value)[0], SEQUENCE))
                is_ca = bool(inner) and inner[0][0] == BOOLEAN and inner[0][1] != b"\x00"
    return orgs, usages, is_ca


def main():
    if len(sys.argv) != 4:
        raise SystemExit(__doc__)
    sig_path, want_org, want_eku = sys.argv[1:4]
    try:
        with open(sig_path, "r", encoding="ascii", errors="strict") as f:
            blocks = re.findall(r"^-----BEGIN PKCS7-----$(.+?)^-----END PKCS7-----$", f.read(), re.S | re.M)
        if len(blocks) != 1:
            raise Malformed(f"{len(blocks)} PKCS7 blocks, need exactly 1")
        certs = certificates(base64.b64decode("".join(blocks[0].split()), validate=True))
        matches = set()
        for cert in certs:
            orgs, usages, is_ca = describe(cert)
            if not is_ca and orgs == [want_org] and want_eku in usages and OID_CODE_SIGNING in usages:
                matches.add(hashlib.sha256(cert).hexdigest())
    except (Malformed, ValueError, IndexError, OSError) as e:
        print(f"windows-signer-leaf: refusing {sig_path}: {e}", file=sys.stderr)
        return 1
    if len(matches) != 1:
        print(f"windows-signer-leaf: {len(matches)} of {len(certs)} certificates match, need exactly 1", file=sys.stderr)
        return 1
    print("sha256:" + matches.pop())
    return 0


if __name__ == "__main__":
    sys.exit(main())
