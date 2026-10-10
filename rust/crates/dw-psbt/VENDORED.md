# Vendored code in dw-psbt

`src/psbt/` is the PSBT container from rust-dashcore's `key-wallet` crate (its `psbt` module),
copied so dw-psbt no longer depends on a module that rust-dashcore #1041
deleted (platform #5307 pins a rust-dashcore without it).

- **Source:** `key-wallet/src/psbt/` in <https://github.com/dashpay/rust-dashcore>
  at `40268cc0402a8933ec539f16b2d634c4e25876ad` (9 files, 4,091 lines).
- **Licence:** the upstream files carry `SPDX-License-Identifier: CC0-1.0`
  (rust-bitcoin's PSBT code, public domain). The headers are kept; each
  vendored file names its upstream path and what was removed. The rest of the
  crate is MIT like the workspace.
- **Wire format:** unchanged for everything Dash Core's BIP174 subset uses
  (see Taproot below). `PartiallySignedTransaction::{serialize,
  deserialize, from_unsigned_tx, extract_tx}`, `Input`, `Output`,
  `PsbtSighashType` and the `raw` and `serialize` modules keep their upstream
  names and behavior.

## What is here

| File | From | Kept |
| --- | --- | --- |
| `psbt/mod.rs` | `mod.rs` | the struct, `from_unsigned_tx`, `extract_tx` |
| `psbt/error.rs` | `error.rs` | errors the kept code can return |
| `psbt/raw.rs` | `raw.rs` | raw `Key`, `Pair`, `ProprietaryKey` |
| `psbt/serialize.rs` | `serialize.rs` | BIP174 (de)serialization of the kept types |
| `psbt/macros.rs` | `macros.rs` | the (de)serialization macros |
| `psbt/map/{mod,global,input,output}.rs` | `map/*` | the global, input and output maps |

## What was removed

- **Signing**: `sign`, `sighash_ecdsa`, `spend_utxo`, `GetKey`, `KeyRequest`,
  `SignError`, `OutputType` and the rest of `mod.rs` from line 216. The desktop
  signs `SIGHASH_ALL` itself (`dw_psbt::sign`).
- **Combine, fee and funding-UTXO helpers**: `combine`, `fee`,
  `iter_funding_utxos`, and the errors only they returned (dw-psbt uses none).
- **Taproot**: the `tap_*` input and output fields, their (de)serialization,
  errors and tests. Dash has no taproot, and Dash Core's BIP174 subset does
  not use these types. A PSBT carrying one now keeps the pair in `unknown`
  verbatim instead of parsing it. It is written back unchanged, but sorted
  among the other unknown pairs, after the proprietary ones, where upstream
  wrote a parsed taproot field in its own slot among the known fields.
  Malformed taproot data that upstream rejected is no longer rejected.
- **serde** derives, `FromStr` for `PsbtSighashType`, `Psbt` alias,
  `serialize_hex`, `hex_psbt`.

## Other edits

- Paths: `crate::bip32` is `key_wallet::bip32` (types are the same ones
  dw-engine uses); `dashcore_hashes`, `secp256k1` and `io` come through
  `dashcore`.
- `raw::Key`'s `Display` writes the hex itself instead of `DisplayHex`, with
  identical output.
- `macros.rs`: `$crate::dashcore::…` is `dashcore::…`, `$crate::prelude::Vec`
  is `Vec`, `$crate::prelude::btree_map` is `std::collections::btree_map`.
- Also removed: the `Prevouts` re-export, `From<TapSighashType>`,
  `Input::{ecdsa_hash_ty, taproot_hash_ty}`, `PsbtSighashType::taproot_hash_ty`
  and the unused `Deserialize for raw::Pair`.
- `PsbtSighashType` is written against the sighash types that exist at **both**
  rust-dashcore 40268cc0 and the later revs, where they come from
  bitcoin-crypto and `NonStandardSighashType`/`sighash::Error::InvalidSighashType`
  changed shape: `From<EcdsaSighashType>` uses `to_u32()`, `ecdsa_hash_ty`
  returns an `Option`, and `Display` is a plain match with upstream's text
  (`SIGHASH_ALL`, `SIGHASH_NONE|SIGHASH_ANYONECANPAY`, …, `0x…` otherwise).
- `raw.rs`: one `return Err(..)?` lost its `return` (clippy 1.98).
- Source formatted with the workspace's `rustfmt`.

## Checks against the old module

Until the pin move (E0-10c), `src/old_vs_vendored.rs` compared this code with
key-wallet's `psbt` module at rust-dashcore 40268cc0 over the dashd vectors, a
PSBT with every kept field set, every truncation and every single-byte flip of
those. The move deleted it with the module; nothing in the workspace refers to
key-wallet's `psbt` module any more.

## Updating

Don't track upstream: rust-dashcore deleted the module. Fix bugs here.
