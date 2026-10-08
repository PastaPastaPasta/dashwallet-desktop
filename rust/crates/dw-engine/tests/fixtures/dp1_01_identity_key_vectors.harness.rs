//! DP1-01 vector harness: an example for a scratch platform checkout
//! (`packages/rs-platform-wallet-ffi/examples/`, see fixtures/README.md).
//! Calls the FFI entry points the iOS wallet calls through SwiftDashSDK,
//! with a resolver and a persister standing in for the iOS Keychain:
//!
//! - `dash_sdk_derive_and_persist_identity_keys` (Swift
//!   `prePersistIdentityKeysForRegistration`, key ids 0..keyCount): the
//!   per-key (key_type, purpose, security_level) bytes come from Rust;
//! - `dash_sdk_derive_identity_key_at_slot_with_resolver` (Swift
//!   `deriveIdentityAuthKeyAtSlot`, the DashPay pair and the upgrader);
//! - `dash_sdk_derive_identity_key_at_slot`, the mnemonic+passphrase
//!   variant of the same derivation, for the passphrase case.
//!
//! Prints paths and public keys only. The persister receives private key
//! bytes from the FFI; this harness never reads them, and the at-slot row's
//! WIF and scalar are freed unread.

use std::ffi::{c_char, c_void, CStr};

use platform_wallet_ffi::derive_and_persist_callbacks::{
    dash_sdk_identity_key_persister_create, dash_sdk_identity_key_persister_destroy,
    PersistKeyArgs, PERSIST_KEY_SUCCESS,
};
use platform_wallet_ffi::derive_identity_key_at_slot::{
    dash_sdk_derive_identity_key_at_slot, dash_sdk_derive_identity_key_at_slot_free,
    dash_sdk_derive_identity_key_at_slot_with_resolver,
};
use platform_wallet_ffi::identity_derive_and_persist::dash_sdk_derive_and_persist_identity_keys;
use platform_wallet_ffi::identity_key_preview::IdentityKeyPreviewFFI;
use platform_wallet_ffi::identity_keys_from_mnemonic::dash_sdk_derive_identity_keys_from_mnemonic_free;
use platform_wallet_ffi::identity_registration_with_signer::IdentityRegistrationKeyDerivationsFFI;
use platform_wallet_ffi::types::FFINetwork;
use rs_sdk_ffi::{dash_sdk_mnemonic_resolver_create, dash_sdk_mnemonic_resolver_destroy};

/// BIP-39 English test vectors (public; throwaway).
const MNEMONICS: [(&str, &str); 2] = [
    (
        "abandon",
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
    ),
    (
        "legal",
        "legal winner thank year wave sausage worth useful legal winner thank yellow",
    ),
];

unsafe extern "C" fn resolve(
    ctx: *const c_void,
    _wallet_id: *const u8,
    out: *mut c_char,
    cap: usize,
    out_len: *mut usize,
) -> i32 {
    let m: &str = *(ctx as *const &str);
    if m.len() > cap {
        return 2;
    }
    std::ptr::copy_nonoverlapping(m.as_ptr(), out as *mut u8, m.len());
    *out_len = m.len();
    0
}

unsafe extern "C" fn noop_destroy(_: *mut c_void) {}

struct Row {
    key_id: u32,
    key_type: u8,
    purpose: u8,
    security_level: u8,
    path: String,
    pubkey: String,
}

unsafe extern "C" fn persist(ctx: *const c_void, args: *const PersistKeyArgs) -> u8 {
    let rows = &mut *(ctx as *mut Vec<Row>);
    let a = &*args;
    let pubkey = std::slice::from_raw_parts(a.public_key_bytes, a.public_key_len);
    rows.push(Row {
        key_id: a.key_id,
        key_type: a.key_type,
        purpose: a.purpose,
        security_level: a.security_level,
        path: CStr::from_ptr(a.derivation_path_cstr).to_str().unwrap().to_owned(),
        pubkey: hex::encode(pubkey),
    });
    PERSIST_KEY_SUCCESS
}

/// The passphrase case: only the mnemonic+passphrase at-slot FFI takes one
/// (the resolver entry points derive with "" at this pin).
fn passphrase_case() {
    let mnemonic = std::ffi::CString::new(MNEMONICS[1].1).unwrap();
    let passphrase = std::ffi::CString::new("TREZOR").unwrap();
    for identity_index in [0u32, 1] {
        for key_id in 0..6 {
            unsafe {
                let mut row = IdentityKeyPreviewFFI::empty();
                let r = dash_sdk_derive_identity_key_at_slot(
                    mnemonic.as_ptr(),
                    passphrase.as_ptr(),
                    FFINetwork::Testnet,
                    identity_index,
                    key_id,
                    &mut row,
                );
                assert!(r.code as u32 == 0, "at_slot failed");
                let path = CStr::from_ptr(row.derivation_path).to_str().unwrap().to_owned();
                let pubkey =
                    hex::encode(std::slice::from_raw_parts(row.public_key, row.public_key_len));
                dash_sdk_derive_identity_key_at_slot_free(&mut row);
                println!(
                    "slot legal+TREZOR testnet i={identity_index} id={key_id} path={path} pub={pubkey}"
                );
            }
        }
    }
}

fn main() {
    passphrase_case();
    let wallet_id = [7u8; 32];
    for (label, mnemonic) in MNEMONICS {
        for (net_name, net) in [
            ("mainnet", FFINetwork::Mainnet),
            ("testnet", FFINetwork::Testnet),
            ("regtest", FFINetwork::Regtest),
        ] {
            for identity_index in [0u32, 1] {
                unsafe {
                    let mut m: &str = mnemonic;
                    let resolver = dash_sdk_mnemonic_resolver_create(
                        &mut m as *mut &str as *mut c_void,
                        resolve,
                        noop_destroy,
                    );
                    let mut rows: Vec<Row> = Vec::new();
                    let persister = dash_sdk_identity_key_persister_create(
                        &mut rows as *mut Vec<Row> as *mut c_void,
                        persist,
                        noop_destroy,
                    );
                    let mut out = IdentityRegistrationKeyDerivationsFFI {
                        items: std::ptr::null_mut(),
                        count: 0,
                    };
                    let r = dash_sdk_derive_and_persist_identity_keys(
                        net,
                        wallet_id.as_ptr(),
                        identity_index,
                        4,
                        resolver,
                        persister,
                        &mut out,
                    );
                    assert!(r.code as u32 == 0, "derive_and_persist failed");
                    assert_eq!(out.count, 4);
                    dash_sdk_derive_identity_keys_from_mnemonic_free(&mut out);
                    dash_sdk_identity_key_persister_destroy(persister);
                    for row in &rows {
                        println!(
                            "base {label} {net_name} i={identity_index} id={} type={} purpose={} sec={} path={} pub={}",
                            row.key_id, row.key_type, row.purpose, row.security_level, row.path, row.pubkey
                        );
                    }
                    let slots: &[u32] = if net_name == "testnet" && identity_index == 0 {
                        &[4, 5, 6, 7]
                    } else {
                        &[4, 5]
                    };
                    for &key_id in slots {
                        let mut row = IdentityKeyPreviewFFI::empty();
                        let r = dash_sdk_derive_identity_key_at_slot_with_resolver(
                            net,
                            wallet_id.as_ptr(),
                            resolver,
                            identity_index,
                            key_id,
                            &mut row,
                        );
                        assert!(r.code as u32 == 0, "at_slot failed");
                        let path = CStr::from_ptr(row.derivation_path).to_str().unwrap().to_owned();
                        let pubkey = hex::encode(std::slice::from_raw_parts(
                            row.public_key,
                            row.public_key_len,
                        ));
                        dash_sdk_derive_identity_key_at_slot_free(&mut row);
                        println!(
                            "slot {label} {net_name} i={identity_index} id={key_id} path={path} pub={pubkey}"
                        );
                    }
                    dash_sdk_mnemonic_resolver_destroy(resolver);
                }
            }
        }
    }
}
