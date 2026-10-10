//! Partially signed transactions as Dash Core and dash-qt use them
//! (QT-076…079), over a vendored BIP174 container ([`psbt`]).
//!
//! Dash has no segwit, so Dash Core's PSBT (`src/psbt.h`) is BIP174 v0 with
//! the legacy fields only: inputs carry the full previous transaction
//! (`non_witness_utxo`), partial signatures, sighash type, redeem script,
//! BIP32 derivations and the final scriptSig; outputs carry redeem scripts
//! and BIP32 derivations. This crate:
//!
//! - parses binary or base64 PSBTs (dash-qt's file and clipboard forms) and
//!   writes both;
//! - builds an unsigned PSBT from a planned transaction ("Create Unsigned");
//! - analyzes one the way dash-qt's PSBT Operations dialog does
//!   (`AnalyzePSBT`, `CountPSBTUnsignedInputs`);
//! - signs P2PKH inputs through a key-wallet [`key_wallet::Signer`] (the
//!   vault's signer) with the legacy sighash, as `walletprocesspsbt` does;
//! - finalizes P2PKH inputs and extracts the transaction (`finalizepsbt`).
//!
//! Other input scripts (P2SH multisig, bare) are carried and counted but
//! neither signed nor finalized here.

use std::collections::BTreeMap;

use base64::Engine as _;
use dashcore::blockdata::script::{Builder, PushBytesBuf, ScriptBuf};
use dashcore::blockdata::transaction::Transaction;
use dashcore::crypto::ecdsa;
use dashcore::hashes::Hash;
use dashcore::secp256k1::{self, Message, Secp256k1};
use dashcore::sighash::{EcdsaSighashType, SighashCache};
use dashcore::{Address, Network, PubkeyHash, Txid};
use key_wallet::Signer;
use key_wallet::bip32::{DerivationPath, Fingerprint};

pub mod psbt;
pub use psbt::{PartiallySignedTransaction, PsbtSighashType};

/// Largest PSBT accepted (dash-qt refuses files of 100 MiB or more).
pub const MAX_PSBT_BYTES: usize = 100 * 1024 * 1024;
/// dash-qt's broadcast cap: `DEFAULT_MAX_RAW_TX_FEE_RATE`, 0.1 DASH/kB.
pub const MAX_BROADCAST_FEE_PER_KB: u64 = 10_000_000;

const MAGIC: &[u8; 5] = b"psbt\xff";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PsbtError {
    #[error("not a PSBT: {0}")]
    Invalid(String),
    #[error("PSBT is {0} bytes")]
    TooLarge(usize),
    #[error("input {0} has no previous transaction")]
    MissingUtxo(usize),
    #[error("input {index}: previous transaction {txid} does not match the input")]
    UtxoMismatch { index: usize, txid: Txid },
    #[error("not every input is signed")]
    NotComplete,
    #[error("signing input {index}: {detail}")]
    Signing { index: usize, detail: String },
}

/// Parses a PSBT from a file (binary, or base64 text) or the clipboard
/// (base64). Surrounding whitespace of base64 text is ignored.
pub fn parse(data: &[u8]) -> Result<PartiallySignedTransaction, PsbtError> {
    if data.len() >= MAX_PSBT_BYTES {
        return Err(PsbtError::TooLarge(data.len()));
    }
    let decoded;
    let bytes = if data.starts_with(MAGIC) {
        data
    } else {
        let text = std::str::from_utf8(data)
            .map_err(|_| PsbtError::Invalid("neither binary nor base64".into()))?;
        let compact: String = text.split_ascii_whitespace().collect();
        decoded = base64::engine::general_purpose::STANDARD
            .decode(compact.as_bytes())
            .map_err(|e| PsbtError::Invalid(format!("base64: {e}")))?;
        &decoded[..]
    };
    let psbt = PartiallySignedTransaction::deserialize(bytes)
        .map_err(|e| PsbtError::Invalid(e.to_string()))?;
    check_utxos(&psbt)?;
    Ok(psbt)
}

/// Dash Core refuses a PSBT whose `non_witness_utxo` is not the transaction
/// the input spends.
fn check_utxos(psbt: &PartiallySignedTransaction) -> Result<(), PsbtError> {
    for (index, (txin, input)) in psbt.unsigned_tx.input.iter().zip(&psbt.inputs).enumerate() {
        if let Some(prev) = &input.non_witness_utxo {
            let txid = prev.txid();
            if txid != txin.previous_output.txid
                || prev.output.len() <= txin.previous_output.vout as usize
            {
                return Err(PsbtError::UtxoMismatch { index, txid });
            }
        }
    }
    Ok(())
}

pub fn to_bytes(psbt: &PartiallySignedTransaction) -> Vec<u8> {
    psbt.serialize()
}

pub fn to_base64(psbt: &PartiallySignedTransaction) -> String {
    base64::engine::general_purpose::STANDARD.encode(psbt.serialize())
}

pub fn unsigned_txid(psbt: &PartiallySignedTransaction) -> Txid {
    psbt.unsigned_tx.txid()
}

/// Wallet data for one input of an unsigned PSBT.
pub struct InputData {
    /// The transaction that created the spent output.
    pub prev_tx: Transaction,
    /// The key that signs it, when the wallet knows it: compressed public
    /// key, master fingerprint and full derivation path.
    pub derivation: Option<(secp256k1::PublicKey, Fingerprint, DerivationPath)>,
}

/// Builds dash-qt's "Create Unsigned" PSBT: `tx` with empty scriptSigs, each
/// input's previous transaction and BIP32 derivation, and the BIP32
/// derivation of outputs paying the wallet (`outputs[i]`, `None` for others).
pub fn create_unsigned(
    mut tx: Transaction,
    inputs: Vec<InputData>,
    outputs: Vec<Option<(secp256k1::PublicKey, Fingerprint, DerivationPath)>>,
) -> Result<PartiallySignedTransaction, PsbtError> {
    if inputs.len() != tx.input.len() || outputs.len() != tx.output.len() {
        return Err(PsbtError::Invalid(
            "input or output data does not match the transaction".into(),
        ));
    }
    for txin in &mut tx.input {
        txin.script_sig = ScriptBuf::new();
    }
    let mut psbt = PartiallySignedTransaction::from_unsigned_tx(tx)
        .map_err(|e| PsbtError::Invalid(e.to_string()))?;
    for (input, data) in psbt.inputs.iter_mut().zip(inputs) {
        input.non_witness_utxo = Some(data.prev_tx);
        if let Some((pk, fp, path)) = data.derivation {
            input.bip32_derivation.insert(pk, (fp, path));
        }
    }
    for (output, data) in psbt.outputs.iter_mut().zip(outputs) {
        if let Some((pk, fp, path)) = data {
            output.bip32_derivation.insert(pk, (fp, path));
        }
    }
    check_utxos(&psbt)?;
    Ok(psbt)
}

/// The output an input spends, from its previous transaction
/// (`non_witness_utxo`) and only when that transaction's txid is the one the
/// input names. Dash has no segwit: a `witness_utxo` is ignored, because
/// nothing commits to its value (the legacy sighash does not sign the
/// amount), so a PSBT could claim any value for one of the wallet's scripts.
/// Every amount, fee and spending cap is therefore computed from verified
/// previous transactions only.
pub fn spent_output(psbt: &PartiallySignedTransaction, index: usize) -> Option<&dashcore::TxOut> {
    let txin = psbt.unsigned_tx.input.get(index)?;
    let prev = psbt.inputs.get(index)?.non_witness_utxo.as_ref()?;
    if prev.txid() != txin.previous_output.txid {
        return None;
    }
    prev.output.get(txin.previous_output.vout as usize)
}

/// The public key of a P2PKH script's hash among `keys`.
fn p2pkh_key<'a>(
    script: &ScriptBuf,
    mut keys: impl Iterator<Item = &'a dashcore::PublicKey>,
) -> Option<&'a dashcore::PublicKey> {
    if !script.is_p2pkh() {
        return None;
    }
    let hash = &script.as_bytes()[3..23];
    keys.find(|pk| pk.pubkey_hash().as_byte_array()[..] == *hash)
}

/// Whether input `index` has a final scriptSig (`PSBTInputSigned`).
pub fn input_signed(psbt: &PartiallySignedTransaction, index: usize) -> bool {
    psbt.inputs[index]
        .final_script_sig
        .as_ref()
        .is_some_and(|s| !s.is_empty())
}

/// Whether input `index` can be finalized now: signed, or a P2PKH input
/// with a partial signature of the key its script pays.
fn input_complete(psbt: &PartiallySignedTransaction, index: usize) -> bool {
    if input_signed(psbt, index) {
        return true;
    }
    spent_output(psbt, index)
        .and_then(|out| p2pkh_key(&out.script_pubkey, psbt.inputs[index].partial_sigs.keys()))
        .is_some()
}

/// `AnalyzePSBT`'s next role, as dash-qt's status line shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Some input lacks its previous transaction ("missing some
    /// information about inputs").
    MissingInputInfo,
    /// Some input still needs a signature.
    NeedsSignatures,
    /// Every input is signed or can be finalized.
    Complete,
}

/// One output line of the dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputLine {
    /// `None` for scripts without an address (OP_RETURN, bare multisig).
    pub address: Option<String>,
    pub amount: u64,
    pub script: ScriptBuf,
}

/// What dash-qt's PSBT Operations dialog shows, minus wallet ownership.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Analysis {
    pub outputs: Vec<OutputLine>,
    /// Inputs minus outputs; `None` while an input's previous transaction is
    /// missing or is not the one the input names (see [`spent_output`]).
    pub fee: Option<u64>,
    /// Sum of every output ("Total Amount"); `None` like `fee`.
    pub total: Option<u64>,
    /// Inputs without a final scriptSig (`CountPSBTUnsignedInputs`).
    pub unsigned_inputs: u32,
    pub status: Status,
    /// Serialized size of the transaction once every input is signed
    /// (dashd's `estimated_vsize`).
    pub estimated_size: usize,
}

pub fn analyze(psbt: &PartiallySignedTransaction, network: Network) -> Analysis {
    let outputs: Vec<OutputLine> = psbt
        .unsigned_tx
        .output
        .iter()
        .map(|o| OutputLine {
            address: Address::from_script(&o.script_pubkey, network)
                .ok()
                .map(|a| a.to_string()),
            amount: o.value,
            script: o.script_pubkey.clone(),
        })
        .collect();
    let out_sum: Option<u64> = outputs
        .iter()
        .try_fold(0u64, |a, o| a.checked_add(o.amount));
    let mut in_sum = Some(0u64);
    let mut status = Status::Complete;
    let mut unsigned_inputs = 0u32;
    for i in 0..psbt.inputs.len() {
        match spent_output(psbt, i) {
            Some(out) => in_sum = in_sum.and_then(|s| s.checked_add(out.value)),
            None => {
                in_sum = None;
                status = Status::MissingInputInfo;
            }
        }
        if !input_signed(psbt, i) {
            unsigned_inputs += 1;
        }
        if status != Status::MissingInputInfo && !input_complete(psbt, i) {
            status = Status::NeedsSignatures;
        }
    }
    let fee = match (in_sum, out_sum) {
        (Some(i), Some(o)) => i.checked_sub(o),
        _ => None,
    };
    Analysis {
        total: fee.and(out_sum),
        outputs,
        fee,
        unsigned_inputs,
        status,
        estimated_size: estimated_size(psbt),
    }
}

/// Serialized size with every unsigned input at the size of a P2PKH input
/// signed with a low-R signature, as dashd's `analyzepsbt` estimates it.
pub fn estimated_size(psbt: &PartiallySignedTransaction) -> usize {
    let mut tx = psbt.unsigned_tx.clone();
    for (i, txin) in tx.input.iter_mut().enumerate() {
        if let Some(s) = &psbt.inputs[i].final_script_sig {
            txin.script_sig = s.clone();
        } else {
            // Core's dummy P2PKH scriptSig with a low-R signature: push of
            // 71 bytes (70-byte DER + sighash byte) and of a 33-byte key.
            txin.script_sig = ScriptBuf::from(vec![0u8; 106]);
        }
    }
    dashcore::consensus::serialize(&tx).len()
}

/// Which inputs a wallet can sign: for each input, the derivation path of
/// the key its P2PKH script pays, when the wallet owns it.
pub type KeyPaths = BTreeMap<usize, DerivationPath>;

/// Signs every P2PKH input listed in `paths` that is not yet complete, with
/// `SIGHASH_ALL` (or the input's own sighash type), and returns how many
/// inputs got a signature. With a low-R signer (the vault's, like Dash
/// Core's `CKey::Sign`) the signatures equal `walletprocesspsbt`'s. Each signature is checked against the input's
/// script before it is stored.
pub async fn sign<S: Signer>(
    psbt: &mut PartiallySignedTransaction,
    paths: &KeyPaths,
    signer: &S,
) -> Result<usize, PsbtError> {
    let tx = psbt.unsigned_tx.clone();
    let cache = SighashCache::new(&tx);
    let secp = Secp256k1::verification_only();
    let mut signed = 0;
    for (&index, path) in paths {
        if index >= psbt.inputs.len() || input_complete(psbt, index) {
            continue;
        }
        let script = spent_output(psbt, index)
            .ok_or(PsbtError::MissingUtxo(index))?
            .script_pubkey
            .clone();
        if !script.is_p2pkh() {
            continue;
        }
        // dash-qt and `walletprocesspsbt` sign SIGHASH_ALL only. Another type
        // asked for by the PSBT (NONE, SINGLE, ANYONECANPAY) would let whoever
        // made it change outputs after signing, past the grant's cap.
        let hash_ty = EcdsaSighashType::All;
        if let Some(t) = psbt.inputs[index].sighash_type
            && t.ecdsa_hash_ty() != Some(hash_ty)
        {
            return Err(PsbtError::Signing {
                index,
                detail: format!("sighash type {t} is not SIGHASH_ALL"),
            });
        }
        let sighash = cache
            .legacy_signature_hash(index, &script, hash_ty.to_u32())
            .map_err(|e| PsbtError::Signing {
                index,
                detail: e.to_string(),
            })?;
        let digest = sighash.to_byte_array();
        let (sig, pk) = signer
            .sign_ecdsa(path, digest)
            .await
            .map_err(|e| PsbtError::Signing {
                index,
                detail: e.to_string(),
            })?;
        let pk = dashcore::PublicKey::new(pk);
        if p2pkh_key(&script, std::iter::once(&pk)).is_none() {
            return Err(PsbtError::Signing {
                index,
                detail: format!("the key at {path} does not pay this input"),
            });
        }
        secp.verify_ecdsa(&Message::from_digest(digest), &sig, &pk.inner)
            .map_err(|e| PsbtError::Signing {
                index,
                detail: format!("signature does not verify: {e}"),
            })?;
        psbt.inputs[index]
            .partial_sigs
            .insert(pk, ecdsa::Signature { sig, hash_ty });
        signed += 1;
    }
    Ok(signed)
}

/// `finalizepsbt` for P2PKH inputs: moves each input's signature and key
/// into its final scriptSig and drops the signing data, as Core does.
/// Returns whether every input is now signed.
pub fn finalize(psbt: &mut PartiallySignedTransaction) -> bool {
    for i in 0..psbt.inputs.len() {
        if input_signed(psbt, i) {
            continue;
        }
        let Some(script) = spent_output(psbt, i).map(|o| o.script_pubkey.clone()) else {
            continue;
        };
        let input = &mut psbt.inputs[i];
        let Some(pk) = p2pkh_key(&script, input.partial_sigs.keys()).copied() else {
            continue;
        };
        let sig = input.partial_sigs[&pk];
        let Ok(sig_push) = PushBytesBuf::try_from(sig.to_vec()) else {
            continue;
        };
        let Ok(pk_push) = PushBytesBuf::try_from(pk.to_bytes()) else {
            continue;
        };
        input.final_script_sig = Some(
            Builder::new()
                .push_slice(sig_push)
                .push_slice(pk_push)
                .into_script(),
        );
        input.partial_sigs.clear();
        input.sighash_type = None;
        input.redeem_script = None;
        input.bip32_derivation.clear();
    }
    (0..psbt.inputs.len()).all(|i| input_signed(psbt, i))
}

/// The network transaction of a fully signed PSBT.
pub fn extract(psbt: &PartiallySignedTransaction) -> Result<Transaction, PsbtError> {
    if !(0..psbt.inputs.len()).all(|i| input_signed(psbt, i)) {
        return Err(PsbtError::NotComplete);
    }
    Ok(psbt.clone().extract_tx())
}

/// Fee rate of a complete PSBT in duffs per 1000 bytes, from the extracted
/// transaction's size and the verified input values; `None` when the fee is
/// unknown.
pub fn fee_rate_per_kb(psbt: &PartiallySignedTransaction, tx: &Transaction) -> Option<u64> {
    let fee = analyze(psbt, Network::Mainnet).fee?;
    let size = dashcore::consensus::serialize(tx).len() as u64;
    Some(fee.saturating_mul(1000) / size.max(1))
}

/// The P2PKH key hash a script pays, for callers mapping scripts to wallet
/// keys.
pub fn p2pkh_hash(script: &ScriptBuf) -> Option<PubkeyHash> {
    script
        .is_p2pkh()
        .then(|| PubkeyHash::from_slice(&script.as_bytes()[3..23]).ok())
        .flatten()
}

#[cfg(test)]
mod tests;

// DELETE AT THE PIN MOVE (E0-10c): key-wallet's psbt module goes away.
#[cfg(test)]
mod old_vs_vendored;
