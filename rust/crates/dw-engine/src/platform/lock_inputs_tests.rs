use std::sync::atomic::AtomicU32;

use super::*;

/// The inner signer: counts what it is asked and signs nothing (the
/// bound is the wrapper's; the vault is not needed to see it).
#[derive(Debug, Default)]
struct Inner {
    asked: AtomicU32,
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
    assert!(too_many(Some(51)).to_string().contains("needs 51"));
}
