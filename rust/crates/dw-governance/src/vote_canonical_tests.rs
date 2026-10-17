//! D63's canonical vote-copy form against recovery itself (validator
//! r13): equal forms exactly when the recovered keys are equal.

use dashcore::hashes::{Hash, sha256};
use dashcore::secp256k1::ecdsa::{RecoverableSignature, RecoveryId, Signature};
use dashcore::secp256k1::{Message, PublicKey, SecretKey};

use crate::vote::{canonical_compact, recover_compact, to_recoverable_compact};

const N: [u8; 32] = [
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xfe,
    0xba, 0xae, 0xdc, 0xe6, 0xaf, 0x48, 0xa0, 0x3b, 0xbf, 0xd2, 0x5e, 0x8c, 0xd0, 0x36, 0x41, 0x41,
];

fn h(tag: &str, i: u32) -> [u8; 32] {
    let mut e = sha256::Hash::engine();
    dashcore::hashes::HashEngine::input(&mut e, tag.as_bytes());
    dashcore::hashes::HashEngine::input(&mut e, &i.to_le_bytes());
    sha256::Hash::from_engine(e).to_byte_array()
}

fn neg(s: &[u8; 32]) -> [u8; 32] {
    SecretKey::from_secret_bytes(*s)
        .unwrap()
        .negate()
        .to_secret_bytes()
}

/// All 512 variants (every header, s and n - s) of `sig`; checks that the
/// canonical form is equal exactly when the recovered key is.
fn check_variants(sig: &[u8; 65], digest: &[u8; 32]) -> usize {
    let s: [u8; 32] = sig[33..].try_into().unwrap();
    let mut variants = Vec::new();
    let s_forms: Vec<[u8; 32]> = if s == [0; 32] {
        vec![s]
    } else {
        vec![s, neg(&s)]
    };
    for sv in s_forms {
        for b in 0u8..=255 {
            let mut c = [0u8; 65];
            c[0] = b;
            c[1..33].copy_from_slice(&sig[1..33]);
            c[33..].copy_from_slice(&sv);
            variants.push((recover_compact(&c, digest), canonical_compact(&c), c));
        }
    }
    let mut recovering = 0;
    for (ka, ca, a) in &variants {
        if ka.is_some() {
            recovering += 1;
            assert!(ca.is_some(), "recovers but no canonical form: {a:?}");
            // The canonical form itself recovers the same key.
            assert_eq!(&recover_compact(&ca.unwrap(), digest), ka);
        }
        for (kb, cb, b) in &variants {
            if ka.is_some() && kb.is_some() {
                assert_eq!(ka == kb, ca == cb, "{a:?} vs {b:?}");
            }
        }
    }
    recovering
}

#[test]
fn canonical_forms_match_recovered_keys_for_honest_signatures() {
    for i in 0..24 {
        let sk = SecretKey::from_secret_bytes(h("key", i)).unwrap();
        let digest = h("digest", i);
        let sig = dashcore::secp256k1::ecdsa::sign(Message::from_digest(digest), &sk);
        let pk = PublicKey::from_secret_key(&sk);
        let full = to_recoverable_compact(&sig.serialize_compact(), &pk, &digest).unwrap();
        // 32 headers per (flags) x 2 s forms x 2 compressions recover.
        assert_eq!(check_variants(&full, &digest), 256);
    }
}

#[test]
fn canonical_forms_match_recovered_keys_for_junk_and_overflowing_x() {
    // Junk (r, s) pairs, including r < p - n so recid bit 1 (x = r + n)
    // can recover a second, distinct key.
    let mut both_bit1 = 0;
    for i in 0..64u32 {
        let digest = h("jd", i);
        let mut r = h("jr", i);
        if i % 2 == 0 {
            // r < p - n (~2^128): x = r + n is a candidate too.
            r[..17].fill(0);
        }
        let s = h("js", i);
        let mut sig = [0u8; 65];
        sig[0] = 27;
        sig[1..33].copy_from_slice(&r);
        sig[33..].copy_from_slice(&s);
        if Signature::from_compact(&sig[1..]).is_err() {
            continue;
        }
        check_variants(&sig, &digest);
        let mut hi = sig;
        hi[0] = 27 + 2;
        let mut lo = sig;
        lo[0] = 27;
        if recover_compact(&hi, &digest).is_some() && recover_compact(&lo, &digest).is_some() {
            both_bit1 += 1;
            assert_ne!(canonical_compact(&hi), canonical_compact(&lo));
        }
    }
    assert!(both_bit1 > 0, "no r with both x = r and x = r + n tried");
}

#[test]
fn both_parsers_agree_on_edge_scalars() {
    let mut n_minus_1 = N;
    n_minus_1[31] -= 1;
    let edges: [[u8; 32]; 6] = [[0; 32], [0xff; 32], N, n_minus_1, h("e", 1), {
        let mut one = [0; 32];
        one[31] = 1;
        one
    }];
    for r in &edges {
        for s in &edges {
            let mut c = [0u8; 64];
            c[..32].copy_from_slice(r);
            c[32..].copy_from_slice(s);
            let plain = Signature::from_compact(&c).is_ok();
            for id in 0..4 {
                let rec = RecoverableSignature::from_compact(&c, RecoveryId::try_from(id).unwrap())
                    .is_ok();
                assert_eq!(plain, rec, "r {r:?} s {s:?} id {id}");
            }
            for b in 0u8..=255 {
                let mut sig = [0u8; 65];
                sig[0] = b;
                sig[1..].copy_from_slice(&c);
                let digest = h("edge", u32::from(b));
                // No canonical form only when nothing recovers.
                if canonical_compact(&sig).is_none() {
                    assert!(recover_compact(&sig, &digest).is_none());
                }
                // s = 0 or r = 0 never recover.
                if *s == [0; 32] || *r == [0; 32] {
                    assert!(recover_compact(&sig, &digest).is_none());
                }
            }
        }
    }
}
