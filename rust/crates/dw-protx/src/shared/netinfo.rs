//! `ExtNetInfo`: the network info of a version-3 provider payload
//! (rc.3 `evo/netinfo.h:194-263,411-435`).
//!
//! ```text
//! u8 version (1) | cs(k) purposes, ascending | per purpose: u8 purpose, cs(n) entries (1..=4)
//! entry: u8 0x01 | ADDRv2 (u8 BIP155 network, cs(len) address) | u16 big-endian port
//!        u8 0x02 | cs(len ≤ 253) domain                         | u16 big-endian port
//! ```
//!
//! An unknown version (0 or above 1) is the version byte alone, as Core
//! writes and reads it. The decoder is stricter than Core's in what it
//! refuses rather than reads: an invalid entry (`0xff`, an unknown network,
//! a wrong address length, an IPv4-, TorV2- or internal-in-IPv6 address, a
//! domain that isn't UTF-8), more than [`MAX_ENTRIES`] entries in a purpose
//! or more than 3 purposes, and purposes out of order or repeated, which
//! Core reads into a different object than the bytes (`Clear()`,
//! `std::map` insertion) and no valid payload holds. Each of these is
//! `bad-protx-payload` here where Core may give a later reason
//! (`-netinfo-bad`, `-netinfo-maxlimit`).
//! Which entries a payload may carry is a rule, not the codec's:
//! [`ExtNetInfo::validate`] is Core's `ExtNetInfo::Validate` with its
//! [`NetInfoStatus`] (MG-04b D5), [`NetInfoStatus::reject`] the reason
//! `CheckNetInfo` gives for it, and [`ExtNetInfo::is_ip_core_p2p_only`] what
//! the wallet supports on top.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use dashcore::Network;
use dashcore::consensus::encode::{self, Decodable, Encodable, VarInt};

use crate::rules::Reject;

/// `ExtNetInfo::CURRENT_VERSION`.
pub const EXT_NET_INFO_VERSION: u8 = 1;
/// `MAX_ENTRIES_EXTNETINFO` (rc.3 `evo/netinfo.h:20`).
pub const MAX_ENTRIES: usize = 4;
/// `DOMAIN_MAX_LEN` (rc.3 `evo/netinfo.h:25`).
pub const DOMAIN_MAX_LEN: usize = 253;

/// `NetInfoPurpose` (rc.3 `evo/netinfo.h:68-76`).
pub const PURPOSE_CORE_P2P: u8 = 0;
pub const PURPOSE_PLATFORM_P2P: u8 = 1;
pub const PURPOSE_PLATFORM_HTTPS: u8 = 2;

const ENTRY_SERVICE: u8 = 0x01;
const ENTRY_DOMAIN: u8 = 0x02;

/// `CNetAddr::BIP155Network` and each network's address size
/// (rc.3 `netaddress.h:94-107,289-296`). TorV2 (3) is no longer valid.
pub const BIP155_IPV4: u8 = 1;
pub const BIP155_IPV6: u8 = 2;
pub const BIP155_TORV3: u8 = 4;
pub const BIP155_I2P: u8 = 5;
pub const BIP155_CJDNS: u8 = 6;

/// `CJDNS_PREFIX` (rc.3 `netaddress.h:91`).
const CJDNS_PREFIX: u8 = 0xfc;
/// `I2P_SAM31_PORT` (rc.3 `netaddress.h:113`): an I2P service's only port.
const I2P_SAM31_PORT: u16 = 0;
/// `PRIVILEGED_PORTS_THRESHOLD` (rc.3 `netbase.h:328`).
const PRIVILEGED_PORTS_THRESHOLD: u16 = 1023;

fn address_size(network: u8) -> Option<usize> {
    match network {
        BIP155_IPV4 => Some(4),
        BIP155_IPV6 | BIP155_CJDNS => Some(16),
        BIP155_TORV3 | BIP155_I2P => Some(32),
        _ => None,
    }
}

/// `IPV4_IN_IPV6_PREFIX`, `TORV2_IN_IPV6_PREFIX`, `INTERNAL_IN_IPV6_PREFIX`
/// (rc.3 `netaddress.h:68-86`): an ADDRv2 IPv6 address with the first two is
/// read as invalid, with the third as an internal name
/// (`UnserializeV2Stream`, `netaddress.h:429-485`); none is a service.
fn is_embedded_ipv6(addr: &[u8]) -> bool {
    addr.starts_with(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff])
        || addr.starts_with(&[0xfd, 0x87, 0xd8, 0x7e, 0xeb, 0x43])
        || addr.starts_with(&[0xfd, 0x6b, 0x88, 0xc0, 0x87, 0x24])
}

/// A `CService` in ADDRv2 form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceAddr {
    pub network: u8,
    pub address: Vec<u8>,
    pub port: u16,
}

impl ServiceAddr {
    /// The ADDRv2 form of an IP service, an fc00::/8 IPv6 address flipped to
    /// CJDNS ([`Self::maybe_flip_to_cjdns`], MG-04b D5); `None` for an IPv6
    /// address with an embedded IPv4, TorV2 or internal prefix.
    pub fn from_socket_addr(addr: SocketAddr) -> Option<Self> {
        let (network, address) = match addr.ip() {
            IpAddr::V4(ip) => (BIP155_IPV4, ip.octets().to_vec()),
            IpAddr::V6(ip) => {
                if is_embedded_ipv6(&ip.octets()) {
                    return None;
                }
                (BIP155_IPV6, ip.octets().to_vec())
            }
        };
        Some(
            Self {
                network,
                address,
                port: addr.port(),
            }
            .maybe_flip_to_cjdns(),
        )
    }

    /// `MaybeFlipIPv6toCJDNS` (rc.3 `netbase.cpp:833-840`), which
    /// `ExtNetInfo::AddEntry` applies to a parsed IP (`evo/netinfo.cpp:543`):
    /// an IPv6 address with the CJDNS prefix 0xfc (`netaddress.h:91,193`)
    /// becomes a CJDNS address. Core flips only when CJDNS is reachable
    /// (`-cjdnsreachable`, off by default, `init.cpp:1856-1862`); unflipped,
    /// such an address is RFC 4193 and unroutable, so on mainnet and testnet
    /// it is refused either way. The wallet always flips and then refuses
    /// CJDNS (D5), so the outcome doesn't depend on the coordinator's node.
    pub fn maybe_flip_to_cjdns(mut self) -> Self {
        self.network = self.effective_network();
        self
    }

    /// The address as IPv4, if it is one.
    fn ipv4(&self) -> Option<Ipv4Addr> {
        if self.network != BIP155_IPV4 {
            return None;
        }
        <[u8; 4]>::try_from(self.address.as_slice())
            .ok()
            .map(Ipv4Addr::from)
    }

    /// The address as a 16-byte IPv6 or CJDNS address, if it is one.
    fn ipv6_bytes(&self) -> Option<[u8; 16]> {
        if !matches!(self.network, BIP155_IPV6 | BIP155_CJDNS) {
            return None;
        }
        self.address.as_slice().try_into().ok()
    }

    /// The network Core would hold this service in once an fc00::/8 IPv6
    /// address is flipped ([`Self::maybe_flip_to_cjdns`]).
    fn effective_network(&self) -> u8 {
        if self.network == BIP155_IPV6 && self.address.first() == Some(&CJDNS_PREFIX) {
            BIP155_CJDNS
        } else {
            self.network
        }
    }

    /// `CNetAddr::IsValid` (rc.3 `netaddress.cpp:426-453`): not the
    /// unspecified IPv6 address, a CJDNS address with the CJDNS prefix, not
    /// RFC 3849 documentation IPv6, not internal, not IPv4 `INADDR_ANY` or
    /// `INADDR_NONE`. An address of the wrong size or with an embedded
    /// prefix is not one Core reads as valid either.
    pub fn is_valid(&self) -> bool {
        if address_size(self.network) != Some(self.address.len()) {
            return false;
        }
        match self.network {
            BIP155_IPV4 => self
                .ipv4()
                .is_some_and(|ip| !ip.is_unspecified() && ip != Ipv4Addr::BROADCAST),
            BIP155_IPV6 => {
                let a = &self.address;
                !a.iter().all(|b| *b == 0)
                    && !is_embedded_ipv6(a)
                    && !a.starts_with(&[0x20, 0x01, 0x0d, 0xb8])
            }
            BIP155_CJDNS => self.address[0] == CJDNS_PREFIX,
            _ => true,
        }
    }

    /// `CNetAddr::IsRoutable` (rc.3 `netaddress.cpp:464-471`) with
    /// `-allowprivatenet` off (its default): valid and not RFC 1918, 2544,
    /// 3927, 4862, 6598, 5737, 4193, 4843, 7343, local or internal. The
    /// IPv6 ranges (`:365-393`, `:400-414`) apply to IPv6 only, so CJDNS,
    /// Tor and I2P addresses are routable when valid.
    pub fn is_routable(&self) -> bool {
        if !self.is_valid() {
            return false;
        }
        match (self.ipv4(), self.network) {
            // RFC 1918, 2544, 3927, 6598, 5737 and local (`:313-341,403-405`).
            (Some(ip), _) => crate::service::is_routable(ip),
            (None, BIP155_IPV6) => {
                let Some(a) = self.ipv6_bytes() else {
                    return false;
                };
                let rfc4862 = a.starts_with(&[0xfe, 0x80, 0, 0, 0, 0, 0, 0]);
                let rfc4193 = a[0] & 0xfe == 0xfc;
                let rfc4843 = a.starts_with(&[0x20, 0x01, 0x00]) && a[3] & 0xf0 == 0x10;
                let rfc7343 = a.starts_with(&[0x20, 0x01, 0x00]) && a[3] & 0xf0 == 0x20;
                let local = a == Ipv6Addr::LOCALHOST.octets();
                !(rfc4862 || rfc4193 || rfc4843 || rfc7343 || local)
            }
            _ => true,
        }
    }

    /// The service as an IP socket address; `None` for Tor, I2P and CJDNS.
    pub fn socket_addr(&self) -> Option<SocketAddr> {
        let ip = match self.network {
            BIP155_IPV4 => IpAddr::V4(self.ipv4()?),
            BIP155_IPV6 => IpAddr::V6(Ipv6Addr::from(self.ipv6_bytes()?)),
            _ => return None,
        };
        Some(SocketAddr::new(ip, self.port))
    }
}

/// `NetInfoEntry`, valid entries only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetInfoEntry {
    Service(ServiceAddr),
    Domain { name: String, port: u16 },
}

/// `ExtNetInfo`: entries per purpose, purposes ascending.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtNetInfo {
    pub version: u8,
    pub entries: Vec<(u8, Vec<NetInfoEntry>)>,
}

impl Default for ExtNetInfo {
    /// The empty network info: `01 00`.
    fn default() -> Self {
        Self {
            version: EXT_NET_INFO_VERSION,
            entries: Vec::new(),
        }
    }
}

impl ExtNetInfo {
    /// Core P2P services only, in the given order: a regular masternode's
    /// network info. `None` if a service has no ADDRv2 IP form or there are
    /// more than [`MAX_ENTRIES`].
    pub fn core_p2p(services: &[SocketAddr]) -> Option<Self> {
        if services.is_empty() {
            return Some(Self::default());
        }
        if services.len() > MAX_ENTRIES {
            return None;
        }
        let entries = services
            .iter()
            .map(|s| ServiceAddr::from_socket_addr(*s).map(NetInfoEntry::Service))
            .collect::<Option<Vec<_>>>()?;
        Some(Self {
            version: EXT_NET_INFO_VERSION,
            entries: vec![(PURPOSE_CORE_P2P, entries)],
        })
    }

    /// The entries of one purpose.
    pub fn purpose(&self, purpose: u8) -> &[NetInfoEntry] {
        self.entries
            .iter()
            .find(|(p, _)| *p == purpose)
            .map(|(_, e)| e.as_slice())
            .unwrap_or(&[])
    }

    pub fn is_empty(&self) -> bool {
        self.entries.iter().all(|(_, e)| e.is_empty())
    }

    /// Every entry, purposes in order (`GetEntries()` with no purpose).
    pub fn all_entries(&self) -> impl Iterator<Item = &NetInfoEntry> {
        self.entries.iter().flat_map(|(_, e)| e)
    }

    fn known_version(&self) -> bool {
        self.version != 0 && self.version <= EXT_NET_INFO_VERSION
    }

    /// Core's `ExtNetInfo::IsEmpty` (rc.3 `evo/netinfo.h:442`): the current
    /// version and no purpose at all. Unlike [`Self::is_empty`], an unknown
    /// version or a purpose with no entries is not empty.
    pub fn is_empty_in_core(&self) -> bool {
        self.version == EXT_NET_INFO_VERSION && self.entries.is_empty()
    }

    /// `ExtNetInfo::HasEntries` (rc.3 `evo/netinfo.cpp:581-586`).
    pub fn has_entries(&self, purpose: u8) -> bool {
        is_valid_purpose(purpose) && !self.purpose(purpose).is_empty()
    }

    /// Whether every entry is an IPv4 or IPv6 Core P2P service, an fc00::/8
    /// IPv6 address counting as CJDNS: what the wallet supports for a
    /// regular shared masternode (MG-04b D5). Anything else is the engine's
    /// `unsupported{feature: "shared_masternodes", field: "service"}`;
    /// Core's rules are [`Self::validate`]'s.
    pub fn is_ip_core_p2p_only(&self) -> bool {
        self.entries.iter().all(|(purpose, entries)| {
            *purpose == PURPOSE_CORE_P2P
                && entries.iter().all(|e| match e {
                    NetInfoEntry::Service(s) => {
                        matches!(s.effective_network(), BIP155_IPV4 | BIP155_IPV6)
                    }
                    NetInfoEntry::Domain { .. } => false,
                })
        })
    }

    /// `ExtNetInfo::Validate` (rc.3 `evo/netinfo.cpp:588-638`) on `network`:
    /// a known version with at least one purpose; no entry twice (address
    /// and port) anywhere; per purpose, a valid purpose with 1 to
    /// [`MAX_ENTRIES`] entries (`netinfo.h:20`) whose addresses are distinct
    /// even across ports (`:609-611`); each entry trivially valid, a service
    /// passing [`validate_service`], a domain only for Platform HTTPS
    /// (`:622-626`) and passing `ValidateDomainPort`. Purposes are read in
    /// ascending order, as Core's `std::map` holds them; out of order or
    /// repeated (which no decoded value has) is `Malformed`.
    pub fn validate(&self, network: Network) -> NetInfoStatus {
        if !self.known_version() || self.entries.is_empty() {
            return NetInfoStatus::Malformed;
        }
        // `HasAddrPortDuplicates` (`:383-393`): `NetInfoEntry::operator==`.
        let all: Vec<_> = self
            .all_entries()
            .map(NetInfoEntry::addr_port_key)
            .collect();
        if has_duplicates(&all) {
            return NetInfoStatus::Duplicate;
        }
        let mut previous = None;
        for (purpose, entries) in &self.entries {
            if !is_valid_purpose(*purpose) || previous.is_some_and(|p| p >= *purpose) {
                return NetInfoStatus::Malformed;
            }
            previous = Some(*purpose);
            if entries.is_empty() {
                return NetInfoStatus::Malformed;
            }
            if entries.len() > MAX_ENTRIES {
                return NetInfoStatus::MaxLimit;
            }
            // `HasAddrDuplicates` (`:401-411`): `ToStringAddr()` equality.
            let addrs: Vec<_> = entries.iter().map(NetInfoEntry::addr_key).collect();
            if has_duplicates(&addrs) {
                return NetInfoStatus::Duplicate;
            }
            for entry in entries {
                if !entry.is_trivially_valid() {
                    return NetInfoStatus::Malformed;
                }
                let status = match entry {
                    NetInfoEntry::Service(s) => validate_service(*purpose, s, network),
                    NetInfoEntry::Domain { .. } if *purpose != PURPOSE_PLATFORM_HTTPS => {
                        NetInfoStatus::BadInput
                    }
                    NetInfoEntry::Domain { name, port } => validate_domain_port(name, *port),
                };
                if status != NetInfoStatus::Success {
                    return status;
                }
            }
        }
        NetInfoStatus::Success
    }
}

/// `IsValidPurpose` (rc.3 `evo/netinfo.h:80-89`).
fn is_valid_purpose(purpose: u8) -> bool {
    purpose <= PURPOSE_PLATFORM_HTTPS
}

fn has_duplicates<T: PartialEq>(items: &[T]) -> bool {
    items
        .iter()
        .enumerate()
        .any(|(i, a)| items[..i].contains(a))
}

/// `NetInfoStatus` (rc.3 `evo/netinfo.h:27-41`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetInfoStatus {
    BadInput,
    Duplicate,
    MaxLimit,
    BadAddress,
    BadPort,
    BadType,
    NotRoutable,
    Malformed,
    Success,
}

impl NetInfoStatus {
    /// `CheckNetInfo`'s reject reason (rc.3 `evo/providertx.cpp:187-210`);
    /// `None` for `Success`.
    pub fn reject(self) -> Option<Reject> {
        Some(match self {
            Self::BadAddress => "bad-protx-netinfo-addr",
            Self::BadPort => "bad-protx-netinfo-port",
            Self::BadType => "bad-protx-netinfo-addr-type",
            Self::NotRoutable => "bad-protx-netinfo-addr-unroutable",
            Self::Malformed => "bad-protx-netinfo-bad",
            Self::BadInput => "bad-protx-netinfo-entry",
            Self::Duplicate => "bad-protx-dup-netinfo-entry",
            Self::MaxLimit => "bad-protx-netinfo-maxlimit",
            Self::Success => return None,
        })
    }
}

/// The ports `IsBadPort` lists above the privileged ones (rc.3
/// `netbase.cpp:806-826`).
const BAD_PORTS: [u16; 21] = [
    1719, 1720, 1723, 2049, 3659, 4045, 5060, 5061, 6000, 6566, 6665, 6666, 6667, 6668, 6669, 6697,
    8332, 8333, 10080, 18332, 18333,
];

/// `IsBadPort` (rc.3 `netbase.cpp:801-831`): the privileged ports and
/// Core's list of ports other protocols use.
pub fn is_bad_port(port: u16) -> bool {
    (port > 0 && port <= PRIVILEGED_PORTS_THRESHOLD) || BAD_PORTS.contains(&port)
}

/// The port rule `ValidateService` and `ValidateDomainPort` share (rc.3
/// `evo/netinfo.cpp:470-474,486-488`): zero, or bad unless it is
/// `IsAllowedPlatformHTTPPort`'s 443 (`:59-66`) where Platform HTTPS allows
/// it.
fn is_refused_port(port: u16, platform_https: bool) -> bool {
    port == 0 || (is_bad_port(port) && !(platform_https && port == 443))
}

/// `ExtNetInfo::ValidateService` (rc.3 `evo/netinfo.cpp:452-478`) on
/// `network`: a valid address of a known network, routable where the
/// network requires it (`RequireRoutableExternalIP`: every network but
/// regtest, `chainparams.cpp:292,495,687,893`), and a port that is the SAM
/// 3.1 port for I2P, else non-zero and not [`is_bad_port`] (443 allowed for
/// Platform HTTPS). Version 3 has no mainnet-9999 rule.
pub fn validate_service(purpose: u8, service: &ServiceAddr, network: Network) -> NetInfoStatus {
    if !service.is_valid() {
        return NetInfoStatus::BadAddress;
    }
    if !matches!(
        service.network,
        BIP155_IPV4 | BIP155_IPV6 | BIP155_TORV3 | BIP155_I2P | BIP155_CJDNS
    ) {
        return NetInfoStatus::BadType;
    }
    if crate::service::requires_routable(network) && !service.is_routable() {
        return NetInfoStatus::NotRoutable;
    }
    let bad = if service.network == BIP155_I2P {
        service.port != I2P_SAM31_PORT
    } else {
        is_refused_port(service.port, purpose == PURPOSE_PLATFORM_HTTPS)
    };
    if bad {
        return NetInfoStatus::BadPort;
    }
    NetInfoStatus::Success
}

/// `TLDS_BAD` and `TLDS_PRIVACY` (rc.3 `evo/netinfo.cpp:30-42`).
const TLDS_REFUSED: [&str; 15] = [
    ".mail",
    ".arpa",
    ".example",
    ".invalid",
    ".localhost",
    ".test",
    ".local",
    ".corp",
    ".home",
    ".internal",
    ".intranet",
    ".lan",
    ".private",
    ".i2p",
    ".onion",
];

/// `DomainPort::Validate() == Success` (rc.3 `evo/netinfo.cpp:90-114,
/// 130-139`): lower case, a non-zero port, 3 to 253 characters of
/// `[a-zA-Z0-9.-]`, no leading or trailing dot, two or more labels of 1 to
/// 63 characters, none starting or ending with `-`.
fn is_valid_domain(name: &str, port: u16) -> bool {
    // `Validate` (`:132-137`), then `ValidateDomain` (`:92-112`).
    if name.is_empty() || name.bytes().any(|b| b.is_ascii_uppercase()) || port == 0 {
        return false;
    }
    if !(3..=DOMAIN_MAX_LEN).contains(&name.len()) {
        return false;
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
    {
        return false;
    }
    if name.starts_with('.') || name.ends_with('.') || !name.contains('.') {
        return false;
    }
    name.split('.')
        .all(|l| (1..=63).contains(&l.len()) && !l.starts_with('-') && !l.ends_with('-'))
}

/// `ExtNetInfo::ValidateDomainPort` (rc.3 `evo/netinfo.cpp:480-498`) for a
/// trivially valid domain: the port as for a Platform HTTPS service, no
/// refused TLD, and an alphabetic last label.
fn validate_domain_port(name: &str, port: u16) -> NetInfoStatus {
    if !is_valid_domain(name, port) {
        return NetInfoStatus::BadInput;
    }
    if is_refused_port(port, true) {
        return NetInfoStatus::BadPort;
    }
    if TLDS_REFUSED.iter().any(|tld| name.ends_with(tld)) {
        return NetInfoStatus::BadInput;
    }
    let last = name.rsplit('.').next().unwrap_or_default();
    if !last.bytes().all(|b| b.is_ascii_alphabetic()) {
        return NetInfoStatus::BadInput;
    }
    NetInfoStatus::Success
}

/// What `NetInfoEntry::ToStringAddr` compares: IPv4 text (which a domain
/// can spell), IPv6 and CJDNS text (the same for the same bytes), other
/// networks' addresses, a domain name, or an invalid entry
/// (`"[invalid entry]"`). Tor and I2P are compared as bytes, not as their
/// `.onion`/`.b32.i2p` text (a TorV3 name needs SHA3-256, which the
/// dependency graph doesn't have). A domain can spell that text, and
/// `HasAddrDuplicates` runs before the domain checks (`netinfo.cpp:609`),
/// so a Platform HTTPS domain equal to a Tor or I2P service of the same
/// purpose is `Duplicate` in Core and `BadInput` (its TLD) here: refused
/// either way, under a different reason.
#[derive(Debug, PartialEq, Eq)]
enum AddrKey {
    Invalid,
    Ipv4([u8; 4]),
    Ipv6([u8; 16]),
    Other(u8, Vec<u8>),
    Domain(String),
}

impl NetInfoEntry {
    /// `NetInfoEntry::IsTriviallyValid` (rc.3 `evo/netinfo.cpp:209-234`):
    /// Core's reader keeps an invalid service or domain as an invalid entry
    /// (`netinfo.h:243-263`), which this is false for.
    pub fn is_trivially_valid(&self) -> bool {
        match self {
            Self::Service(s) => s.is_valid(),
            Self::Domain { name, port } => is_valid_domain(name, *port),
        }
    }

    /// `operator==`'s key (`netinfo.cpp:141-152`): the entry itself, every
    /// invalid entry alike.
    fn addr_port_key(&self) -> Option<&Self> {
        self.is_trivially_valid().then_some(self)
    }

    fn addr_key(&self) -> AddrKey {
        if !self.is_trivially_valid() {
            return AddrKey::Invalid;
        }
        match self {
            Self::Service(s) => match (s.ipv4(), s.ipv6_bytes()) {
                (Some(ip), _) => AddrKey::Ipv4(ip.octets()),
                (_, Some(v6)) => AddrKey::Ipv6(v6),
                _ => AddrKey::Other(s.network, s.address.clone()),
            },
            Self::Domain { name, .. } => match name.parse::<Ipv4Addr>() {
                Ok(ip) if ip.to_string() == *name => AddrKey::Ipv4(ip.octets()),
                _ => AddrKey::Domain(name.clone()),
            },
        }
    }
}

fn write_port<W: std::io::Write + ?Sized>(w: &mut W, port: u16) -> Result<usize, std::io::Error> {
    w.write_all(&port.to_be_bytes())?;
    Ok(2)
}

fn read_port<R: std::io::Read + ?Sized>(r: &mut R) -> Result<u16, encode::Error> {
    let mut b = [0u8; 2];
    r.read_exact(&mut b).map_err(encode::Error::Io)?;
    Ok(u16::from_be_bytes(b))
}

fn read_len<R: std::io::Read + ?Sized>(r: &mut R, max: usize) -> Result<usize, encode::Error> {
    let n = VarInt::consensus_decode(r)?.0;
    usize::try_from(n)
        .ok()
        .filter(|n| *n <= max)
        .ok_or(encode::Error::ParseFailed("net info length out of range"))
}

impl Encodable for NetInfoEntry {
    fn consensus_encode<W: std::io::Write + ?Sized>(
        &self,
        w: &mut W,
    ) -> Result<usize, std::io::Error> {
        match self {
            Self::Service(s) => {
                let mut len = ENTRY_SERVICE.consensus_encode(w)?;
                len += s.network.consensus_encode(w)?;
                len += VarInt(s.address.len() as u64).consensus_encode(w)?;
                w.write_all(&s.address)?;
                len += s.address.len();
                Ok(len + write_port(w, s.port)?)
            }
            Self::Domain { name, port } => {
                let mut len = ENTRY_DOMAIN.consensus_encode(w)?;
                len += VarInt(name.len() as u64).consensus_encode(w)?;
                w.write_all(name.as_bytes())?;
                len += name.len();
                Ok(len + write_port(w, *port)?)
            }
        }
    }
}

impl Decodable for NetInfoEntry {
    fn consensus_decode<R: std::io::Read + ?Sized>(r: &mut R) -> Result<Self, encode::Error> {
        match u8::consensus_decode(r)? {
            ENTRY_SERVICE => {
                let network = u8::consensus_decode(r)?;
                // `CNetAddr::MAX_ADDRV2_SIZE` (rc.3 `netaddress.h:308`).
                let size = read_len(r, 512)?;
                if address_size(network) != Some(size) {
                    return Err(encode::Error::ParseFailed(
                        "unknown or malformed ADDRv2 address",
                    ));
                }
                let mut address = vec![0u8; size];
                r.read_exact(&mut address).map_err(encode::Error::Io)?;
                if network == BIP155_IPV6 && is_embedded_ipv6(&address) {
                    return Err(encode::Error::ParseFailed(
                        "embedded address in ADDRv2 IPv6",
                    ));
                }
                Ok(Self::Service(ServiceAddr {
                    network,
                    address,
                    port: read_port(r)?,
                }))
            }
            ENTRY_DOMAIN => {
                let size = read_len(r, DOMAIN_MAX_LEN)?;
                let mut name = vec![0u8; size];
                r.read_exact(&mut name).map_err(encode::Error::Io)?;
                let name = String::from_utf8(name)
                    .map_err(|_| encode::Error::ParseFailed("domain is not text"))?;
                Ok(Self::Domain {
                    name,
                    port: read_port(r)?,
                })
            }
            _ => Err(encode::Error::ParseFailed("invalid net info entry")),
        }
    }
}

impl Encodable for ExtNetInfo {
    fn consensus_encode<W: std::io::Write + ?Sized>(
        &self,
        w: &mut W,
    ) -> Result<usize, std::io::Error> {
        let mut len = self.version.consensus_encode(w)?;
        if !self.known_version() {
            return Ok(len);
        }
        len += VarInt(self.entries.len() as u64).consensus_encode(w)?;
        for (purpose, entries) in &self.entries {
            len += purpose.consensus_encode(w)?;
            len += VarInt(entries.len() as u64).consensus_encode(w)?;
            for entry in entries {
                len += entry.consensus_encode(w)?;
            }
        }
        Ok(len)
    }
}

impl Decodable for ExtNetInfo {
    fn consensus_decode<R: std::io::Read + ?Sized>(r: &mut R) -> Result<Self, encode::Error> {
        let version = u8::consensus_decode(r)?;
        let mut info = Self {
            version,
            entries: Vec::new(),
        };
        if !info.known_version() {
            return Ok(info);
        }
        // Three purposes exist; more can't be valid.
        let purposes = read_len(r, 3)?;
        for _ in 0..purposes {
            let purpose = u8::consensus_decode(r)?;
            if info.entries.last().is_some_and(|(p, _)| *p >= purpose) {
                return Err(encode::Error::ParseFailed("net info purposes out of order"));
            }
            let count = read_len(r, MAX_ENTRIES)?;
            let entries = (0..count)
                .map(|_| NetInfoEntry::consensus_decode(r))
                .collect::<Result<Vec<_>, _>>()?;
            info.entries.push((purpose, entries));
        }
        Ok(info)
    }
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;
    use dashcore::consensus::{deserialize, serialize};

    #[test]
    fn test_QT_126_empty_net_info_is_version_and_zero_purposes() {
        assert_eq!(serialize(&ExtNetInfo::default()), [1, 0]);
        assert_eq!(ExtNetInfo::core_p2p(&[]).unwrap(), ExtNetInfo::default());
    }

    #[test]
    fn test_QT_126_ipv4_service_is_addrv2_with_a_big_endian_port() {
        let info = ExtNetInfo::core_p2p(&["1.2.3.4:19999".parse().unwrap()]).unwrap();
        let bytes = serialize(&info);
        // version, 1 purpose, CORE_P2P, 1 entry, Service, IPv4, len 4, addr, port 0x4e1f.
        assert_eq!(bytes, [1, 1, 0, 1, 1, 1, 4, 1, 2, 3, 4, 0x4e, 0x1f]);
        let back: ExtNetInfo = deserialize(&bytes).unwrap();
        assert_eq!(back, info);
        let ServiceAddr { port, .. } = match &back.purpose(PURPOSE_CORE_P2P)[0] {
            NetInfoEntry::Service(s) => s.clone(),
            other => panic!("{other:?}"),
        };
        assert_eq!(port, 19999);
    }

    #[test]
    fn test_QT_126_ipv6_service_round_trips_and_mapped_ipv4_is_refused() {
        let addr: SocketAddr = "[2001:db8::1]:9999".parse().unwrap();
        let info = ExtNetInfo::core_p2p(&[addr]).unwrap();
        let back: ExtNetInfo = deserialize(&serialize(&info)).unwrap();
        let NetInfoEntry::Service(s) = &back.purpose(PURPOSE_CORE_P2P)[0] else {
            panic!()
        };
        assert_eq!(s.socket_addr(), Some(addr));
        assert!(ExtNetInfo::core_p2p(&["[::ffff:1.2.3.4]:9999".parse().unwrap()]).is_none());
    }

    #[test]
    fn test_QT_126_domain_entry_round_trips() {
        let info = ExtNetInfo {
            version: 1,
            entries: vec![(
                PURPOSE_PLATFORM_HTTPS,
                vec![NetInfoEntry::Domain {
                    name: "node.example".into(),
                    port: 443,
                }],
            )],
        };
        let bytes = serialize(&info);
        assert_eq!(&bytes[..5], &[1, 1, 2, 1, 2]);
        assert_eq!(&bytes[bytes.len() - 2..], &[1, 0xbb]);
        assert_eq!(deserialize::<ExtNetInfo>(&bytes).unwrap(), info);
    }

    #[test]
    fn test_QT_126_unknown_version_is_the_version_byte_alone() {
        let info = ExtNetInfo {
            version: 2,
            entries: vec![],
        };
        assert_eq!(serialize(&info), [2]);
        assert_eq!(deserialize::<ExtNetInfo>(&[2]).unwrap(), info);
    }

    #[test]
    fn test_QT_126_decoder_refuses_entries_core_would_not_keep() {
        for bad in [
            // An invalid entry type.
            &[1, 1, 0, 1, 0xff][..],
            // IPv4 with a 16-byte address.
            &[
                1, 1, 0, 1, 1, 1, 16, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            ][..],
            // An unknown network.
            &[1, 1, 0, 1, 1, 9, 4, 1, 2, 3, 4, 0, 1][..],
            // Five entries.
            &[1, 1, 0, 5][..],
            // Purposes repeated.
            &[1, 2, 0, 0, 0, 0][..],
        ] {
            assert!(deserialize::<ExtNetInfo>(bad).is_err(), "{bad:?}");
        }
    }

    // ---- D5: Core's v3 rules (ExtNetInfo::Validate) ----

    fn info(services: &[&str]) -> ExtNetInfo {
        let addrs: Vec<SocketAddr> = services.iter().map(|s| s.parse().unwrap()).collect();
        ExtNetInfo::core_p2p(&addrs).unwrap()
    }

    /// The one entry of `service`'s Core P2P net info.
    fn service(text: &str) -> NetInfoEntry {
        info(&[text]).entries[0].1[0].clone()
    }

    fn one(purpose: u8, entries: Vec<NetInfoEntry>) -> ExtNetInfo {
        ExtNetInfo {
            version: 1,
            entries: vec![(purpose, entries)],
        }
    }

    const ALL: [Network; 4] = [
        Network::Mainnet,
        Network::Testnet,
        Network::Devnet,
        Network::Regtest,
    ];

    #[test]
    fn test_QT_126_v3_net_info_has_no_mainnet_port_rule() {
        for network in ALL {
            for port in [9999, 19999, 26656] {
                let i = info(&[&format!("8.8.8.8:{port}")]);
                assert_eq!(i.validate(network), NetInfoStatus::Success, "{network:?}");
            }
        }
        let v6 = info(&["[2a01:4f8::1]:9999", "8.8.4.4:9999"]);
        assert_eq!(v6.validate(Network::Mainnet), NetInfoStatus::Success);
        assert!(v6.is_ip_core_p2p_only());
    }

    #[test]
    fn test_QT_126_v3_net_info_ports_zero_and_bad_are_refused() {
        for port in [0, 1, 80, 443, 1023, 1719, 6667, 8333, 10080, 18333] {
            let i = info(&[&format!("8.8.8.8:{port}")]);
            assert_eq!(
                i.validate(Network::Mainnet),
                NetInfoStatus::BadPort,
                "{port}"
            );
        }
        for port in [1024, 1718, 8334, 18334, 65535] {
            let i = info(&[&format!("8.8.8.8:{port}")]);
            assert_eq!(
                i.validate(Network::Mainnet),
                NetInfoStatus::Success,
                "{port}"
            );
        }
        // 443 is allowed for Platform HTTPS only.
        let https = one(PURPOSE_PLATFORM_HTTPS, vec![service("8.8.8.8:443")]);
        assert_eq!(https.validate(Network::Mainnet), NetInfoStatus::Success);
        assert_eq!(
            NetInfoStatus::BadPort.reject(),
            Some("bad-protx-netinfo-port")
        );
    }

    #[test]
    fn test_QT_126_v3_net_info_addresses_are_distinct_across_ports() {
        let same = info(&["8.8.8.8:19999", "8.8.8.8:20000"]);
        assert_eq!(same.validate(Network::Testnet), NetInfoStatus::Duplicate);
        let twice = info(&["8.8.8.8:19999", "8.8.8.8:19999"]);
        assert_eq!(twice.validate(Network::Testnet), NetInfoStatus::Duplicate);
        assert_eq!(
            NetInfoStatus::Duplicate.reject(),
            Some("bad-protx-dup-netinfo-entry")
        );
        // The same address may serve two purposes on different ports, not on the same one.
        let mut two = ExtNetInfo {
            version: 1,
            entries: vec![
                (PURPOSE_CORE_P2P, vec![service("8.8.8.8:19999")]),
                (PURPOSE_PLATFORM_P2P, vec![service("8.8.8.8:22000")]),
            ],
        };
        assert_eq!(two.validate(Network::Testnet), NetInfoStatus::Success);
        assert!(!two.is_ip_core_p2p_only());
        two.entries[1].1[0] = service("8.8.8.8:19999");
        assert_eq!(two.validate(Network::Testnet), NetInfoStatus::Duplicate);
        // A domain spelling an IPv4 address is that address (`ToStringAddr`).
        let https = one(
            PURPOSE_PLATFORM_HTTPS,
            vec![
                service("8.8.8.8:443"),
                NetInfoEntry::Domain {
                    name: "8.8.8.8".into(),
                    port: 8443,
                },
            ],
        );
        assert_eq!(https.validate(Network::Mainnet), NetInfoStatus::Duplicate);
    }

    #[test]
    fn test_QT_126_v3_net_info_holds_at_most_four_per_purpose() {
        let four = info(&[
            "8.8.8.1:1024",
            "8.8.8.2:1024",
            "8.8.8.3:1024",
            "8.8.8.4:1024",
        ]);
        assert_eq!(four.validate(Network::Mainnet), NetInfoStatus::Success);
        let mut five = four.clone();
        five.entries[0].1.push(service("8.8.8.5:1024"));
        assert_eq!(five.validate(Network::Mainnet), NetInfoStatus::MaxLimit);
        assert_eq!(
            NetInfoStatus::MaxLimit.reject(),
            Some("bad-protx-netinfo-maxlimit")
        );
        // The codec can't write a fifth entry the decoder would read.
        assert!(deserialize::<ExtNetInfo>(&serialize(&five)).is_err());
    }

    #[test]
    fn test_QT_126_v3_net_info_routable_everywhere_but_regtest() {
        for unroutable in [
            "10.1.2.3:19999",
            "172.16.0.1:19999",
            "192.168.1.1:19999",
            "127.0.0.1:19999",
            "100.64.0.1:19999",
            "169.254.1.1:19999",
            "198.18.0.1:19999",
            "192.0.2.1:19999",
            "198.51.100.1:19999",
            "203.0.113.1:19999",
            "[::1]:19999",
            "[fe80::1]:19999",
            "[2001:10::1]:19999",
            "[2001:20::1]:19999",
        ] {
            let i = info(&[unroutable]);
            for network in [Network::Mainnet, Network::Testnet, Network::Devnet] {
                assert_eq!(
                    i.validate(network),
                    NetInfoStatus::NotRoutable,
                    "{unroutable}"
                );
            }
            assert_eq!(
                i.validate(Network::Regtest),
                NetInfoStatus::Success,
                "{unroutable}"
            );
        }
        // RFC 4862 is fe80::/64 only (`netaddress.cpp:365-369`).
        assert_eq!(
            info(&["[fe80:1::1]:19999"]).validate(Network::Mainnet),
            NetInfoStatus::Success
        );
        // fd00::/8 is RFC 4193 too, and not CJDNS.
        let ula = info(&["[fd00::1]:19999"]);
        assert_eq!(ula.validate(Network::Testnet), NetInfoStatus::NotRoutable);
        assert!(ula.is_ip_core_p2p_only());
        // Invalid everywhere: unspecified, broadcast, RFC 3849 documentation
        // IPv6. Core's reader keeps such a service as an invalid entry, so
        // `Validate` finds it `Malformed` before `ValidateService` would
        // say `BadAddress`.
        for invalid in [
            "0.0.0.0:19999",
            "255.255.255.255:19999",
            "[2001:db8::1]:19999",
        ] {
            let i = info(&[invalid]);
            assert_eq!(
                i.validate(Network::Regtest),
                NetInfoStatus::Malformed,
                "{invalid}"
            );
            let NetInfoEntry::Service(s) = &i.purpose(PURPOSE_CORE_P2P)[0] else {
                panic!()
            };
            assert!(!s.is_routable());
            assert_eq!(
                validate_service(PURPOSE_CORE_P2P, s, Network::Regtest),
                NetInfoStatus::BadAddress
            );
        }
        assert_eq!(
            NetInfoStatus::NotRoutable.reject(),
            Some("bad-protx-netinfo-addr-unroutable")
        );
    }

    #[test]
    fn test_QT_126_fc00_ipv6_flips_to_cjdns_and_the_wallet_refuses_it() {
        let i = info(&["[fc00::1]:19999"]);
        let NetInfoEntry::Service(s) = &i.purpose(PURPOSE_CORE_P2P)[0] else {
            panic!()
        };
        assert_eq!(s.network, BIP155_CJDNS);
        assert_eq!(s.socket_addr(), None);
        // A valid, routable CJDNS address for Core; not a service the wallet supports.
        assert_eq!(i.validate(Network::Mainnet), NetInfoStatus::Success);
        assert!(!i.is_ip_core_p2p_only());
        // dash-qt without -cjdnsreachable writes it as IPv6: unroutable
        // (RFC 4193) outside regtest, and still CJDNS to the wallet.
        let mut unflipped = i.clone();
        let NetInfoEntry::Service(s) = &mut unflipped.entries[0].1[0] else {
            panic!()
        };
        s.network = BIP155_IPV6;
        assert_eq!(
            unflipped.validate(Network::Testnet),
            NetInfoStatus::NotRoutable
        );
        assert_eq!(unflipped.validate(Network::Regtest), NetInfoStatus::Success);
        assert!(!unflipped.is_ip_core_p2p_only());
    }

    #[test]
    fn test_QT_126_v3_net_info_domains_only_for_platform_https() {
        let domain = |name: &str, port: u16| NetInfoEntry::Domain {
            name: name.into(),
            port,
        };
        let ok = domain("node.dash.org", 443);
        assert_eq!(
            one(PURPOSE_PLATFORM_HTTPS, vec![ok.clone()]).validate(Network::Mainnet),
            NetInfoStatus::Success
        );
        for purpose in [PURPOSE_CORE_P2P, PURPOSE_PLATFORM_P2P] {
            assert_eq!(
                one(purpose, vec![ok.clone()]).validate(Network::Mainnet),
                NetInfoStatus::BadInput
            );
        }
        for (bad, status) in [
            (domain("node.example", 443), NetInfoStatus::BadInput),
            (domain("node.onion", 443), NetInfoStatus::BadInput),
            (domain("node.d4sh", 443), NetInfoStatus::BadInput),
            (domain("node.dash.org", 80), NetInfoStatus::BadPort),
            (domain("Node.dash.org", 443), NetInfoStatus::Malformed),
            (domain("dash", 443), NetInfoStatus::Malformed),
            (domain("-a.dash.org", 443), NetInfoStatus::Malformed),
            (domain("node.dash.org", 0), NetInfoStatus::Malformed),
        ] {
            assert_eq!(
                one(PURPOSE_PLATFORM_HTTPS, vec![bad.clone()]).validate(Network::Mainnet),
                status,
                "{bad:?}"
            );
        }
        assert_eq!(
            NetInfoStatus::BadInput.reject(),
            Some("bad-protx-netinfo-entry")
        );
    }

    #[test]
    fn test_QT_126_v3_net_info_structure_rules() {
        // No purpose, an unknown version, an empty purpose, an unknown purpose.
        assert_eq!(
            ExtNetInfo::default().validate(Network::Regtest),
            NetInfoStatus::Malformed
        );
        let mut v = info(&["8.8.8.8:19999"]);
        v.version = 2;
        assert_eq!(v.validate(Network::Regtest), NetInfoStatus::Malformed);
        assert!(!v.is_empty_in_core());
        let empty_purpose = one(PURPOSE_CORE_P2P, vec![]);
        assert_eq!(
            empty_purpose.validate(Network::Regtest),
            NetInfoStatus::Malformed
        );
        assert!(!empty_purpose.is_empty_in_core());
        assert!(!empty_purpose.has_entries(PURPOSE_CORE_P2P));
        let mut unknown = info(&["8.8.8.8:19999"]);
        unknown.entries[0].0 = 3;
        assert_eq!(unknown.validate(Network::Regtest), NetInfoStatus::Malformed);
        assert!(ExtNetInfo::default().is_empty_in_core());
        assert_eq!(
            NetInfoStatus::Malformed.reject(),
            Some("bad-protx-netinfo-bad")
        );
        assert_eq!(NetInfoStatus::Success.reject(), None);
        // I2P takes port 0 only.
        let i2p = |port| {
            one(
                PURPOSE_CORE_P2P,
                vec![NetInfoEntry::Service(ServiceAddr {
                    network: BIP155_I2P,
                    address: vec![7; 32],
                    port,
                })],
            )
        };
        assert_eq!(i2p(0).validate(Network::Mainnet), NetInfoStatus::Success);
        assert_eq!(
            i2p(19999).validate(Network::Mainnet),
            NetInfoStatus::BadPort
        );
    }
}
