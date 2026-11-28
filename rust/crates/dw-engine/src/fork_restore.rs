use std::collections::{BTreeMap, BTreeSet, HashSet};

use dashcore::OutPoint;
use key_wallet::wallet::managed_wallet_info::ManagedWalletInfo;
use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;

const SPEND_OBSERVATION_LIMIT: usize = 16_384;

pub(crate) fn unspent_outpoints(core: &ManagedWalletInfo) -> HashSet<OutPoint> {
    core.accounts
        .all_funding_accounts()
        .into_iter()
        .flat_map(|account| account.utxos.keys().copied())
        .collect()
}

#[derive(Debug)]
struct CoinRead {
    height: u32,
    unspent: HashSet<OutPoint>,
    observed_spent: Option<BTreeMap<OutPoint, u32>>,
}

#[derive(Debug)]
struct Hold {
    height: u32,
    max_rescan_start: u32,
    coins: BTreeSet<OutPoint>,
    unknown: bool,
}

#[derive(Debug, Default)]
pub(crate) struct ForkRestores {
    last_read: Option<CoinRead>,
    new_outputs: HashSet<OutPoint>,
    hold: Option<Hold>,
    fork_revision: u64,
    rescan_target: Option<u32>,
}

impl ForkRestores {
    pub(crate) fn note_output(&mut self, coin: OutPoint) {
        if self.last_read.is_some() {
            self.new_outputs.insert(coin);
        }
    }

    pub(crate) fn note_event_fork(&mut self, height: u32) {
        self.fork_revision += 1;
        self.rescan_target = None;
        self.last_read = None;
        self.new_outputs.clear();
        if let Some(hold) = &mut self.hold {
            hold.max_rescan_start = hold.max_rescan_start.min(height.saturating_sub(1));
        }
    }

    pub(crate) fn infer(
        &mut self,
        previous: u32,
        height: u32,
        wallet_birth: u32,
        unspent: &HashSet<OutPoint>,
    ) {
        self.fork_revision += 1;
        self.rescan_target = None;
        let baseline = self.last_read.as_ref().filter(|r| r.height == previous);
        let Some(baseline) = baseline else {
            self.hold_at(height, wallet_birth).unknown = true;
            return;
        };
        let restored: BTreeSet<_> = unspent
            .iter()
            .copied()
            .filter(|coin| {
                // Funding learned after the read can still be a restored
                // input if its spend was observed before the funding.
                let new_output = self.new_outputs.contains(coin)
                    && baseline
                        .observed_spent
                        .as_ref()
                        .is_some_and(|spent| !spent.contains_key(coin));
                !baseline.unspent.contains(coin) && !new_output
            })
            .collect();
        if !restored.is_empty() {
            // Detection can run after part of the new chain was scanned.
            // Use the older spend evidence to bound where its replay starts.
            let replay_limit = restored
                .iter()
                .map(|coin| {
                    baseline
                        .observed_spent
                        .as_ref()
                        .and_then(|spent| spent.get(coin))
                        .map_or(wallet_birth, |spent_at| spent_at.saturating_sub(2))
                })
                .min()
                .unwrap_or(wallet_birth);
            self.hold_at(height, replay_limit).coins.extend(restored);
        }
    }

    fn hold_at(&mut self, height: u32, max_rescan_start: u32) -> &mut Hold {
        let max_rescan_start = max_rescan_start.min(height.saturating_sub(1));
        let hold = self.hold.get_or_insert_with(|| Hold {
            height,
            max_rescan_start,
            coins: BTreeSet::new(),
            unknown: false,
        });
        hold.height = hold.height.max(height);
        hold.max_rescan_start = hold.max_rescan_start.min(max_rescan_start);
        hold
    }

    pub(crate) fn held(&self) -> impl Iterator<Item = OutPoint> + '_ {
        self.hold.iter().flat_map(|h| h.coins.iter().copied())
    }

    pub(crate) fn rescan_start_for_unknown(&self) -> Option<u32> {
        self.hold
            .as_ref()
            .filter(|h| h.unknown)
            .map(|h| h.max_rescan_start)
    }

    pub(crate) fn holding(&self) -> bool {
        self.hold.is_some()
    }

    pub(crate) fn rescan_revision(&self, from: u32) -> Option<u64> {
        self.hold
            .as_ref()
            .filter(|hold| from <= hold.max_rescan_start)
            .map(|_| self.fork_revision)
    }

    pub(crate) fn start_rescan(&mut self, revision: u64, target: u32) {
        if revision == self.fork_revision
            && let Some(hold) = &self.hold
        {
            self.rescan_target = Some(target.max(hold.height));
        }
    }

    pub(crate) fn cancel_rescan(&mut self) {
        self.rescan_target = None;
    }

    pub(crate) fn record_read(&mut self, core: &ManagedWalletInfo) -> bool {
        let unspent = unspent_outpoints(core);
        let observed = core.observed_spent_outpoints();
        let observed_spent = (observed.len() <= SPEND_OBSERVATION_LIMIT).then(|| observed.clone());
        self.last_read = Some(CoinRead {
            height: core.last_processed_height(),
            unspent,
            observed_spent,
        });
        self.new_outputs.clear();
        if self
            .rescan_target
            .is_some_and(|target| core.synced_height() >= target)
        {
            self.rescan_target = None;
            return self.hold.take().is_some();
        }
        false
    }
}
