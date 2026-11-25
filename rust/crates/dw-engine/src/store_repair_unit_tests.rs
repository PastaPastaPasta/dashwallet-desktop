use super::*;

fn spend(scripts: &[&[u8]], outpoints: &[OutPoint]) -> Transaction {
    Transaction {
        version: 2,
        lock_time: 0,
        input: (scripts.iter().zip(outpoints))
            .map(|(s, o)| dashcore::TxIn {
                previous_output: *o,
                script_sig: dashcore::ScriptBuf::from_bytes(s.to_vec()),
                sequence: u32::MAX,
                witness: Default::default(),
            })
            .collect(),
        output: Vec::new(),
        special_transaction_payload: None,
    }
}

/// S4: an input script over `MAX_SCRIPT_SIZE`, one that doesn't parse, and
/// an outpoint spent twice are structural; a parseable script that isn't
/// push-only is not.
#[test]
fn structural_invalidity_is_size_parse_or_a_repeated_outpoint() {
    let (a, b) = (
        OutPoint::new(Txid::from_byte_array([1; 32]), 0),
        OutPoint::new(Txid::from_byte_array([2; 32]), 0),
    );
    let long = vec![0x61; crate::input_authorization::MAX_SCRIPT_SIZE + 1];
    assert!(structurally_invalid(&spend(&[&long], &[a]), 0));
    let at_limit = vec![0x61; crate::input_authorization::MAX_SCRIPT_SIZE];
    assert!(!structurally_invalid(&spend(&[&at_limit], &[a]), 0));
    assert!(structurally_invalid(
        &spend(&[&[0x4c, 0xff, 0x01]], &[a]),
        0
    ));
    assert!(structurally_invalid(
        &spend(&[&[0x61], &[0x61]], &[a, a]),
        1
    ));
    assert!(!structurally_invalid(
        &spend(&[&[0x61], &[0x61]], &[a, b]),
        1
    ));
    assert!(!structurally_invalid(
        &spend(&[&[0x61, 0x01, 0x01]], &[a]),
        0
    ));
}

/// The store's outpoint key: a length byte, the txid's 32 bytes, then the
/// index, as platform-wallet-storage's V014 note pins it.
#[test]
fn an_outpoint_key_carries_its_txid_at_bytes_two_to_thirty_three() {
    let o = OutPoint::new(Txid::from_byte_array([7; 32]), 300);
    let key = encode_outpoint(&o);
    assert_eq!(&key[1..33], o.txid.as_byte_array().as_slice());
    assert_eq!(decode_outpoint(&key), Some(o));
    assert_eq!(decode_outpoint(&[key.as_slice(), &[0]].concat()), None);
}
