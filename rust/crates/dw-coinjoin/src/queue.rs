//! Known mixing queues (`dsq`) on the client side: Dash Core
//! `CJWalletManagerImpl::ProcessDSQueue` (`src/coinjoin/walletman.cpp:243-329`),
//! `CoinJoinQueueManager` (`src/coinjoin/coinjoin.cpp:170-235`) and the
//! per-masternode queue counters of `CMasternodeMetaMan`
//! (`AllowMixing`/`IsMixingThresholdExceeded`, `src/masternode/meta.cpp`).

use std::collections::HashMap;

use crate::messages::{Queue, denomination_to_amount};

/// What happened to a received `dsq`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueVerdict {
    /// A new queue to join later.
    Added,
    /// A masternode announced that its session is ready: the session mixing
    /// with it submits its entry.
    Ready { pro_tx_hash: [u8; 32], denom: u32 },
    /// Dropped; the text says why (for logs).
    Ignored(&'static str),
    /// The signature does not verify against the operator key (Core gives
    /// the relaying peer 10 misbehavior points).
    BadSignature,
}

/// The parts of a list entry a queue check needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueMasternode {
    pub operator_public_key: [u8; 48],
    pub is_valid: bool,
}

#[derive(Debug, Clone)]
struct Known {
    queue: Queue,
    hash: [u8; 32],
    tried: bool,
}

/// Queues seen and the queue counters per masternode.
#[derive(Debug, Default)]
pub struct QueueManager {
    queues: Vec<Known>,
    /// `nDsqCount`: queues allowed so far.
    dsq_count: u64,
    /// `nLastDsq` per masternode.
    last_dsq: HashMap<[u8; 32], u64>,
}

impl QueueManager {
    pub fn len(&self) -> usize {
        self.queues.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queues.is_empty()
    }

    /// `IsMixingThresholdExceeded`: the masternode announced a queue within
    /// the last `enabled / 5` queues of the network.
    pub fn threshold_exceeded(&self, pro_tx_hash: &[u8; 32], enabled_count: usize) -> bool {
        match self.last_dsq.get(pro_tx_hash) {
            Some(last) if *last != 0 => last + (enabled_count as u64 / 5) > self.dsq_count,
            _ => false,
        }
    }

    /// `AllowMixing`: counts a queue of this masternode.
    pub fn allow_mixing(&mut self, pro_tx_hash: [u8; 32]) {
        self.dsq_count += 1;
        self.last_dsq.insert(pro_tx_hash, self.dsq_count);
    }

    /// Processes one received `dsq`. `masternode` is the valid list entry of
    /// its proTxHash (`None` when not in the list or banned); `now` is UNIX
    /// seconds; `enabled_count` the list's enabled count.
    pub fn receive(
        &mut self,
        dsq: Queue,
        masternode: Option<&QueueMasternode>,
        now: i64,
        enabled_count: usize,
    ) -> QueueVerdict {
        if denomination_to_amount(dsq.denom).is_none() {
            return QueueVerdict::Ignored("invalid denomination");
        }
        let hash = dsq.hash();
        if self.queues.iter().any(|k| k.hash == hash) {
            return QueueVerdict::Ignored("already known");
        }
        if self
            .queues
            .iter()
            .any(|k| k.queue.pro_tx_hash == dsq.pro_tx_hash && k.queue.ready == dsq.ready)
        {
            return QueueVerdict::Ignored("masternode already has a queue with this readiness");
        }
        if dsq.is_time_out_of_bounds(now) {
            return QueueVerdict::Ignored("time out of bounds");
        }
        let Some(mn) = masternode.filter(|m| m.is_valid) else {
            return QueueVerdict::Ignored("masternode not in the valid list");
        };
        if !dsq.verify(&mn.operator_public_key) {
            return QueueVerdict::BadSignature;
        }
        if dsq.ready {
            return QueueVerdict::Ready {
                pro_tx_hash: dsq.pro_tx_hash,
                denom: dsq.denom,
            };
        }
        if self.threshold_exceeded(&dsq.pro_tx_hash, enabled_count) {
            return QueueVerdict::Ignored("masternode sends queues too often");
        }
        self.allow_mixing(dsq.pro_tx_hash);
        self.queues.push(Known {
            queue: dsq,
            hash,
            tried: false,
        });
        QueueVerdict::Added
    }

    /// Marks a queue of a masternode we already mix with as tried
    /// (`MarkAlreadyJoinedQueueAsTried`).
    pub fn mark_tried_for(&mut self, pro_tx_hash: &[u8; 32]) {
        for k in &mut self.queues {
            if &k.queue.pro_tx_hash == pro_tx_hash {
                k.tried = true;
            }
        }
    }

    /// `GetQueueItemAndTry`: the first untried, in-bounds queue (of
    /// `denom_filter` when non-zero), marked tried.
    pub fn next_untried(&mut self, denom_filter: u32, now: i64) -> Option<Queue> {
        for k in &mut self.queues {
            if k.tried || k.queue.is_time_out_of_bounds(now) {
                continue;
            }
            if denom_filter != 0 && k.queue.denom != denom_filter {
                continue;
            }
            k.tried = true;
            return Some(k.queue.clone());
        }
        None
    }

    /// `CheckQueue`: forgets timed-out queues.
    pub fn expire(&mut self, now: i64) {
        self.queues.retain(|k| !k.queue.is_time_out_of_bounds(now));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bls::testing::OperatorKey;

    fn signed(key: &OperatorKey, mn: u8, denom: u32, time: i64, ready: bool) -> Queue {
        let mut q = Queue {
            denom,
            pro_tx_hash: [mn; 32],
            time,
            ready,
            signature: vec![],
        };
        q.signature = key.sign(&q.signature_hash());
        q
    }

    #[test]
    fn test_qt_045_queues_are_verified_deduplicated_and_rate_limited() {
        let key = OperatorKey::from_seed(b"op");
        let mn = QueueMasternode {
            operator_public_key: key.public_key(),
            is_valid: true,
        };
        let now = 1_700_000_000;
        let mut qm = QueueManager::default();
        let q = signed(&key, 1, 4, now, false);
        assert_eq!(
            qm.receive(q.clone(), Some(&mn), now, 10),
            QueueVerdict::Added
        );
        assert!(matches!(
            qm.receive(q, Some(&mn), now, 10),
            QueueVerdict::Ignored(_)
        ));
        // Same masternode, same readiness, different time: dropped.
        let again = signed(&key, 1, 4, now + 1, false);
        assert!(matches!(
            qm.receive(again, Some(&mn), now, 10),
            QueueVerdict::Ignored(_)
        ));
        // A forged signature.
        let mut forged = signed(&key, 2, 4, now, false);
        forged.signature = OperatorKey::from_seed(b"other").sign(&forged.signature_hash());
        assert_eq!(
            qm.receive(forged, Some(&mn), now, 10),
            QueueVerdict::BadSignature
        );
        // Unknown or banned masternode, stale time, bad denomination.
        assert!(matches!(
            qm.receive(signed(&key, 3, 4, now, false), None, now, 10),
            QueueVerdict::Ignored(_)
        ));
        assert!(matches!(
            qm.receive(signed(&key, 3, 4, now - 31, false), Some(&mn), now, 10),
            QueueVerdict::Ignored(_)
        ));
        assert!(matches!(
            qm.receive(signed(&key, 3, 3, now, false), Some(&mn), now, 10),
            QueueVerdict::Ignored(_)
        ));
        // Ready queues are reported, not stored.
        assert_eq!(
            qm.receive(signed(&key, 1, 4, now, true), Some(&mn), now, 10),
            QueueVerdict::Ready {
                pro_tx_hash: [1; 32],
                denom: 4
            }
        );
        assert_eq!(qm.len(), 1);
        // With 10 enabled masternodes, MN 1 may queue again after 2 others.
        assert!(qm.threshold_exceeded(&[1; 32], 10));
        qm.allow_mixing([5; 32]);
        qm.allow_mixing([6; 32]);
        assert!(!qm.threshold_exceeded(&[1; 32], 10));
    }

    #[test]
    fn untried_queues_are_handed_out_once() {
        let key = OperatorKey::from_seed(b"op");
        let mn = QueueMasternode {
            operator_public_key: key.public_key(),
            is_valid: true,
        };
        let now = 1_700_000_000;
        let mut qm = QueueManager::default();
        qm.receive(signed(&key, 1, 4, now, false), Some(&mn), now, 100);
        qm.receive(signed(&key, 2, 8, now, false), Some(&mn), now, 100);
        assert_eq!(qm.next_untried(8, now).unwrap().pro_tx_hash, [2; 32]);
        assert_eq!(qm.next_untried(0, now).unwrap().pro_tx_hash, [1; 32]);
        assert!(qm.next_untried(0, now).is_none());
        qm.expire(now + 31);
        assert!(qm.is_empty());
    }
}
