//! `dw-protx::shared::json` and the message code against Dash Core
//! v24.0.0-rc.3's own UniValue (MG-04b D7). The vectors come from
//! `testdata/oracle/univalue/gen_vectors.py`, which compiles rc.3's
//! `src/univalue` with a short driver; this test only reads them.
//!
//! Besides every value, the check pins the vectors' structure: the case
//! count, which fields each case carries (the driver's emission rules) and
//! which cases are `read_only`, so a vector file with fields or cases
//! dropped cannot pass. `test_QT_126_corrupted_vectors_fail` applies such
//! corruptions in memory and requires the check to fail on each.

#![allow(non_snake_case)]

use std::collections::BTreeSet;
use std::fmt::Debug;

use dw_protx::shared::envelope::{fingerprint, without_fingerprint};
use dw_protx::shared::json::Json;
use serde_json::Value;

/// The number of cases in `gen_vectors.py`'s `INPUTS`.
const EXPECTED_CASES: usize = 112;
/// The deep-nesting cases whose pretty rendering is not kept.
const EXPECTED_READ_ONLY: usize = 4;

fn vectors() -> Value {
    let path = format!(
        "{}/../../../testdata/oracle/univalue/univalue_vectors.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn bytes(v: &Value) -> Result<Vec<u8>, String> {
    let s = v.as_str().ok_or_else(|| format!("{v} is not a string"))?;
    hex::decode(s).map_err(|e| format!("{s}: {e}"))
}

fn show(b: &[u8]) -> String {
    String::from_utf8_lossy(&b[..b.len().min(80)]).into_owned()
}

fn same<T: PartialEq + Debug>(got: T, want: T, what: &str, input: &[u8]) -> Result<(), String> {
    if got == want {
        Ok(())
    } else {
        Err(format!(
            "{what} of {}: got {got:?}, want {want:?}",
            show(input)
        ))
    }
}

/// gen_vectors.py's READ_ONLY rule: quadratic-to-render deep nesting.
fn is_deep(input: &[u8]) -> bool {
    input.len() > 1000 && (input.starts_with(b"[[[[") || input.starts_with(b"{\"a\""))
}

/// Checks the Rust reader, writer, `WithoutFingerprint` and message code
/// against every case of the vector document, and the document's structure
/// against the driver's emission rules (`univalue_oracle.cpp`).
fn check(doc: &Value) -> Result<(), String> {
    let cases = doc["cases"].as_array().ok_or("cases is not an array")?;
    same(
        doc["count"].as_u64(),
        Some(cases.len() as u64),
        "count",
        b"",
    )?;
    same(cases.len(), EXPECTED_CASES, "number of cases", b"")?;
    let mut read_only = 0;
    for case in cases {
        let obj = case.as_object().ok_or("a case is not an object")?;
        let input = bytes(&case["input"])?;
        let ok = case["ok"].as_bool().ok_or("ok is not a bool")?;
        let parsed = Json::read(&input);
        same(parsed.is_some(), ok, "read", &input)?;

        // The fields the driver emits, derived from the Rust reader's own
        // verdict on the value (already equal to the vector's `ok`).
        let deep = is_deep(&input);
        let mut want_keys: BTreeSet<&str> = ["input", "ok"].into();
        if deep {
            want_keys.insert("read_only");
            read_only += 1;
        } else if let Some(json) = &parsed {
            want_keys.extend(["write0", "write2", "without_fp"]);
            if json.is_object() {
                want_keys.insert("fingerprint");
            }
            if json.as_array().is_some() {
                want_keys.insert("ints");
            }
        }
        let got_keys: BTreeSet<&str> = obj.keys().map(String::as_str).collect();
        same(got_keys, want_keys, "fields", &input)?;
        if deep {
            same(case["read_only"].as_bool(), Some(true), "read_only", &input)?;
        }

        let Some(json) = parsed else { continue };
        if deep {
            continue;
        }
        same(json.write(0), bytes(&case["write0"])?, "write()", &input)?;
        same(json.write(2), bytes(&case["write2"])?, "write(2)", &input)?;
        same(
            without_fingerprint(&json).write(2),
            bytes(&case["without_fp"])?,
            "WithoutFingerprint",
            &input,
        )?;
        if json.is_object() {
            same(
                Some(fingerprint(&json)),
                case["fingerprint"].as_str().map(str::to_owned),
                "code",
                &input,
            )?;
        }
        if let Some(elems) = json.as_array() {
            let rows = case["ints"].as_array().ok_or("ints is not an array")?;
            same(rows.len(), elems.len(), "ints rows", &input)?;
            for (elem, row) in elems.iter().zip(rows) {
                let row = row.as_array().ok_or("an ints row is not an array")?;
                if row.is_empty() {
                    same(elem.is_num(), false, "empty ints row of a number", &input)?;
                    continue;
                }
                same(row.len(), 4, "ints row length", &input)?;
                same(elem.is_num(), true, "ints row of a non-number", &input)?;
                let want = |i: usize| row[i].as_str().map(str::to_owned);
                same(
                    elem.get_i64().map(|n| n.to_string()),
                    want(0),
                    "int64",
                    &input,
                )?;
                same(
                    elem.get_u64().map(|n| n.to_string()),
                    want(1),
                    "uint64",
                    &input,
                )?;
                same(
                    elem.get_i32().map(|n| n.to_string()),
                    want(2),
                    "int",
                    &input,
                )?;
                same(
                    elem.get_u32().map(|n| n.to_string()),
                    want(3),
                    "uint32",
                    &input,
                )?;
            }
        }
    }
    same(
        read_only,
        EXPECTED_READ_ONLY,
        "number of read_only cases",
        b"",
    )
}

#[test]
fn test_QT_126_reader_writer_and_code_equal_univalues() {
    if let Err(e) = check(&vectors()) {
        panic!("{e}");
    }
}

/// Flips the first hex digit of a string value.
fn flip(v: &mut Value) {
    let s = v.as_str().unwrap();
    let first = if s.starts_with('0') { '1' } else { '0' };
    *v = Value::String(format!("{first}{}", &s[1..]));
}

fn cases_mut(doc: &mut Value) -> &mut Vec<Value> {
    doc["cases"].as_array_mut().unwrap()
}

fn remove_field_everywhere(doc: &mut Value, field: &str) {
    for case in cases_mut(doc) {
        case.as_object_mut().unwrap().remove(field);
    }
}

fn first_with<'a>(doc: &'a mut Value, field: &str) -> &'a mut Value {
    cases_mut(doc)
        .iter_mut()
        .find(|c| c.get(field).is_some())
        .unwrap_or_else(|| panic!("no case has {field}"))
}

#[test]
fn test_QT_126_corrupted_vectors_fail() {
    type Corruption = (&'static str, fn(&mut Value));
    let corruptions: &[Corruption] = &[
        ("a byte of an envelope's write2", |d| {
            flip(&mut cases_mut(d)[0]["write2"]);
        }),
        ("a byte of write0", |d| {
            flip(&mut first_with(d, "write0")["write0"])
        }),
        ("a byte of without_fp", |d| {
            flip(&mut first_with(d, "without_fp")["without_fp"]);
        }),
        ("a fingerprint value", |d| {
            *first_with(d, "fingerprint").get_mut("fingerprint").unwrap() =
                Value::String("0000-0000".into());
        }),
        ("an ints value", |d| {
            let row = cases_mut(d)
                .iter_mut()
                .filter_map(|c| c.get_mut("ints")?.as_array_mut())
                .flatten()
                .find(|r| !r.as_array().unwrap().is_empty())
                .unwrap();
            row[0] = Value::String("999".into());
        }),
        ("an ok verdict", |d| {
            let ok = cases_mut(d)[0]["ok"].as_bool().unwrap();
            cases_mut(d)[0]["ok"] = Value::Bool(!ok);
        }),
        ("fingerprint removed from every case", |d| {
            remove_field_everywhere(d, "fingerprint");
        }),
        ("ints removed from every case", |d| {
            remove_field_everywhere(d, "ints");
        }),
        ("every case marked read_only", |d| {
            for case in cases_mut(d) {
                case["read_only"] = Value::Bool(true);
            }
        }),
        ("read_only removed from every case", |d| {
            remove_field_everywhere(d, "read_only");
        }),
        ("truncated to 81 cases, count kept", |d| {
            cases_mut(d).truncate(81)
        }),
        ("truncated to 81 cases, count updated", |d| {
            cases_mut(d).truncate(81);
            d["count"] = Value::from(81);
        }),
        ("count removed", |d| {
            d.as_object_mut().unwrap().remove("count");
        }),
    ];

    let pristine = vectors();
    check(&pristine).expect("the uncorrupted vectors pass");
    for (name, corrupt) in corruptions {
        let mut doc = pristine.clone();
        corrupt(&mut doc);
        assert_ne!(doc, pristine, "{name}: the corruption changed nothing");
        assert!(check(&doc).is_err(), "check passed with {name}");
    }
}
