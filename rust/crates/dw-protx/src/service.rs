//! Service addresses and Platform ports of provider transactions, checked
//! with Dash Core's rules for version-2 payloads (one Core P2P address):
//!
//! - `MnNetInfo::AddEntry` / `ValidateService` (`src/evo/netinfo.cpp:286-330`):
//!   IPv4 only, `host[:port]` with the network's default port, no DNS
//!   lookup, routable unless the network allows local addresses (regtest,
//!   `fRequireRoutableExternalIP = false`, `src/chainparams.cpp:857`), the
//!   mainnet port 9999 on mainnet and never elsewhere.
//! - `CheckProviderNetworkFields` (`src/evo/providertx.cpp:212-263`): the
//!   EvoNode Platform ports — the mainnet defaults on mainnet, never 9999,
//!   and all three ports different.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4};

use dashcore::Network;

use crate::params::default_ports;

/// The mainnet Core P2P port (`MainParams().GetDefaultPort()`).
pub const MAINNET_CORE_PORT: u16 = 9_999;

/// The address a payload carries when it has no service (`CService()`:
/// sixteen zero bytes, port 0). Such a masternode registers PoSe-banned until
/// an Update Service (`src/evo/specialtxman.cpp`, "It's allowed to set addr
/// to 0").
pub const NO_SERVICE: SocketAddr = SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0);

/// Why a service address or port set is refused (`NetInfoStatus` and the
/// `bad-protx-platform-*` reasons).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ServiceError {
    #[error("{0:?} is not an IPv4 address with an optional port")]
    BadInput(String),
    #[error("{0} is not a valid address")]
    BadAddress(String),
    #[error("{0} is not routable")]
    NotRoutable(String),
    #[error("{0}: mainnet masternodes must use port 9999, other networks must not")]
    BadPort(String),
    #[error("a version-2 payload carries one Core P2P address, {0} were given")]
    TooMany(usize),
    #[error("platform {0} port {1} is not allowed")]
    BadPlatformPort(&'static str, u16),
    #[error("the Core, Platform P2P and Platform HTTPS ports must differ")]
    DuplicatePorts,
}

/// Whether the network requires routable masternode addresses
/// (`fRequireRoutableExternalIP`): every network but regtest.
pub fn requires_routable(network: Network) -> bool {
    network != Network::Regtest
}

/// Parses one Core P2P service as dash-qt's wizard and `protx register`
/// do: `a.b.c.d` or `a.b.c.d:port`, the network's default port when none is
/// given.
pub fn parse_service(text: &str, network: Network) -> Result<SocketAddr, ServiceError> {
    let text = text.trim();
    let (host, port) = match text.rsplit_once(':') {
        Some((host, port)) => {
            let port: u16 = port
                .parse()
                .map_err(|_| ServiceError::BadInput(text.to_string()))?;
            (host, port)
        }
        None => (text, default_ports(network).core_p2p),
    };
    // SAFE_CHARS_IPV4 (Core `MatchCharsFilter`): digits and dots only.
    if host.is_empty() || !host.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return Err(ServiceError::BadInput(text.to_string()));
    }
    let ip: Ipv4Addr = host
        .parse()
        .map_err(|_| ServiceError::BadInput(text.to_string()))?;
    let addr = SocketAddrV4::new(ip, port);
    validate_service(addr, network)?;
    Ok(SocketAddr::V4(addr))
}

/// `MnNetInfo::ValidateService` for an IPv4 service.
pub fn validate_service(addr: SocketAddrV4, network: Network) -> Result<(), ServiceError> {
    let ip = *addr.ip();
    if !is_valid(ip) {
        return Err(ServiceError::BadAddress(addr.to_string()));
    }
    if requires_routable(network) && !is_routable(ip) {
        return Err(ServiceError::NotRoutable(addr.to_string()));
    }
    if (network == Network::Mainnet) != (addr.port() == MAINNET_CORE_PORT) {
        return Err(ServiceError::BadPort(addr.to_string()));
    }
    Ok(())
}

/// The payload address of a service list: none = [`NO_SERVICE`], one = that
/// address, more = refused (version-2 payloads hold one entry).
pub fn single_service(list: &[String], network: Network) -> Result<SocketAddr, ServiceError> {
    match list {
        [] => Ok(NO_SERVICE),
        [one] => parse_service(one, network),
        more => Err(ServiceError::TooMany(more.len())),
    }
}

/// Whether `addr` is the empty service.
pub fn is_no_service(addr: &SocketAddr) -> bool {
    addr.port() == 0 && addr.ip().is_unspecified()
}

/// The `ip:port` text of a payload address, `None` for the empty service.
pub fn service_text(addr: &SocketAddr) -> Option<String> {
    if is_no_service(addr) {
        return None;
    }
    Some(match addr.ip() {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => SocketAddr::new(IpAddr::V4(v4), addr.port()).to_string(),
            None => addr.to_string(),
        },
        IpAddr::V4(_) => addr.to_string(),
    })
}

/// Checks the version-2 EvoNode Platform ports against the Core port
/// (`CheckProviderNetworkFields`).
pub fn check_platform_ports(
    network: Network,
    core_port: u16,
    p2p: u16,
    https: u16,
) -> Result<(), ServiceError> {
    if network == Network::Mainnet {
        let defaults = default_ports(Network::Mainnet);
        if p2p != defaults.platform_p2p {
            return Err(ServiceError::BadPlatformPort("P2P", p2p));
        }
        if https != defaults.platform_https {
            return Err(ServiceError::BadPlatformPort("HTTPS", https));
        }
    }
    if p2p == MAINNET_CORE_PORT {
        return Err(ServiceError::BadPlatformPort("P2P", p2p));
    }
    if https == MAINNET_CORE_PORT {
        return Err(ServiceError::BadPlatformPort("HTTPS", https));
    }
    if p2p == https || p2p == core_port || https == core_port {
        return Err(ServiceError::DuplicatePorts);
    }
    Ok(())
}

/// The port of a `host:port` Platform address (only the port of the first
/// entry counts in a version-2 payload); a bare number is taken as the port.
pub fn platform_port(text: &str) -> Option<u16> {
    let text = text.trim();
    match text.rsplit_once(':') {
        Some((_, port)) => port.parse().ok(),
        None => text.parse().ok(),
    }
}

/// `CNetAddr::IsValid` for IPv4: not unspecified, not the broadcast /
/// `INADDR_NONE` address.
fn is_valid(ip: Ipv4Addr) -> bool {
    !ip.is_unspecified() && ip != Ipv4Addr::BROADCAST
}

/// `CNetAddr::IsRoutable` for IPv4 (`src/netaddress.cpp`): not RFC1918,
/// RFC2544, RFC3927, RFC6598, RFC5737, local (`0/8`, `127/8`).
pub fn is_routable(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    let rfc1918 =
        o[0] == 10 || (o[0] == 192 && o[1] == 168) || (o[0] == 172 && (16..=31).contains(&o[1]));
    let rfc2544 = o[0] == 198 && (o[1] == 18 || o[1] == 19);
    let rfc3927 = o[0] == 169 && o[1] == 254;
    let rfc6598 = o[0] == 100 && (64..=127).contains(&o[1]);
    let rfc5737 = (o[0] == 192 && o[1] == 0 && o[2] == 2)
        || (o[0] == 198 && o[1] == 51 && o[2] == 100)
        || (o[0] == 203 && o[1] == 0 && o[2] == 113);
    let local = o[0] == 0 || o[0] == 127;
    is_valid(ip) && !(rfc1918 || rfc2544 || rfc3927 || rfc6598 || rfc5737 || local)
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;

    #[test]
    fn test_QT_123_service_takes_the_default_port_and_core_port_rules() {
        assert_eq!(
            parse_service("1.2.3.4", Network::Mainnet).unwrap(),
            "1.2.3.4:9999".parse().unwrap()
        );
        assert_eq!(
            parse_service("1.2.3.4", Network::Testnet).unwrap(),
            "1.2.3.4:19999".parse().unwrap()
        );
        assert!(matches!(
            parse_service("1.2.3.4:19999", Network::Mainnet),
            Err(ServiceError::BadPort(_))
        ));
        assert!(matches!(
            parse_service("1.2.3.4:9999", Network::Testnet),
            Err(ServiceError::BadPort(_))
        ));
    }

    #[test]
    fn test_QT_123_service_needs_routable_ipv4_except_on_regtest() {
        assert!(matches!(
            parse_service("10.0.0.1:19999", Network::Testnet),
            Err(ServiceError::NotRoutable(_))
        ));
        assert!(parse_service("127.0.0.1:19899", Network::Regtest).is_ok());
        for bad in [
            "host.example:9999",
            "[::1]:9999",
            "1.2.3:x",
            "",
            "1.2.3.4:70000",
        ] {
            assert!(
                matches!(
                    parse_service(bad, Network::Mainnet),
                    Err(ServiceError::BadInput(_))
                ),
                "{bad}"
            );
        }
        assert!(matches!(
            parse_service("0.0.0.0:9999", Network::Mainnet),
            Err(ServiceError::BadAddress(_))
        ));
    }

    #[test]
    fn test_QT_123_service_list_holds_at_most_one_entry() {
        assert_eq!(single_service(&[], Network::Testnet).unwrap(), NO_SERVICE);
        assert!(is_no_service(&NO_SERVICE));
        assert_eq!(service_text(&NO_SERVICE), None);
        let two = ["1.2.3.4".to_string(), "1.2.3.5".to_string()];
        assert_eq!(
            single_service(&two, Network::Testnet),
            Err(ServiceError::TooMany(2))
        );
        let mapped: SocketAddr = "[::ffff:1.2.3.4]:19999".parse().unwrap();
        assert_eq!(service_text(&mapped).as_deref(), Some("1.2.3.4:19999"));
    }

    #[test]
    fn test_QT_123_platform_ports_follow_core() {
        check_platform_ports(Network::Mainnet, 9999, 26656, 443).unwrap();
        assert!(check_platform_ports(Network::Mainnet, 9999, 26657, 443).is_err());
        check_platform_ports(Network::Testnet, 19999, 22000, 22001).unwrap();
        assert_eq!(
            check_platform_ports(Network::Testnet, 19999, 22000, 22000),
            Err(ServiceError::DuplicatePorts)
        );
        assert!(check_platform_ports(Network::Regtest, 19899, 9999, 22201).is_err());
        assert_eq!(platform_port("1.2.3.4:22000"), Some(22000));
        assert_eq!(platform_port("22001"), Some(22001));
    }
}
