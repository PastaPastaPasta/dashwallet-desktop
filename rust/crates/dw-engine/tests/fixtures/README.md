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
