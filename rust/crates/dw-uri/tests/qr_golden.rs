//! testdata/qr_cases.json: matrices from libqrencode called the way dash-qt
//! calls it (byte mode, ECC L, no quiet zone).

use dw_uri::qr::qr_matrix;
use serde_json::Value;

#[test]
fn matrices_match_libqrencode() {
    let path = format!(
        "{}/../../../testdata/qr_cases.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let cases = doc["cases"].as_array().unwrap();
    assert!(cases.len() >= 6);
    for case in cases {
        let text = case["text"].as_str().unwrap();
        let want: Vec<bool> = case["rows"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|row| row.as_str().unwrap().chars().map(|c| c == '1'))
            .collect();
        let got = qr_matrix(text).unwrap();
        assert_eq!(got.size as u64, case["size"].as_u64().unwrap(), "{text}");
        assert!(
            got.modules == want,
            "matrix differs from libqrencode for {text:?}"
        );
    }
}
