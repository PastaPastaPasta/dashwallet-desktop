//! App Connect v2 login requests (DIP pasta-app-connect, "connect URI" and
//! "Request Body"): the `dashpay://connect` carrier, the transitional
//! `dash-key:` v2 carrier, and the request body codec.
//!
//! Which refusal is `unsupported` and which `invalid`, and in what order the
//! checks run, follow the iOS wallet (`DashConnectUri.swift`,
//! `AppConnectRequest.swift`). Framing errors refuse at once; rule
//! violations are noted and raised after the last TLV, so a later unknown
//! mandatory part is reported as `unsupported` first.
//!
//! The deployment subset (DIP "Deployment and Compatibility") is
//! [`AppConnectRequest::check_supported`], kept apart from the format check.
//! One known deviation from the DIP, for iOS parity: CRITICAL on a binding
//! other than a contract group, and a third `ENC_BINDING`, are `invalid`
//! (as wallets built before the DIP answer), not `unsupported`.

use dashcore::hashes::{Hash, hash160, sha256d};

use crate::Network;

/// The whole `dashpay://connect` URI, in UTF-8 bytes, checked first.
pub const MAX_CONNECT_URI_BYTES: usize = 4096;
/// A request body on any carrier.
pub const MAX_BODY_BYTES: usize = 1024;
/// `r`, before it is decoded: 1,024 bytes of unpadded base64url.
const MAX_R_CHARS: usize = 1366;
/// The `dash-key:` query, in Unicode scalars, before it is split.
const MAX_KEY_QUERY_SCALARS: usize = 4096;
/// The `dash-key:` Base58 body before it is decoded: 1.366 characters per
/// byte rounded up, plus slack for leading zeros.
const MAX_KEY_BODY_CHARS: usize = (MAX_BODY_BYTES * 1366).div_ceil(1000) + 8;
const MAX_CALLBACK_UTF16: usize = 512;
/// How much of the text after `dashpay://` the verb classifier reads.
const MAX_AUTHORITY_SCALARS: usize = 256;

const DASHPAY_PREFIX: &str = "dashpay://";
const DASH_KEY_PREFIX: &str = "dash-key:";
const CONNECT_HOST: &str = "connect";
const OTHER_KNOWN_HOSTS: [&str; 2] = ["invite", "user"];

const VERSION: u8 = 0x02;
const PUBKEY_LEN: usize = 33;
const ID_LEN: usize = 32;
const HEADER_LEN: usize = 1 + PUBKEY_LEN + ID_LEN + 1;
const MAX_LABEL_BYTES: usize = 64;
const MAX_DOCUMENT_TYPE_NAME: usize = 64;
const AUTH_BINDING_FIXED: usize = 1 + 1 + ID_LEN + 8 + 8;
const MAX_AUTH_BINDINGS: usize = 4;
const MAX_ENC_BINDINGS: usize = 2;
const MAX_TIMESTAMP: u64 = (1 << 53) - 1;
const MAX_AGE_MS: u64 = 10 * 60 * 1000;
const MAX_SKEW_MS: u64 = 2 * 60 * 1000;

const TLV_IDENTITY_HINT: u8 = 0x01;
const TLV_AUTH_BINDING: u8 = 0x02;
const TLV_CONNECTION_ID: u8 = 0x03;
const TLV_ENC_BINDING: u8 = 0x04;
const TLV_MODE: u8 = 0x05;
const TLV_CREATED_AT: u8 = 0x06;
const MODE_RENEW: u8 = 1;

/// The URI a request arrived in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Carrier {
    DashPayConnect,
    DashKey,
}

/// The `n` parameter: `m`, `t` or `d`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkSel {
    Mainnet,
    Testnet,
    Devnet,
}

impl NetworkSel {
    pub fn from_code(code: &str) -> Option<NetworkSel> {
        match code {
            "m" => Some(NetworkSel::Mainnet),
            "t" => Some(NetworkSel::Testnet),
            "d" => Some(NetworkSel::Devnet),
            _ => None,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            NetworkSel::Mainnet => "m",
            NetworkSel::Testnet => "t",
            NetworkSel::Devnet => "d",
        }
    }

    /// Whether the wallet's `network` is the one asked for. Regtest counts
    /// as a devnet.
    pub fn matches(self, network: Network) -> bool {
        matches!(
            (self, network),
            (NetworkSel::Mainnet, Network::Mainnet)
                | (NetworkSel::Testnet, Network::Testnet)
                | (NetworkSel::Devnet, Network::Devnet | Network::Regtest)
        )
    }
}

/// The `ac` value of a return link (DIP "Status Codes").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Status {
    Ok,
    Declined,
    Stale,
    Unsupported,
    Invalid,
    Failed,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Declined => "declined",
            Status::Stale => "stale",
            Status::Unsupported => "unsupported",
            Status::Invalid => "invalid",
            Status::Failed => "failed",
        }
    }
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why a request was refused. `detail` is for logs and debugging; it never
/// carries the body or an unbounded value from the URI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub status: Status,
    pub detail: String,
}

impl Refusal {
    fn invalid(detail: impl Into<String>) -> Refusal {
        Refusal {
            status: Status::Invalid,
            detail: detail.into(),
        }
    }

    fn unsupported(detail: impl Into<String>) -> Refusal {
        Refusal {
            status: Status::Unsupported,
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.status, self.detail)
    }
}

impl std::error::Error for Refusal {}

/// A parsed App Connect URI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectUri {
    pub carrier: Carrier,
    pub network: NetworkSel,
    pub request: AppConnectRequest,
    pub callback: Option<Callback>,
}

/// A checked `cb` return link: an absolute `https` URL as written, with no
/// userinfo, whose host is a plain lowercase DNS name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Callback {
    url: String,
    host: String,
    port: Option<u16>,
}

impl Callback {
    /// Strict RFC 3986 characters only, as `URLComponents` takes them
    /// without re-encoding.
    fn parse(text: &str) -> Result<Callback, Refusal> {
        let bad = || Refusal::invalid("callback is not an https URL without credentials");
        if text.encode_utf16().count() > MAX_CALLBACK_UTF16 || !is_strict_uri_text(text) {
            return Err(bad());
        }
        let (scheme, rest) = text.split_once(':').ok_or_else(bad)?;
        if !scheme.eq_ignore_ascii_case("https") {
            return Err(bad());
        }
        let rest = rest.strip_prefix("//").ok_or_else(bad)?;
        let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let (authority, tail) = rest.split_at(authority_end);
        if authority.contains('@') || tail.matches('#').count() > 1 {
            return Err(bad());
        }
        let (host, port) = match authority.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        };
        if !is_plain_host_name(host) {
            return Err(bad());
        }
        let port = match port {
            None => None,
            Some(digits) => {
                if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(bad());
                }
                Some(digits.parse::<u16>().map_err(|_| bad())?)
            }
        };
        Ok(Callback {
            url: text.to_owned(),
            host: host.to_owned(),
            port,
        })
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    /// What the approval sheet shows.
    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> Option<u16> {
        self.port
    }

    /// The link with `ac=<status>` as the last query item, replacing any
    /// `ac` the app wrote. The other query items, and a fragment, stay as
    /// written.
    pub fn with_status(&self, status: Status) -> String {
        let (rest, fragment) = match self.url.split_once('#') {
            Some((rest, fragment)) => (rest, Some(fragment)),
            None => (self.url.as_str(), None),
        };
        let (base, query) = rest.split_once('?').unwrap_or((rest, ""));
        let ac = format!("ac={}", status.as_str());
        let items: Vec<&str> = (!query.is_empty())
            .then(|| query.split('&'))
            .into_iter()
            .flatten()
            .filter(|item| {
                let name = item.split_once('=').map_or(*item, |(name, _)| name);
                form_decode(name) != "ac"
            })
            .chain([ac.as_str()])
            .collect();
        let mut out = format!("{base}?{}", items.join("&"));
        if let Some(fragment) = fragment {
            out.push('#');
            out.push_str(fragment);
        }
        out
    }
}

/// An App Connect v2 request body (DIP "Request Body").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppConnectRequest {
    pub app_eph_pub: [u8; 33],
    pub app_id: [u8; 32],
    /// The app's own claim; nothing verifies it.
    pub label: String,
    pub identity_hint: Option<[u8; 32]>,
    /// One to four; the first is the primary binding.
    pub auth_bindings: Vec<AuthBinding>,
    pub connection_id: Option<[u8; 16]>,
    pub enc_bindings: Vec<EncBinding>,
    /// `MODE = 1`.
    pub renew: bool,
    pub created_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthBinding {
    pub bounds: Bounds,
    pub level: Level,
    /// Proposed lifetime budget in credits; `None` for no proposal. Always
    /// set on a [`Bounds::Group`] binding.
    pub budget: Option<u64>,
    /// Proposed expiry as a block time; `None` for no proposal.
    pub expires_at_ms: Option<u64>,
}

/// Platform's `ContractBounds`, with its bincode tags as wire kinds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Bounds {
    Contract([u8; 32]),
    DocumentType([u8; 32], String),
    Group([u8; 32]),
}

impl Bounds {
    fn kind(&self) -> u8 {
        match self {
            Bounds::Contract(_) => 0,
            Bounds::DocumentType(..) => 1,
            Bounds::Group(_) => 2,
        }
    }

    pub fn id(&self) -> &[u8; 32] {
        match self {
            Bounds::Contract(id) | Bounds::DocumentType(id, _) | Bounds::Group(id) => id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Critical = 1,
    High = 2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncBinding {
    pub contract: [u8; 32],
    pub purpose: EncPurpose,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncPurpose {
    Encryption = 1,
    Decryption = 2,
}

/// A request's `CREATED_AT` against a block time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Age {
    Fresh,
    /// More than 10 minutes before the block time.
    Stale,
    /// More than 2 minutes after it.
    Future,
}

/// The first broken rule, raised once the last TLV has been read.
#[derive(Default)]
struct Rules(Option<String>);

impl Rules {
    fn note(&mut self, detail: impl Into<String>) {
        if self.0.is_none() {
            self.0 = Some(detail.into());
        }
    }
}

/// `hash160(app_eph_pub)`, the request's id on Platform.
pub fn request_hash(app_eph_pub: &[u8; PUBKEY_LEN]) -> [u8; 20] {
    hash160::Hash::hash(app_eph_pub).to_byte_array()
}

/// `hash256(app_eph_pub ‖ u8(binding_index))`, the session key's last
/// derivation index.
pub fn session_leaf(app_eph_pub: &[u8; PUBKEY_LEN], binding_index: u8) -> [u8; 32] {
    let mut data = [0u8; PUBKEY_LEN + 1];
    data[..PUBKEY_LEN].copy_from_slice(app_eph_pub);
    data[PUBKEY_LEN] = binding_index;
    sha256d::Hash::hash(&data).to_byte_array()
}

impl AppConnectRequest {
    pub fn decode(body: &[u8]) -> Result<AppConnectRequest, Refusal> {
        let Some(&version) = body.first() else {
            return Err(Refusal::invalid("empty request"));
        };
        if version != VERSION {
            return Err(Refusal::unsupported(format!("request version {version}")));
        }
        if body.len() < HEADER_LEN {
            return Err(Refusal::invalid("request is truncated"));
        }
        let mut rules = Rules::default();
        let app_eph_pub: [u8; 33] = body[1..1 + PUBKEY_LEN].try_into().expect("33 bytes");
        if dashcore::secp256k1::PublicKey::from_byte_array_compressed(app_eph_pub).is_err() {
            rules.note("app ephemeral public key is not a compressed secp256k1 point");
        }
        let app_id: [u8; 32] = body[1 + PUBKEY_LEN..HEADER_LEN - 1]
            .try_into()
            .expect("32 bytes");
        let label_len = usize::from(body[HEADER_LEN - 1]);
        if label_len > MAX_LABEL_BYTES {
            return Err(Refusal::invalid(format!(
                "label is longer than {MAX_LABEL_BYTES} bytes"
            )));
        }
        let mut offset = HEADER_LEN;
        let label_bytes = body
            .get(offset..offset + label_len)
            .ok_or_else(|| Refusal::invalid("request is truncated"))?;
        let label = std::str::from_utf8(label_bytes)
            .map_err(|_| Refusal::invalid("label is not valid UTF-8"))?
            .to_owned();
        if label.chars().any(is_forbidden_in_label) {
            rules.note("label contains control or bidirectional formatting characters");
        }
        offset += label_len;

        let mut request = AppConnectRequest {
            app_eph_pub,
            app_id,
            label,
            identity_hint: None,
            auth_bindings: Vec::new(),
            connection_id: None,
            enc_bindings: Vec::new(),
            renew: false,
            created_at_ms: 0,
        };
        let mut created_at = None;
        let mut previous_type = 0u8;

        while offset < body.len() {
            if offset + 2 > body.len() {
                return Err(Refusal::invalid("TLV header is truncated"));
            }
            let tlv_type = body[offset];
            let length = usize::from(body[offset + 1]);
            offset += 2;
            let value = body
                .get(offset..offset + length)
                .ok_or_else(|| Refusal::invalid(format!("TLV {tlv_type} is truncated")))?;
            offset += length;

            let known = (TLV_IDENTITY_HINT..=TLV_CREATED_AT).contains(&tlv_type);
            let repeatable = tlv_type == TLV_AUTH_BINDING
                || tlv_type == TLV_ENC_BINDING
                || (!tlv_type.is_multiple_of(2) && !known);
            if !(tlv_type > previous_type || (tlv_type == previous_type && repeatable)) {
                return Err(Refusal::invalid(format!(
                    "TLV {tlv_type} is out of order or repeated"
                )));
            }
            previous_type = tlv_type;

            if let Some(fixed) = fixed_length(tlv_type)
                && length != fixed
            {
                return Err(Refusal::invalid(format!(
                    "TLV {tlv_type} must be {fixed} bytes"
                )));
            }

            match tlv_type {
                TLV_IDENTITY_HINT => {
                    request.identity_hint = Some(value.try_into().expect("32 bytes"));
                }
                TLV_AUTH_BINDING => {
                    if request.auth_bindings.len() >= MAX_AUTH_BINDINGS {
                        return Err(Refusal::invalid(format!(
                            "more than {MAX_AUTH_BINDINGS} auth bindings"
                        )));
                    }
                    let binding = decode_auth_binding(value, &mut rules)?;
                    if request
                        .auth_bindings
                        .iter()
                        .any(|b| b.bounds == binding.bounds)
                    {
                        rules.note("duplicate auth binding");
                    }
                    request.auth_bindings.push(binding);
                }
                TLV_CONNECTION_ID => {
                    request.connection_id = Some(value.try_into().expect("16 bytes"));
                }
                TLV_ENC_BINDING => {
                    if request.enc_bindings.len() >= MAX_ENC_BINDINGS {
                        return Err(Refusal::invalid(format!(
                            "more than {MAX_ENC_BINDINGS} encryption bindings"
                        )));
                    }
                    let raw_purpose = value[ID_LEN];
                    let purpose = match raw_purpose {
                        1 => Some(EncPurpose::Encryption),
                        2 => Some(EncPurpose::Decryption),
                        _ => None,
                    };
                    let binding = EncBinding {
                        contract: value[..ID_LEN].try_into().expect("32 bytes"),
                        purpose: purpose.unwrap_or(EncPurpose::Encryption),
                    };
                    if purpose.is_none() {
                        rules.note(format!("encryption binding purpose {raw_purpose}"));
                    } else if request.enc_bindings.contains(&binding) {
                        rules.note("duplicate encryption binding");
                    }
                    request.enc_bindings.push(binding);
                }
                TLV_MODE => {
                    if value[0] != MODE_RENEW {
                        return Err(Refusal::invalid("unknown mode"));
                    }
                    request.renew = true;
                }
                TLV_CREATED_AT => {
                    let timestamp = read_timestamp(value, "created-at")?;
                    if timestamp == 0 {
                        return Err(Refusal::invalid("created-at is zero"));
                    }
                    created_at = Some(timestamp);
                }
                _ if tlv_type.is_multiple_of(2) => {
                    return Err(Refusal::unsupported(format!("mandatory TLV {tlv_type}")));
                }
                _ => {}
            }
        }

        let Some(created_at) = created_at else {
            return Err(Refusal::invalid("request has no created-at"));
        };
        if let Some(detail) = rules.0 {
            return Err(Refusal::invalid(detail));
        }
        if request.auth_bindings.is_empty() {
            return Err(Refusal::invalid("request has no auth binding"));
        }
        request.created_at_ms = created_at;
        Ok(request)
    }

    /// The canonical body: known fields only, in type order. Only a request
    /// [`AppConnectRequest::decode`] would accept encodes faithfully: lengths
    /// are single bytes, and a zero budget or expiry means none.
    pub fn encode(&self) -> Vec<u8> {
        debug_assert!(self.label.len() <= MAX_LABEL_BYTES, "label too long");
        let mut out = Vec::with_capacity(MAX_BODY_BYTES);
        out.push(VERSION);
        out.extend_from_slice(&self.app_eph_pub);
        out.extend_from_slice(&self.app_id);
        out.push(self.label.len() as u8);
        out.extend_from_slice(self.label.as_bytes());
        let mut tlv = |tlv_type: u8, value: &[u8]| {
            out.push(tlv_type);
            out.push(value.len() as u8);
            out.extend_from_slice(value);
        };
        if let Some(hint) = &self.identity_hint {
            tlv(TLV_IDENTITY_HINT, hint);
        }
        for binding in &self.auth_bindings {
            debug_assert!(
                binding.budget != Some(0) && binding.expires_at_ms != Some(0),
                "a zero limit is written as None"
            );
            let mut value = Vec::with_capacity(AUTH_BINDING_FIXED + 1 + MAX_DOCUMENT_TYPE_NAME);
            value.push(binding.bounds.kind());
            value.push(binding.level as u8);
            value.extend_from_slice(binding.bounds.id());
            value.extend_from_slice(&binding.budget.unwrap_or(0).to_be_bytes());
            value.extend_from_slice(&binding.expires_at_ms.unwrap_or(0).to_be_bytes());
            if let Bounds::DocumentType(_, name) = &binding.bounds {
                debug_assert!(
                    (1..=MAX_DOCUMENT_TYPE_NAME).contains(&name.len()),
                    "document type name length"
                );
                value.push(name.len() as u8);
                value.extend_from_slice(name.as_bytes());
            }
            tlv(TLV_AUTH_BINDING, &value);
        }
        if let Some(connection_id) = &self.connection_id {
            tlv(TLV_CONNECTION_ID, connection_id);
        }
        for binding in &self.enc_bindings {
            let mut value = binding.contract.to_vec();
            value.push(binding.purpose as u8);
            tlv(TLV_ENC_BINDING, &value);
        }
        if self.renew {
            tlv(TLV_MODE, &[MODE_RENEW]);
        }
        tlv(TLV_CREATED_AT, &self.created_at_ms.to_be_bytes());
        out
    }

    /// `hash160(appEphPub)`: the request's id on Platform.
    pub fn request_hash(&self) -> [u8; 20] {
        request_hash(&self.app_eph_pub)
    }

    /// `hash256(appEphPub ‖ u8(bindingIndex))`, the `requestId'` of the
    /// binding's session key path.
    pub fn session_leaf(&self, binding_index: u8) -> [u8; 32] {
        session_leaf(&self.app_eph_pub, binding_index)
    }

    /// Judged against a block time in milliseconds since the epoch.
    pub fn age(&self, block_time_ms: u64) -> Age {
        if block_time_ms.saturating_sub(self.created_at_ms) > MAX_AGE_MS {
            Age::Stale
        } else if self.created_at_ms.saturating_sub(block_time_ms) > MAX_SKEW_MS {
            Age::Future
        } else {
            Age::Fresh
        }
    }

    /// The subset this wallet grants: encryption bindings on `appId` only.
    /// CRITICAL off a contract group and a third encryption binding are
    /// already refused by [`AppConnectRequest::decode`], as on iOS.
    pub fn check_supported(&self) -> Result<(), Refusal> {
        if self.enc_bindings.iter().any(|b| b.contract != self.app_id) {
            return Err(Refusal::unsupported(
                "encryption binding on a contract other than the app's",
            ));
        }
        Ok(())
    }
}

fn fixed_length(tlv_type: u8) -> Option<usize> {
    match tlv_type {
        TLV_IDENTITY_HINT => Some(ID_LEN),
        TLV_CONNECTION_ID => Some(16),
        TLV_ENC_BINDING => Some(ID_LEN + 1),
        TLV_MODE => Some(1),
        TLV_CREATED_AT => Some(8),
        _ => None,
    }
}

fn decode_auth_binding(value: &[u8], rules: &mut Rules) -> Result<AuthBinding, Refusal> {
    if value.len() < AUTH_BINDING_FIXED {
        return Err(Refusal::invalid("auth binding is truncated"));
    }
    let kind = value[0];
    if kind > 2 {
        return Err(Refusal::unsupported(format!("auth binding kind {kind}")));
    }
    let level = match value[1] {
        1 => Some(Level::Critical),
        2 => Some(Level::High),
        _ => None,
    };
    if level.is_none() || (level == Some(Level::Critical) && kind != 2) {
        rules.note(format!(
            "security level {} is not allowed on this binding",
            value[1]
        ));
    }
    let id: [u8; 32] = value[2..2 + ID_LEN].try_into().expect("32 bytes");
    let budget = u64::from_be_bytes(value[34..42].try_into().expect("8 bytes"));
    let expires_at = read_timestamp(&value[42..50], "expires-at")?;
    if kind == 2 && budget == 0 {
        rules.note("a contractGroup binding must carry a budget");
    }
    let rest = &value[AUTH_BINDING_FIXED..];
    let bounds = match kind {
        0 | 2 => {
            if !rest.is_empty() {
                return Err(Refusal::invalid("binding carries trailing bytes"));
            }
            if kind == 0 {
                Bounds::Contract(id)
            } else {
                Bounds::Group(id)
            }
        }
        _ => {
            let Some((&name_len, name)) = rest.split_first() else {
                return Err(Refusal::invalid("document type name length does not match"));
            };
            if usize::from(name_len) != name.len() {
                return Err(Refusal::invalid("document type name length does not match"));
            }
            let text = std::str::from_utf8(name)
                .map_err(|_| Refusal::invalid("document type name is not valid UTF-8"))?;
            if !(1..=MAX_DOCUMENT_TYPE_NAME).contains(&name.len())
                || !name
                    .iter()
                    .all(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'-')
            {
                rules.note("document type name must be 1-64 of [A-Za-z0-9_-]");
            }
            Bounds::DocumentType(id, text.to_owned())
        }
    };
    Ok(AuthBinding {
        bounds,
        level: level.unwrap_or(Level::High),
        budget: (budget > 0).then_some(budget),
        expires_at_ms: (expires_at > 0).then_some(expires_at),
    })
}

fn read_timestamp(bytes: &[u8], what: &str) -> Result<u64, Refusal> {
    let value = u64::from_be_bytes(bytes.try_into().expect("8 bytes"));
    if value > MAX_TIMESTAMP {
        return Err(Refusal::invalid(format!("{what} is out of range")));
    }
    Ok(value)
}

/// C0 and C1 controls, DEL, and the bidirectional marks, embeddings,
/// overrides and isolates.
fn is_forbidden_in_label(c: char) -> bool {
    matches!(
        u32::from(c),
        0x00..=0x1F
            | 0x7F..=0x9F
            | 0x061C
            | 0x200E
            | 0x200F
            | 0x202A..=0x202E
            | 0x2066..=0x2069
    )
}

/// Parses an App Connect v2 URI on either carrier, surrounding whitespace
/// trimmed. A `dash-key:` URI of version 1 is `unsupported` here.
pub fn parse(uri: &str) -> Result<ConnectUri, Refusal> {
    let uri = uri.trim();
    if uri.starts_with(DASH_KEY_PREFIX) {
        parse_dash_key(uri)
    } else {
        parse_dashpay_connect(uri)
    }
}

/// Whether `s` is an App Connect link: a `dashpay://connect` URI (only the
/// verb is read), or a `dash-key:` URI with `v=2`. [`parse`] decides whether
/// it is valid.
pub fn is_connect_link(s: &str) -> bool {
    let s = s.trim();
    dashpay_host(s).as_deref() == Some(CONNECT_HOST) || is_dash_key_v2(s)
}

/// Whether `s` is a `dashpay://` link whose verb this wallet does not know
/// (`sign`, or one a later DIP adds): the user is told it needs a newer
/// wallet. `connect`, `invite` and `user` are known.
pub fn needs_newer_wallet(s: &str) -> bool {
    dashpay_host(s.trim()).is_some_and(|host| {
        !host.is_empty() && host != CONNECT_HOST && !OTHER_KNOWN_HOSTS.contains(&host.as_str())
    })
}

/// The lowercased host of a `dashpay://` URI, or `None` for another
/// string. A userinfo and a port are dropped, so `dashpay://connect:80` is
/// still `connect` and refused by the parser. Only the first
/// [`MAX_AUTHORITY_SCALARS`] scalars after the scheme are read.
fn dashpay_host(uri: &str) -> Option<String> {
    let mut chars = uri.chars();
    for expected in DASHPAY_PREFIX.chars() {
        if chars.next()?.to_ascii_lowercase() != expected {
            return None;
        }
    }
    let authority: Vec<char> = chars
        .take(MAX_AUTHORITY_SCALARS)
        .take_while(|c| !matches!(c, '/' | '?' | '#'))
        .collect();
    let after_userinfo = authority.rsplit(|c| *c == '@').next().unwrap_or_default();
    Some(
        after_userinfo
            .iter()
            .take_while(|c| **c != ':')
            .map(char::to_ascii_lowercase)
            .collect(),
    )
}

fn is_dash_key_v2(uri: &str) -> bool {
    let Some((_, query)) = uri
        .strip_prefix(DASH_KEY_PREFIX)
        .and_then(|rest| rest.split_once('?'))
    else {
        return false;
    };
    if query.chars().nth(MAX_KEY_QUERY_SCALARS).is_some() {
        return false;
    }
    let query = query.strip_prefix('?').unwrap_or(query);
    query.split('&').any(|item| {
        item.split_once('=')
            .is_some_and(|(name, value)| form_decode(name) == "v" && form_decode(value) == "2")
    })
}

fn parse_dashpay_connect(uri: &str) -> Result<ConnectUri, Refusal> {
    if uri.len() > MAX_CONNECT_URI_BYTES {
        return Err(Refusal::invalid("URI is too long"));
    }
    if dashpay_host(uri).as_deref() != Some(CONNECT_HOST) {
        return Err(Refusal::invalid("not a dashpay://connect URI"));
    }
    let rest = &uri[DASHPAY_PREFIX.len()..];
    if rest.contains('#') {
        return Err(Refusal::invalid("URI has a fragment"));
    }
    let authority_end = rest.find(['/', '?']).unwrap_or(rest.len());
    if rest[..authority_end].chars().count() != CONNECT_HOST.len() {
        return Err(Refusal::invalid("URI has userinfo or a port"));
    }
    let after = &rest[authority_end..];
    let (path, query) = after.split_at(after.find('?').unwrap_or(after.len()));
    if !(path.is_empty() || path == "/") {
        return Err(Refusal::invalid("URI has a path"));
    }
    let Some(query) = query.strip_prefix('?') else {
        return Err(Refusal::invalid("URI has no query"));
    };
    let params = Params::read(query, &["n", "v", "r", "cb"])?;
    params.finish(Carrier::DashPayConnect, |params| {
        let encoded = params
            .get("r")
            .filter(|r| !r.is_empty())
            .ok_or_else(|| Refusal::invalid("request is missing"))?;
        if encoded.len() > MAX_R_CHARS {
            return Err(Refusal::invalid("request is too long"));
        }
        base64url_decode(encoded)
            .ok_or_else(|| Refusal::invalid("request is not canonical base64url"))
    })
}

fn parse_dash_key(uri: &str) -> Result<ConnectUri, Refusal> {
    let rest = uri
        .strip_prefix(DASH_KEY_PREFIX)
        .ok_or_else(|| Refusal::invalid("not a dash-key URI"))?;
    let (body, query) = rest
        .split_once('?')
        .ok_or_else(|| Refusal::invalid("URI has no query"))?;
    if query.chars().nth(MAX_KEY_QUERY_SCALARS).is_some() {
        return Err(Refusal::invalid("query is too long"));
    }
    let query = query.strip_prefix('?').unwrap_or(query);
    let params = Params::read(query, &["n", "v", "cb"])?;
    params.finish(Carrier::DashKey, |_| {
        if body.len() > MAX_KEY_BODY_CHARS {
            return Err(Refusal::invalid("request is too long"));
        }
        dashcore::base58::decode(body).map_err(|_| Refusal::invalid("request is not base58"))
    })
}

/// The known parameters of a query, form-decoded.
struct Params(Vec<(String, String)>);

impl Params {
    /// Split on `&`; names and values form-decoded before they are
    /// compared. A repeated known parameter is invalid; others are ignored.
    fn read(query: &str, known: &[&str]) -> Result<Params, Refusal> {
        let mut params: Vec<(String, String)> = Vec::new();
        for item in query.split('&').filter(|item| !item.is_empty()) {
            let (name, value) = match item.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None => (item, None),
            };
            let name = form_decode(name);
            if !known.contains(&name.as_str()) {
                continue;
            }
            if params.iter().any(|(n, _)| *n == name) {
                return Err(Refusal::invalid(format!(
                    "query parameter {name} is repeated"
                )));
            }
            params.push((name, value.map(form_decode).unwrap_or_default()));
        }
        Ok(Params(params))
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// `v`, then `n`, then the body the carrier decodes, then `cb`, then
    /// the body itself. `dash-key:` is refused on mainnet before its body
    /// is read.
    fn finish(
        &self,
        carrier: Carrier,
        payload: impl FnOnce(&Params) -> Result<Vec<u8>, Refusal>,
    ) -> Result<ConnectUri, Refusal> {
        let version = self.get("v");
        if version != Some("2") {
            let shown = match version {
                None => "(none)",
                Some(v) if (1..=3).contains(&v.len()) && v.bytes().all(|b| b.is_ascii_digit()) => v,
                Some(_) => "(unreadable)",
            };
            return Err(Refusal::unsupported(format!("URI version {shown}")));
        }
        let network = self
            .get("n")
            .and_then(NetworkSel::from_code)
            .ok_or_else(|| Refusal::invalid("unknown network"))?;
        if carrier == Carrier::DashKey && network == NetworkSel::Mainnet {
            return Err(Refusal::invalid(
                "dash-key request v2 is not accepted on mainnet",
            ));
        }
        let body = payload(self)?;
        if body.len() > MAX_BODY_BYTES {
            return Err(Refusal::invalid("request is too long"));
        }
        let callback = self.get("cb").map(Callback::parse).transpose()?;
        Ok(ConnectUri {
            carrier,
            network,
            request: AppConnectRequest::decode(&body)?,
            callback,
        })
    }
}

/// Form decoding as iOS does it: `+` is a space, each `%XX` a byte, a `%`
/// without two hex digits stands as written, and bytes that are not UTF-8
/// become U+FFFD.
fn form_decode(text: &str) -> String {
    let source = text.as_bytes();
    let mut bytes = Vec::with_capacity(source.len());
    let mut i = 0;
    while i < source.len() {
        let byte = source[i];
        if byte == b'%'
            && i + 2 < source.len()
            && let (Some(high), Some(low)) = (hex_value(source[i + 1]), hex_value(source[i + 2]))
        {
            bytes.push(high << 4 | low);
            i += 3;
        } else {
            bytes.push(if byte == b'+' { b' ' } else { byte });
            i += 1;
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    char::from(byte).to_digit(16).map(|d| d as u8)
}

/// RFC 3986 characters only, every `%` starting a `%XX` triplet.
fn is_strict_uri_text(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.iter().enumerate().all(|(i, &b)| match b {
        b'%' => bytes
            .get(i + 1..i + 3)
            .is_some_and(|hex| hex.iter().all(u8::is_ascii_hexdigit)),
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' => true,
        _ => b"-._~!$&'()*+,;=:@/?#".contains(&b),
    })
}

/// Dot-separated labels of `[a-z0-9-]`, none empty, the last starting with
/// a letter: no IP address in any spelling, no brackets, no
/// percent-encoding and no capitals (a browser lowercases them).
fn is_plain_host_name(host: &str) -> bool {
    host.split('.').all(|label| {
        !label.is_empty()
            && label
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    }) && host
        .rsplit('.')
        .next()
        .and_then(|last| last.bytes().next())
        .is_some_and(|b| b.is_ascii_lowercase())
}

const BASE64URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// RFC 4648 section 5 base64url without padding.
pub fn base64url_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let group = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, b)| acc | u32::from(*b) << (16 - 8 * i));
        for i in 0..=chunk.len() {
            out.push(char::from(BASE64URL[(group >> (18 - 6 * i) & 63) as usize]));
        }
    }
    out
}

/// Canonical base64url, or `None`: refuses `=`, `+`, `/`, anything else
/// outside the alphabet, a length of 1 mod 4, and non-zero trailing bits, so
/// each value has exactly one encoding.
pub fn base64url_decode(text: &str) -> Option<Vec<u8>> {
    if text.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut accumulator = 0u32;
    let mut bits = 0;
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        };
        accumulator = accumulator << 6 | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((accumulator >> bits) as u8);
            accumulator &= (1 << bits) - 1;
        }
    }
    (accumulator == 0).then_some(out)
}

#[cfg(test)]
#[path = "connect_tests.rs"]
mod tests;
