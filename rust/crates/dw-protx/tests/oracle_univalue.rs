//! `dw-protx::shared::json` and the message code against Dash Core
//! v24.0.0-rc.3's own UniValue (MG-04b D7). The vectors come from
//! `testdata/oracle/univalue/gen_vectors.py`, which compiles rc.3's
//! `src/univalue` with a short driver; this test only reads them.

#![allow(non_snake_case)]

use dw_protx::shared::envelope::{fingerprint, without_fingerprint};
use dw_protx::shared::json::Json;

fn vectors() -> Vec<serde_json::Value> {
    let path = format!(
        "{}/../../../testdata/oracle/univalue/univalue_vectors.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    doc["cases"].as_array().unwrap().clone()
}

fn bytes(v: &serde_json::Value) -> Vec<u8> {
    hex::decode(v.as_str().unwrap()).unwrap()
}

fn show(b: &[u8]) -> String {
    String::from_utf8_lossy(&b[..b.len().min(80)]).into_owned()
}

#[test]
fn test_QT_126_reader_writer_and_code_equal_univalues() {
    let cases = vectors();
    assert!(cases.len() > 80, "the vectors hold every case");
    for case in &cases {
        let input = bytes(&case["input"]);
        let parsed = Json::read(&input);
        assert_eq!(
            parsed.is_some(),
            case["ok"].as_bool().unwrap(),
            "read({})",
            show(&input)
        );
        let Some(json) = parsed else { continue };
        if case["read_only"].as_bool() == Some(true) {
            continue;
        }
        assert_eq!(
            json.write(0),
            bytes(&case["write0"]),
            "write() of {}",
            show(&input)
        );
        assert_eq!(
            json.write(2),
            bytes(&case["write2"]),
            "write(2) of {}",
            show(&input)
        );
        assert_eq!(
            without_fingerprint(&json).write(2),
            bytes(&case["without_fp"]),
            "WithoutFingerprint of {}",
            show(&input)
        );
        if let Some(code) = case.get("fingerprint") {
            assert_eq!(
                fingerprint(&json),
                code.as_str().unwrap(),
                "code of {}",
                show(&input)
            );
        }
        if let Some(rows) = case.get("ints") {
            let elems = json.as_array().unwrap();
            for (elem, row) in elems.iter().zip(rows.as_array().unwrap()) {
                let row = row.as_array().unwrap();
                if row.is_empty() {
                    assert!(!elem.is_num());
                    continue;
                }
                let want = |i: usize| row[i].as_str().map(str::to_owned);
                assert_eq!(
                    elem.get_i64().map(|n| n.to_string()),
                    want(0),
                    "int64 of {elem:?}"
                );
                assert_eq!(
                    elem.get_u64().map(|n| n.to_string()),
                    want(1),
                    "uint64 of {elem:?}"
                );
                assert_eq!(
                    elem.get_i32().map(|n| n.to_string()),
                    want(2),
                    "int of {elem:?}"
                );
                assert_eq!(
                    elem.get_u32().map(|n| n.to_string()),
                    want(3),
                    "uint32 of {elem:?}"
                );
            }
        }
    }
}
