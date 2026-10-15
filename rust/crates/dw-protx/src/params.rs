//! Masternode constants (Dash Core `src/chainparams.cpp`,
//! `src/evo/providertx.h`, `src/qt/sharedmnwidgets.h`; research 02 §10).

use dashcore::Network;

/// 1 DASH in duffs.
const COIN: u64 = 100_000_000;

/// Collateral of a regular masternode.
pub const MASTERNODE_COLLATERAL: u64 = 1_000 * COIN;
/// Collateral of an EvoNode.
pub const EVONODE_COLLATERAL: u64 = 4_000 * COIN;
/// Shares of a shared masternode (`CProRegTx::MIN_SHARES`/`MAX_SHARES`).
pub const MIN_SHARES: u32 = 2;
pub const MAX_SHARES: u32 = 8;
/// Smallest share (`MIN_AMOUNT`, `evo/providertx.h`).
pub const MIN_SHARE_AMOUNT: u64 = 100 * COIN;
/// Longest early-exit period (`CProRegTx::MAX_EARLY_PERIOD_BLOCKS`).
pub const MAX_EARLY_PERIOD_BLOCKS: u32 = 420_480;
/// Largest shared-session message dash-qt reads (`MAX_ENVELOPE_FILE_BYTES`).
pub const MAX_ENVELOPE_BYTES: usize = 2 * 1024 * 1024;
/// Highest operator reward, in hundredths of a percent (100.00 %).
pub const MAX_OPERATOR_REWARD: u16 = 10_000;

/// Default ports of a network (`nDefaultPort`, `nDefaultPlatformP2PPort`,
/// `nDefaultPlatformHTTPPort`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DefaultPorts {
    pub core_p2p: u16,
    pub platform_p2p: u16,
    pub platform_https: u16,
}

pub fn default_ports(network: Network) -> DefaultPorts {
    let (core_p2p, platform_p2p, platform_https) = match network {
        Network::Mainnet => (9_999, 26_656, 443),
        Network::Testnet => (19_999, 22_000, 22_001),
        Network::Devnet => (19_799, 22_100, 22_101),
        Network::Regtest => (19_899, 22_200, 22_201),
    };
    DefaultPorts {
        core_p2p,
        platform_p2p,
        platform_https,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_follow_chainparams() {
        assert_eq!(default_ports(Network::Mainnet).core_p2p, 9_999);
        assert_eq!(default_ports(Network::Testnet).platform_https, 22_001);
        // Eight minimum shares fit in one regular collateral.
        assert!(MIN_SHARE_AMOUNT * u64::from(MAX_SHARES) <= MASTERNODE_COLLATERAL);
    }
}
