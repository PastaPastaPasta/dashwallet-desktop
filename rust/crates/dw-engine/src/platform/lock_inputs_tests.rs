use std::sync::atomic::AtomicU32;

use super::*;

/// The inner signer: counts what it is asked and signs nothing (the
/// bound is the wrapper's; the vault is not needed to see it).
#[derive(Debug, Default)]
struct Inner {
    asked: AtomicU32,
    /// The input counts of the builds it was shown.
    shown: Mutex<Vec<usize>>,
}

#[async_trait]
impl Signer for Inner {
    type Error = SignerError;

    fn supported_methods(&self) -> &[SignerMethod] {
        &[SignerMethod::Digest]
    }

    async fn sign_ecdsa(
        &self,
        _: &DerivationPath,
        _: [u8; 32],
    ) -> Result<(ecdsa::Signature, PublicKey), SignerError> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        Err(SignerError::PathNotAllowed("inner".into()))
    }

    async fn public_key(&self, _: &DerivationPath) -> Result<PublicKey, SignerError> {
        Err(SignerError::PathNotAllowed("inner".into()))
    }

    async fn approve_inputs(&self, _: &Transaction, inputs: &[Utxo]) -> Result<(), SignerError> {
        self.shown.lock().unwrap().push(inputs.len());
        Ok(())
    }
}

/// A build of `n` inputs, as key-wallet's builder shows it to the signer.
fn build(n: u32) -> (Transaction, Vec<Utxo>) {
    use dashcore::hashes::Hash;
    use dashcore::{Address, Network, OutPoint, TxIn, TxOut, Txid};
    let address = Address::p2pkh(
        &dashcore::PublicKey::from_slice(&[2; 33]).unwrap(),
        Network::Testnet,
    );
    let outpoints: Vec<OutPoint> = (0..n)
        .map(|vout| OutPoint::new(Txid::from_byte_array([0xe1; 32]), vout))
        .collect();
    let utxos = (outpoints.iter())
        .map(|&outpoint| {
            let txout = TxOut {
                value: 60_000,
                script_pubkey: address.script_pubkey(),
            };
            Utxo::new(outpoint, txout, address.clone(), 1, false)
        })
        .collect();
    let tx = Transaction {
        version: 3,
        lock_time: 0,
        input: (outpoints.into_iter())
            .map(|previous_output| TxIn {
                previous_output,
                ..Default::default()
            })
            .collect(),
        output: Vec::new(),
        special_transaction_payload: None,
    };
    (tx, utxos)
}

fn reason(r: Result<(ecdsa::Signature, PublicKey), SignerError>) -> String {
    match r {
        Err(SignerError::PathNotAllowed(m)) => m,
        other => panic!("{other:?}"),
    }
}

/// Review r2 R2-m3: the signer of a lock signs at most 50 distinct
/// sighashes (inputs); the 51st is refused before the vault is asked, so
/// the transaction is never complete. A sighash signed already passes.
#[test]
fn the_fifty_first_input_is_never_signed() {
    let capped = Capped::new(Inner::default());
    let path = DerivationPath::default();
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    rt.block_on(async {
        for n in 0..50u8 {
            assert_eq!(reason(capped.sign_ecdsa(&path, [n; 32]).await), "inner");
        }
        assert!(!capped.refused());
        assert_eq!(reason(capped.sign_ecdsa(&path, [7; 32]).await), "inner");
        let refused = reason(capped.sign_ecdsa(&path, [50; 32]).await);
        assert!(refused.contains("at most 50 coins"), "{refused}");
    });
    assert!(capped.refused());
    assert_eq!(
        capped.inner.asked.load(Ordering::SeqCst),
        51,
        "50 inputs + one again"
    );
}

/// The fewest coins, largest first, that reach an amount: no selection
/// takes fewer.
#[test]
fn the_least_coins_reaching_an_amount() {
    assert_eq!(least(&[5, 1, 3], 4), Some(1));
    assert_eq!(least(&[5, 1, 3], 6), Some(2));
    assert_eq!(least(&[5, 1, 3], 9), Some(3));
    assert_eq!(least(&[5, 1, 3], 10), None);
    assert_eq!(least(&[1; 51], 51), Some(51));
    assert_eq!(least(&[1; 51], 50), Some(MAX_INPUTS));
    // The top-up screen shows this detail as it is (topup.test.ts pins it).
    assert_eq!(
        too_many(Some(51)).to_string(),
        "invalid argument: this payment would use 51 coins and at most 50 can be used: \
         send Dash to your own wallet first to combine them, then try again"
    );
}

/// 0.1.x row 62: a build of exactly 50 inputs is approved and shown to the
/// inner signer; one of 51 is refused before any signature, the inner
/// signer is neither shown it nor asked to sign, and the wrapper then
/// signs nothing, its refusal naming the 51 coins.
#[test]
fn a_build_of_fifty_inputs_passes_and_one_of_fifty_one_is_refused_before_signing() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let path = DerivationPath::default();
    let fifty = Capped::new(Inner::default());
    let (tx, inputs) = build(50);
    assert!(rt.block_on(fifty.approve_inputs(&tx, &inputs)).is_ok());
    assert!(!fifty.refused());
    assert_eq!(*fifty.inner.shown.lock().unwrap(), vec![50]);

    let capped = Capped::new(Inner::default());
    let (tx, inputs) = build(51);
    let Err(SignerError::PathNotAllowed(refused)) =
        rt.block_on(capped.approve_inputs(&tx, &inputs))
    else {
        panic!("a build of 51 inputs was approved")
    };
    assert!(refused.contains("at most 50 coins"), "{refused}");
    assert!(capped.refused());
    assert!(
        capped.inner.shown.lock().unwrap().is_empty(),
        "not shown on"
    );
    let after = reason(rt.block_on(capped.sign_ecdsa(&path, [1; 32])));
    assert!(after.contains("refused for its inputs"), "{after}");
    assert_eq!(
        capped.inner.asked.load(Ordering::SeqCst),
        0,
        "nothing signed"
    );
    assert_eq!(capped.refusal().to_string(), too_many(Some(51)).to_string());
}
