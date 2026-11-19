//! Fuzz harnesses for the parsers that take untrusted input (H-02,
//! docs/ci.md "Fuzzing"). Each function feeds one channel of outside data
//! to the entry points the app calls with it, as the app calls them, and
//! panics only on a bug: a panic in a parser, or a broken invariant checked
//! here. `fuzz_targets/` runs them under libFuzzer (`--features libfuzzer`,
//! nightly, scripts/fuzz.sh); `tests/replay.rs` runs every seed and every
//! saved finding on stable.

use dw_compat::{bip39core, descriptor, dump, walletdat};
use dw_uri::deeplink::{
    DashPayContactCode, DashPayUserLink, DeepLink, classify_link, dpns_label, normalize_invitation,
};
use dw_uri::redact::{REDACTED, redact_links, redacted_whole};
use dw_uri::{Network, core, ext, keyio};

pub mod seeds;

const NETWORKS: [Network; 4] = [
    Network::Mainnet,
    Network::Testnet,
    Network::Devnet,
    Network::Regtest,
];

/// The fuzz targets, by name (a `fuzz_targets/` file and a seed directory).
pub const TARGETS: [&str; 9] = [
    "link",
    "send_input",
    "qr_image",
    "psbt",
    "dumpwallet",
    "listdescriptors",
    "walletdat",
    "avatar",
    "invitation",
];

/// Targets that need the `engine` feature.
pub const ENGINE_TARGETS: [&str; 2] = ["avatar", "invitation"];

/// Runs the target `name` on `data`; `false` for an unknown name, or an
/// engine target without the `engine` feature.
pub fn run(name: &str, data: &[u8]) -> bool {
    match name {
        "link" => link(data),
        "send_input" => send_input(data),
        "qr_image" => qr_image(data),
        "psbt" => psbt(data),
        "dumpwallet" => dumpwallet(data),
        "listdescriptors" => listdescriptors(data),
        "walletdat" => walletdat(data),
        #[cfg(feature = "engine")]
        "avatar" => avatar(data),
        #[cfg(feature = "engine")]
        "invitation" => invitation(data),
        _ => return false,
    }
    true
}

/// An opened, dropped, pasted or scanned link (shell/links.rs, the link
/// stash, the contact and invitation intake). A link the stash keeps is cut
/// whole from a log line (DASHPAY §3.8).
pub fn link(data: &[u8]) {
    let text = String::from_utf8_lossy(data);
    let link = classify_link(&text);
    let _ = normalize_invitation(&text);
    if let Some(user) = DashPayUserLink::parse(&text) {
        let _ = (user.identity_base58(), user.to_uri());
    }
    let _ = DashPayContactCode::parse(&text);
    let _ = ext::parse_payment_uri(&text);
    let _ = ext::parse_payment_string(&text);
    let _ = ext::remove_percent_encoding(&text);
    let _ = dpns_label(&text);
    let _ = redact_links(&text);

    let trimmed = text.trim();
    let kept = !trimmed.chars().any(char::is_control)
        && redacted_whole(trimmed)
        && match &link {
            DeepLink::Payment(req) => req.address.is_some(),
            DeepLink::DashPayUser(_) | DeepLink::Invitation(_) => true,
            _ => false,
        };
    if kept {
        for before in ["link ", "link\x1b[2m=\x1b[0m"] {
            let line = format!("{before}{trimmed} tail");
            let redacted = redact_links(&line);
            assert!(
                redacted.ends_with(REDACTED) && redacted.len() <= before.len() + REDACTED.len(),
                "a kept link survives redaction: {line:?} -> {redacted:?}"
            );
        }
    }
}

/// Text pasted or dropped on the Send screen (dash-qt's URI handler) and
/// typed into the address field; a WIF from a dump or a sweep.
pub fn send_input(data: &[u8]) {
    let text = String::from_utf8_lossy(data);
    if let Some(recipient) = core::parse_bitcoin_uri(&text) {
        let _ = core::parse_bitcoin_uri(&core::format_bitcoin_uri(&recipient));
    }
    for network in NETWORKS {
        if let Ok(recipient) = core::handle_uri(&text, network) {
            let _ = core::format_request_uri(&recipient, Some("alice"));
        }
        let _ = keyio::classify_address(&text, network);
        let _ = keyio::decode_platform_destination(&text, network);
        let _ = keyio::decode_secret(&text, network);
        if let Ok(dest) = keyio::decode_destination(&text, network) {
            let again =
                keyio::decode_destination(&keyio::encode_destination(&dest, network), network);
            assert_eq!(again.ok(), Some(dest), "{text:?} on {network:?}");
        }
    }
    let _ = keyio::locate_bech32_errors(&text);
    if let Some(duffs) = ext::duffs_from_decimal_dash(&text) {
        let written = ext::dash_string_from_duffs(duffs);
        assert_eq!(
            ext::duffs_from_decimal_dash(&written),
            Some(duffs),
            "{text:?}"
        );
    }
    let _ = dw_uri::qr::qr_matrix(&text);
}

/// An image file, a dropped or pasted image, or a screen region, scanned
/// for QR codes (IOS-043).
pub fn qr_image(data: &[u8]) {
    let _ = dw_desktop::qr::decode(data);
}

/// A PSBT file or clipboard text, and what the PSBT dialog, signing and
/// broadcasting read from it.
pub fn psbt(data: &[u8]) {
    let Ok(psbt) = dw_psbt::parse(data) else {
        return;
    };
    let bytes = dw_psbt::to_bytes(&psbt);
    assert_eq!(
        dw_psbt::parse(&bytes).ok().as_ref(),
        Some(&psbt),
        "binary round trip"
    );
    assert_eq!(
        dw_psbt::parse(dw_psbt::to_base64(&psbt).as_bytes())
            .ok()
            .as_ref(),
        Some(&psbt),
        "base64 round trip"
    );
    let _ = dw_psbt::unsigned_txid(&psbt);
    let _ = dw_psbt::estimated_size(&psbt);
    for network in NETWORKS {
        let _ = dw_psbt::analyze(&psbt, network);
    }
    for i in 0..psbt.inputs.len() {
        let _ = dw_psbt::spent_output(&psbt, i);
        let _ = dw_psbt::input_signed(&psbt, i);
    }
    let _ = dw_psbt::spent_output(&psbt, psbt.inputs.len());
    for out in &psbt.unsigned_tx.output {
        let _ = dw_psbt::p2pkh_hash(&out.script_pubkey);
    }
    let mut finalized = psbt;
    if dw_psbt::finalize(&mut finalized)
        && let Ok(tx) = dw_psbt::extract(&finalized)
    {
        let _ = dw_psbt::fee_rate_per_kb(&finalized, &tx);
    }
}

/// A `dumpwallet` file to import or inspect.
pub fn dumpwallet(data: &[u8]) {
    let text = String::from_utf8_lossy(data);
    let label = dump::decode_dump_string(&text);
    assert_eq!(
        dump::decode_dump_string(&dump::encode_dump_string(&label)),
        label
    );
    let Ok(file) = dump::parse(&text) else {
        return;
    };
    for key in &file.keys {
        let _ = key.is_hd();
        for network in NETWORKS {
            let _ = key.secret(network);
        }
    }
    if let Some(hd) = &file.hd {
        let _ = bip39core::validate(&hd.mnemonic);
    }
    for network in NETWORKS {
        let _ = dump::write(&file, network);
    }
}

/// `listdescriptors true` output pasted as key material, and a descriptor.
pub fn listdescriptors(data: &[u8]) {
    if let Ok(listed) = descriptor::parse_listdescriptors(data) {
        let _ = (listed.external.chain_path(), listed.external.is_private());
        let _ = listed.external.to_string_with_checksum();
    }
    let text = String::from_utf8_lossy(data);
    let _ = descriptor::checksum(&text);
    let _ = descriptor::strip_checksum(&text);
    if let Ok(parsed) = descriptor::PkhDescriptor::parse(&text) {
        let written = parsed.to_string_with_checksum();
        assert!(
            descriptor::PkhDescriptor::parse(&written).is_ok_and(|again| again == parsed),
            "a written descriptor reads back"
        );
    }
}

/// Most key derivation rounds this harness spends on a passphrase. Core
/// writes 25 000 or more; the parser allows up to `MAX_ROUNDS` (10 M, about
/// a second), which libFuzzer would only report as slow.
const FUZZ_ROUNDS: u32 = 50_000;

/// A Core-serialized `compactSize`-prefixed field.
fn field<'a>(data: &mut &'a [u8]) -> Option<&'a [u8]> {
    let (&first, rest) = data.split_first()?;
    let (len, rest) = match first {
        0xfd => {
            let (n, rest) = rest.split_first_chunk::<2>()?;
            (u16::from_le_bytes(*n) as usize, rest)
        }
        0xfe | 0xff => {
            let (n, rest) = rest.split_first_chunk::<4>()?;
            (u32::from_le_bytes(*n) as usize, rest)
        }
        n => (n as usize, rest),
    };
    let (value, rest) = rest.split_at_checked(len)?;
    *data = rest;
    Some(value)
}

/// An SQLite descriptor wallet.dat to import: its `main` table's records
/// (SQLite itself is fuzzed upstream). The input is a passphrase, then
/// `(key, value)` records, each field `compactSize`-prefixed.
pub fn walletdat(data: &[u8]) {
    let mut rest = data;
    let Some(passphrase) = field(&mut rest) else {
        return;
    };
    let mut records = Vec::new();
    while let (Some(key), Some(value)) = (field(&mut rest), field(&mut rest)) {
        records.push((key, value));
    }
    let Ok(wallet) = walletdat::SqliteWallet::from_records(records) else {
        return;
    };
    let _ = (wallet.encrypted(), wallet.is_descriptor_wallet());
    let _ = (wallet.has_mnemonic(), wallet.external_descriptor());
    let _ = (wallet.next_index(), wallet.birth_time());
    let _ = wallet.hd_root(None);
    if wallet.derivation_rounds() <= FUZZ_ROUNDS {
        let _ = wallet.hd_root(Some(passphrase));
    }
}

/// Avatar bytes fetched from a profile's URL, with the URL's Android zoom
/// crop, and a picture file the user picks, with its crop rectangle. The
/// input is a flag byte (bit 0: fingerprint), the rectangle (four
/// little-endian `u32`), a length byte and the zoom text, then the image.
#[cfg(feature = "engine")]
pub fn avatar(data: &[u8]) {
    let Some((&flags, rest)) = data.split_first() else {
        return;
    };
    let Some((rect, rest)) = rest.split_first_chunk::<16>() else {
        return;
    };
    let Some((&zoom_len, rest)) = rest.split_first() else {
        return;
    };
    let Some((zoom, image)) = rest.split_at_checked(usize::from(zoom_len)) else {
        return;
    };
    let rect: [u32; 4] =
        std::array::from_fn(|i| u32::from_le_bytes(rect[i * 4..i * 4 + 4].try_into().unwrap()));
    let zoom = String::from_utf8_lossy(zoom);
    dw_engine::platform::fuzzing::avatar(image, &zoom, rect, flags & 1 == 1);
}

/// An invitation link through the claim's parser.
#[cfg(feature = "engine")]
pub fn invitation(data: &[u8]) {
    let _ = dw_engine::platform::fuzzing::invitation(&String::from_utf8_lossy(data));
}
