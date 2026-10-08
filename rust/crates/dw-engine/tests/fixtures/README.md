# Test fixtures

## `e0_03_signer_vectors.json`

Reference outputs for the vault-backed Platform signers (roadmap E0-03, `tests/e0_03_signers.rs`).

The library's test `SeedCryptoProvider` produced them (`rs-platform-wallet/src/wallet/identity/network/contact_requests.rs:171-345`
at platform `bc321362b9ff9c8ac24244f9d4b09ec3de7a5938`). That provider is `#[cfg(test)] pub(crate)`, so the
desktop cannot call it. The vectors come from a one-off test inside a scratch checkout of that revision. The
upstream tree was never modified or pushed. The test is `e0_03_signer_vectors.generator.patch`.

The vectors cover:

- two seeds: the BIP39 `abandon … about` phrase with no passphrase, and `legal winner … yellow` with passphrase
  `TREZOR`;
- testnet and mainnet;
- the identity keys 0–5 of identities 0 and 1: public key, HASH160, and the `dashcore::signer::sign` signature
  over `constants.identity_data`;
- DIP-15 receiving xpubs, accounts 0 and 1, user `0x11…`, friend `0x22…`;
- the BIP44 account-0 xpub;
- the auto-accept public key and exported key at expiry `1900000000`;
- ECDH with the public key of `constants.peer_secret`;
- account references and their unmask;
- contactInfo seal and open.

To regenerate:

```sh
git -C <platform checkout> worktree add --detach /tmp/platform-vectors bc321362b9ff9c8ac24244f9d4b09ec3de7a5938
git -C /tmp/platform-vectors apply <this dir>/e0_03_signer_vectors.generator.patch
cd /tmp/platform-vectors && CARGO_TARGET_DIR=/tmp/platform-vectors-target \
  E0_03_VECTORS_OUT=<this dir>/e0_03_signer_vectors.json \
  cargo test -p platform-wallet --lib e0_03_vectors
git -C <platform checkout> worktree remove --force /tmp/platform-vectors
```

## `dp1_01_identity_key_vectors.json`

The DashPay identity key sets iOS registers (roadmap DP1-01, `tests/dp1_01_identity_keys.rs`): for each seed, network
and identity index, keys 0–5 with id, purpose, security level, key type, contract bounds, DIP-13 path and public key.

- iOS: dashwallet-ios `37c0e78a2f557fa35c98c051a4ce40b77be350ee` (`DWDashPayIdentityKeys.swift` last changed in
  `6836db798b`). iOS builds against a sibling `../platform` checkout, not a pinned revision.
- Platform: `bc41f1bc233dec4607d387101c1d9c2f111019b2`, the desktop's pin.
- Seeds: the BIP39 `abandon … about` and `legal winner … yellow` phrases with no passphrase (the only kind iOS can
  derive from at this platform revision: its resolver entry points use `to_seed("")`) on mainnet, testnet and regtest,
  identities 0 and 1; and `legal winner … yellow` with passphrase `TREZOR` on testnet, which iOS cannot produce yet.
- `upgrade_slots`: ids 6 and 7 of `abandon … about`, testnet, identity 0, the slots the IdentityUpdate fallback picks
  in the upgrade test.

Each key's `sources` says how it was obtained:

- `python-independent`: `dp1_01_identity_key_vectors.derive.py`, a from-scratch BIP39 / BIP32 / secp256k1 derivation
  (standard library only) of every path and public key. Its key table is transcribed from the Swift and FFI sources.
- `rs-platform-wallet-ffi …`: `dp1_01_identity_key_vectors.harness.rs`, which calls the entry points SwiftDashSDK
  calls on iOS, with a test resolver and persister in place of the Keychain. Keys 0–3 come from
  `dash_sdk_derive_and_persist_identity_keys` (Swift `prePersistIdentityKeysForRegistration`, `keyCount` 4), which
  also decides their key type, purpose and security level. Keys 4–7 come from
  `dash_sdk_derive_identity_key_at_slot_with_resolver` (Swift `deriveIdentityAuthKeyAtSlot`). The passphrase case uses
  `dash_sdk_derive_identity_key_at_slot`, which shares its derivation with the resolver variant.
- `DWDashPayIdentityKeys.swift (source reading)`: the purpose, security level, key type and bounds of keys 4 and 5 are
  Swift constants, so no executable source exists for them on Linux. A Mac can confirm them with iOS's
  `DashPayIdentityKeysTests`.
- `e0_03_signer_vectors.json`: the key also appears there, with the same public key.

`dp1_01_identity_key_vectors.build.py` fails unless the Python and FFI public keys and paths agree for every key (and
for keys 0–3, the metadata too), and the E0-03 vectors where they overlap. Then it writes the fixture.

To regenerate (prints public keys and paths only):

```sh
git -C <platform checkout> worktree add --detach /tmp/platform-dp101 bc41f1bc233dec4607d387101c1d9c2f111019b2
cp <this dir>/dp1_01_identity_key_vectors.harness.rs /tmp/platform-dp101/packages/rs-platform-wallet-ffi/examples/dp101_vectors.rs
cd /tmp/platform-dp101 && CARGO_TARGET_DIR=/tmp/platform-dp101-target \
  cargo run -p platform-wallet-ffi --example dp101_vectors > /tmp/dp101-ffi.txt
python3 -I <this dir>/dp1_01_identity_key_vectors.derive.py > /tmp/dp101-python.json
python3 -I <this dir>/dp1_01_identity_key_vectors.build.py /tmp/dp101-python.json /tmp/dp101-ffi.txt \
  <this dir>/e0_03_signer_vectors.json <this dir>/dp1_01_identity_key_vectors.json
git -C <platform checkout> worktree remove --force /tmp/platform-dp101
```
