//! Governance consensus parameters (Dash Core `src/chainparams.cpp`,
//! `src/governance/common.h`; research 02 §11.3–11.4).

use dashcore::Network;

/// 1 DASH in duffs.
const COIN: u64 = 100_000_000;

/// The governance parameters of one network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GovernanceParams {
    /// First superblock height (`nSuperblockStartBlock`); superblocks are at
    /// `start + k·cycle`.
    pub superblock_start_height: u32,
    /// Blocks between superblocks (`nSuperblockCycle`).
    pub superblock_cycle: u32,
    /// Blocks before a superblock when voting closes
    /// (`nSuperblockMaturityWindow`).
    pub maturity_window: u32,
    /// Floor of the passing threshold (`nGovernanceMinQuorum`).
    pub min_quorum: u32,
    /// Proposal collateral (`GOVERNANCE_PROPOSAL_FEE_TX`), duffs.
    pub proposal_fee: u64,
    /// Target block spacing, seconds (ETA estimates).
    pub target_spacing_secs: u32,
}

/// Collateral confirmations before a proposal leaves "Confirming"
/// (`GOVERNANCE_FEE_CONFIRMATIONS`).
pub const PROPOSAL_FEE_CONFIRMATIONS: u32 = 6;
/// Confirmations dash-qt waits for before "Broadcast" is enabled.
pub const PROPOSAL_MIN_SUBMIT_CONFIRMATIONS: u32 = 1;
/// Longest proposal name (`MAX_NAME_SIZE`, `governance/object.cpp`).
pub const PROPOSAL_NAME_MAX_LEN: usize = 40;
/// Largest serialized proposal payload (`MAX_DATA_SIZE`,
/// `governance/object.cpp`).
pub const PROPOSAL_MAX_PAYLOAD_BYTES: usize = 512;
/// Payments a proposal may request in the wizard.
pub const PROPOSAL_MAX_PAYMENTS: u32 = 12;
/// Vote weight of an EvoNode (`voting_weight`, `evo/dmn_types.h`); a
/// regular masternode weighs 1.
pub const EVONODE_VOTE_WEIGHT: u32 = 4;
/// The network refuses a vote change from one masternode on one object more
/// often than this (`GOVERNANCE_UPDATE_MIN`, "Masternode voting too
/// often").
pub const VOTE_UPDATE_MIN_SECS: u64 = 60 * 60;

/// The parameters of `network`. Regtest values are Core's defaults;
/// `-budgetparams` can change them on a regtest node, which the wallet
/// cannot see.
pub fn governance_params(network: Network) -> GovernanceParams {
    let (start, cycle, window, quorum) = match network {
        Network::Mainnet => (614_820, 16_616, 1_662, 10),
        Network::Testnet | Network::Devnet => (4_200, 24, 8, 1),
        Network::Regtest => (1_500, 20, 10, 1),
    };
    GovernanceParams {
        superblock_start_height: start,
        superblock_cycle: cycle,
        maturity_window: window,
        min_quorum: quorum,
        proposal_fee: COIN,
        target_spacing_secs: 150,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mainnet_cycle_and_window_match_core() {
        let p = governance_params(Network::Mainnet);
        assert_eq!((p.superblock_cycle, p.maturity_window), (16_616, 1_662));
        assert_eq!(governance_params(Network::Regtest).superblock_cycle, 20);
        assert_eq!(p.proposal_fee, 100_000_000);
    }
}
