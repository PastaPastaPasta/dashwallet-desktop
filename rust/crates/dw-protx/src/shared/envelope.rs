//! The message code of dash-qt's shared-masternode envelopes (MG-04b §2.2):
//! `EnvelopeFingerprint` over the object without its `fingerprint` key
//! (rc.3 `src/qt/mnsharesession.cpp:46-57,225-249`).

use dashcore::hashes::{Hash, sha256};

use super::json::Json;

/// The key the producer appends last.
pub const FINGERPRINT_KEY: &str = "fingerprint";

/// `WithoutFingerprint`: the object rebuilt with `pushKV` minus every
/// `fingerprint` key. `pushKV` replaces the first value of a key already
/// pushed, so a top-level duplicate key collapses to one entry, in the first
/// one's place, holding the last one's value. Anything but an object is
/// returned as is.
pub fn without_fingerprint(json: &Json) -> Json {
    let Json::Obj(entries) = json else {
        return json.clone();
    };
    let mut ret = Json::obj();
    for (key, value) in entries {
        if key == FINGERPRINT_KEY.as_bytes() {
            continue;
        }
        let Json::Obj(out) = &mut ret else {
            unreachable!("built as an object");
        };
        match out.iter_mut().find(|(k, _)| k == key) {
            Some((_, v)) => *v = value.clone(),
            None => out.push((key.clone(), value.clone())),
        }
    }
    ret
}

/// `EnvelopeFingerprint`: single SHA-256 of `write(2)` of
/// [`without_fingerprint`], the first four bytes as upper-case hex,
/// `XXXX-XXXX`.
pub fn fingerprint(json: &Json) -> String {
    let digest = sha256::Hash::hash(&without_fingerprint(json).write(2));
    let hex = hex::encode_upper(&digest.as_byte_array()[..4]);
    format!("{}-{}", &hex[..4], &hex[4..])
}

/// `CheckFingerprint`: `true` unless the object carries a string
/// `fingerprint` (the first, as `find_value`) that differs from
/// [`fingerprint`] ignoring case. A mismatch only warns in dash-qt.
pub fn fingerprint_matches(json: &Json) -> bool {
    match json.find(FINGERPRINT_KEY) {
        Some(stamped) if json.is_object() && stamped.is_str() => stamped
            .as_text()
            .is_some_and(|s| s.eq_ignore_ascii_case(&fingerprint(json))),
        _ => true,
    }
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;

    #[test]
    fn test_QT_126_fingerprint_ignores_every_fingerprint_key_and_case() {
        let mut j = Json::obj();
        j.push_kv("type", Json::str("dash-shared-mn-sigs"));
        let code = fingerprint(&j);
        assert_eq!(code.len(), 9);
        assert_eq!(&code[4..5], "-");
        j.push_kv(FINGERPRINT_KEY, Json::str(&code.to_lowercase()));
        assert_eq!(fingerprint(&j), code);
        assert!(fingerprint_matches(&j));
        j.push_kv(FINGERPRINT_KEY, Json::str("0000-0000"));
        assert!(!fingerprint_matches(&j));
        // A message without a code, or with a non-string one, is not "edited".
        assert!(fingerprint_matches(&Json::obj()));
        let mut n = Json::obj();
        n.push_kv(FINGERPRINT_KEY, Json::int(1));
        assert!(fingerprint_matches(&n));
    }
}
