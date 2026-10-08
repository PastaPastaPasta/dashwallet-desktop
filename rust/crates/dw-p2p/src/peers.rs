//! Picking peers from a masternode-list snapshot.
//!
//! CoinJoin connects to masternodes chosen at random from the valid ones,
//! skipping those used recently (Dash Core `CMasternodeMetaMan`
//! `AddUsedMasternode`/`IsUsedMasternode`/`RemoveUsedMasternodes`,
//! `src/masternode/meta.h`; the 90 %/70 % trimming of
//! `CCoinJoinClientManager::DoAutomaticDenominating`,
//! `src/coinjoin/client.cpp:1231-1242`).

use std::collections::VecDeque;
use std::net::SocketAddr;

use rand::seq::SliceRandom;

/// What the picker needs from one masternode-list entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerEntry {
    /// proTxHash, wire order.
    pub pro_tx_hash: [u8; 32],
    /// Primary Core P2P address; `None` for entries without one.
    pub service: Option<SocketAddr>,
    /// Operator BLS public key as serialized in the list.
    pub operator_public_key: [u8; 48],
    /// Not PoSe-banned.
    pub is_valid: bool,
    pub is_evonode: bool,
}

/// A snapshot of the list plus the "recently used" ring.
#[derive(Debug, Clone, Default)]
pub struct PeerPicker {
    entries: Vec<PeerEntry>,
    used: VecDeque<[u8; 32]>,
}

impl PeerPicker {
    pub fn new(entries: Vec<PeerEntry>) -> Self {
        Self {
            entries,
            used: VecDeque::new(),
        }
    }

    /// Replaces the snapshot, keeping the used ring (a new list at a new
    /// tip, as Core keeps its meta store across list updates).
    pub fn update(&mut self, entries: Vec<PeerEntry>) {
        self.entries = entries;
    }

    /// Valid masternodes with a reachable service address.
    pub fn masternodes(&self) -> impl Iterator<Item = &PeerEntry> {
        self.entries
            .iter()
            .filter(|e| e.is_valid && e.service.is_some())
    }

    /// `ForEachMN(onlyValid=true)` count, the `enabled` of Core's list counts.
    pub fn enabled_count(&self) -> usize {
        self.entries.iter().filter(|e| e.is_valid).count()
    }

    pub fn by_pro_tx_hash(&self, hash: &[u8; 32]) -> Option<&PeerEntry> {
        self.entries.iter().find(|e| &e.pro_tx_hash == hash)
    }

    pub fn is_used(&self, hash: &[u8; 32]) -> bool {
        self.used.contains(hash)
    }

    pub fn used_count(&self) -> usize {
        self.used.len()
    }

    /// Adds `hash` to the ring (Core `AddUsedMasternode`).
    pub fn mark_used(&mut self, hash: [u8; 32]) {
        if !self.used.contains(&hash) {
            self.used.push_back(hash);
        }
    }

    /// Once more than 90 % of the enabled masternodes were used, forgets the
    /// oldest until 70 % of that threshold remain (client.cpp:1231-1242).
    pub fn trim_used(&mut self) {
        let high = (self.enabled_count() as f64 * 0.9) as usize;
        let low = (high as f64 * 0.7) as usize;
        if self.used.len() > high {
            let remove = self.used.len() - low;
            for _ in 0..remove {
                self.used.pop_front();
            }
        }
    }

    /// A random valid masternode not in the used ring
    /// (`CCoinJoinClientSession::GetRandomNotUsedMasternode`,
    /// client.cpp:1264-1298).
    pub fn random_unused_masternode(&self) -> Option<&PeerEntry> {
        let mut candidates: Vec<&PeerEntry> = self
            .masternodes()
            .filter(|e| !self.is_used(&e.pro_tx_hash))
            .collect();
        candidates.shuffle(&mut rand::thread_rng());
        candidates.into_iter().next()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(seed: u8, valid: bool) -> PeerEntry {
        PeerEntry {
            pro_tx_hash: [seed; 32],
            service: Some(SocketAddr::from(([10, 0, 0, seed], 9999))),
            operator_public_key: [seed; 48],
            is_valid: valid,
            is_evonode: false,
        }
    }

    #[test]
    fn picks_only_valid_unused_masternodes() {
        let mut p = PeerPicker::new(vec![entry(1, true), entry(2, false), entry(3, true)]);
        assert_eq!(p.enabled_count(), 2);
        p.mark_used([1; 32]);
        for _ in 0..20 {
            assert_eq!(p.random_unused_masternode().unwrap().pro_tx_hash, [3; 32]);
        }
        p.mark_used([3; 32]);
        assert!(p.random_unused_masternode().is_none());
    }

    #[test]
    fn trims_the_used_ring_like_core() {
        let entries: Vec<_> = (1..=10).map(|i| entry(i, true)).collect();
        let mut p = PeerPicker::new(entries);
        for i in 1..=10 {
            p.mark_used([i; 32]);
        }
        // high = 9, low = 6: 10 used > 9, so 4 of the oldest go.
        p.trim_used();
        assert_eq!(p.used_count(), 6);
        assert!(!p.is_used(&[1; 32]) && p.is_used(&[10; 32]));
    }
}
