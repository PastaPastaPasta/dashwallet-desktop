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
//! a wrong address length, an IPv4- or TorV2-in-IPv6 address), and purposes
//! out of order or repeated, which Core reads into a different object than
//! the bytes (`Clear()`, `std::map` insertion) and no valid payload holds.
//! Which entries a payload may carry is a rule, not the codec's.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use dashcore::consensus::encode::{self, Decodable, Encodable, VarInt};

/// `ExtNetInfo::CURRENT_VERSION`.
pub const EXT_NET_INFO_VERSION: u8 = 1;
/// `MAX_ENTRIES_EXTNETINFO` (rc.3 `evo/netinfo.h:20`).
pub const MAX_ENTRIES: usize = 4;
/// `DOMAIN_MAX_LEN` (rc.3 `evo/netinfo.h:25`).
pub const DOMAIN_MAX_LEN: usize = 253;

/// `NetInfoPurpose` (rc.3 `evo/netinfo.h:66-76`).
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
    /// The ADDRv2 form of an IP service; `None` for an IPv6 address with an
    /// embedded IPv4, TorV2 or internal prefix.
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
        Some(Self {
            network,
            address,
            port: addr.port(),
        })
    }

    /// The service as an IP socket address; `None` for Tor, I2P and CJDNS.
    pub fn socket_addr(&self) -> Option<SocketAddr> {
        let ip = match self.network {
            BIP155_IPV4 => IpAddr::V4(Ipv4Addr::from(
                <[u8; 4]>::try_from(self.address.as_slice()).ok()?,
            )),
            BIP155_IPV6 => IpAddr::V6(Ipv6Addr::from(
                <[u8; 16]>::try_from(self.address.as_slice()).ok()?,
            )),
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

    fn known_version(&self) -> bool {
        self.version != 0 && self.version <= EXT_NET_INFO_VERSION
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
}
