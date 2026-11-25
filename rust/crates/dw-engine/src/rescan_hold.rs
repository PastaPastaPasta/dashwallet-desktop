//! Coins an abandon or a drop gives back with no durable evidence that
//! they are unspent wait for that rewind's rescan (#818, Q100 E2).
//!
//! key-wallet keeps a coin a processed block spent in its observed spends
//! only until a ChainLock prunes them, so an abandon or drop that runs
//! after that prune gives the coin back although a final block spent it.
//! The rescan the give-back rewinds for records that block's spend again
//! once it reaches the block. Until it is back above the height it was at
//! when the coin was given back, the coin is held: neither counted nor
//! offered.
//!
//! Each hold is stored as a [`RESCAN_HOLD_PREFIX`] mark in the same write
//! as the abandon's marks, after the rewind's replay record
//! (`crate::replay_record`). A later open reads them back and resumes that
//! replay, so a hold outlives a restart until its rescan ends.

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

use dashcore::OutPoint;
use dw_appdb::AppDb;

use crate::events::WalletState;
use crate::WalletId;

/// Settings key prefix (wallet scope) of a coin given back that waits for
/// its rescan; the value is `checkpoint,target` ([`RescanHold::encode`]).
pub(crate) const RESCAN_HOLD_PREFIX: &str = "rescan_hold:";

/// One coin's hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RescanHold {
    /// The checkpoint the give-back rewinds to: at or below the coin's
    /// funding height.
    pub checkpoint: u32,
    /// The height the rescan must be back at: the replay's target when
    /// the coin was given back.
    pub target: u32,
    /// The rewind is applied (or a later open resumes its replay): an
    /// end of a replay can release the hold. Before it, the scan's height
    /// still predates the give-back.
    pub armed: bool,
}

impl RescanHold {
    pub(crate) fn new(checkpoint: u32, target: u32) -> Self {
        Self {
            checkpoint,
            target,
            armed: false,
        }
    }

    pub(crate) fn encode(&self) -> String {
        format!("{},{}", self.checkpoint, self.target)
    }

    /// A stored hold, armed: the open that reads it resumes its replay.
    /// One whose value does not read waits for a rescan from the start.
    fn decode(value: Option<&str>) -> Self {
        let parsed = value.and_then(|v| {
            let (c, t) = v.split_once(',')?;
            Some((c.parse().ok()?, t.parse().ok()?))
        });
        let (checkpoint, target) = parsed.unwrap_or((0, u32::MAX));
        Self {
            checkpoint,
            target,
            armed: true,
        }
    }

    /// Whether the hold still keeps its coin, `state` being its wallet's
    /// scan state and `rescanning` whether a rescan of it is under way.
    pub(crate) fn holds(&self, state: Option<&WalletState>, rescanning: bool) -> bool {
        let Some(state) = state else {
            return true;
        };
        !self.armed
            || rescanning
            || state.replaying_at(state.synced_height)
            || state.synced_height < self.target
    }
}

/// The marks storing `holds`.
pub(crate) fn hold_marks<'a>(
    holds: impl IntoIterator<Item = (&'a OutPoint, &'a RescanHold)>,
) -> Vec<(String, Option<String>)> {
    (holds.into_iter())
        .map(|(o, h)| (format!("{RESCAN_HOLD_PREFIX}{o}"), Some(h.encode())))
        .collect()
}

/// The keys of `coins`' marks.
pub(crate) fn hold_keys<'a>(coins: impl IntoIterator<Item = &'a OutPoint>) -> Vec<String> {
    (coins.into_iter())
        .map(|o| format!("{RESCAN_HOLD_PREFIX}{o}"))
        .collect()
}

/// `wallet`'s stored holds, armed. `Err` when they cannot be read: the
/// open then keeps the history incomplete.
pub(crate) fn load_rescan_holds(
    appdb: &AppDb,
    wallet: WalletId,
) -> Result<BTreeMap<OutPoint, RescanHold>, dw_appdb::AppDbError> {
    Ok(
        (appdb.settings_with_prefix_lossy(&wallet.to_string(), RESCAN_HOLD_PREFIX)?)
            .into_iter()
            .filter_map(|(key, value)| {
                let coin = OutPoint::from_str(&key[RESCAN_HOLD_PREFIX.len()..]).ok()?;
                Some((coin, RescanHold::decode(value.as_deref())))
            })
            .collect(),
    )
}

/// The holds an open of `wallet` keeps: every stored one while the
/// wallet has a replay record, which the open resumes. With none, each
/// hold's replay is back and stored, so their marks go. `Err` when they
/// cannot be read.
pub(crate) fn load_for_open(
    appdb: &AppDb,
    wallet: WalletId,
) -> Result<BTreeMap<OutPoint, RescanHold>, dw_appdb::AppDbError> {
    let holds = load_rescan_holds(appdb, wallet)?;
    if holds.is_empty() || crate::replay_record::stored_in(appdb, wallet)? {
        return Ok(holds);
    }
    let keys = hold_keys(holds.keys());
    let writes: Vec<(&str, Option<&str>)> = keys.iter().map(|k| (k.as_str(), None)).collect();
    if let Err(e) = appdb.set_settings_durable(&wallet.to_string(), &writes) {
        tracing::warn!(%wallet, error = %e, "could not drop the marks of coins whose rescan is back");
    }
    Ok(BTreeMap::new())
}

/// The coins of `holds` still held.
pub(crate) fn held(
    holds: &BTreeMap<OutPoint, RescanHold>,
    state: Option<&WalletState>,
    rescanning: bool,
) -> BTreeSet<OutPoint> {
    (holds.iter())
        .filter(|(_, h)| h.holds(state, rescanning))
        .map(|(o, _)| *o)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(synced: u32, target: u32) -> WalletState {
        WalletState {
            synced_height: synced,
            scan_target: target,
            ..WalletState::default()
        }
    }

    #[test]
    fn a_hold_is_released_only_once_armed_and_back_at_its_target() {
        let mut hold = RescanHold::new(99, 200);
        assert!(hold.holds(Some(&state(200, 200)), false), "not armed");
        hold.armed = true;
        assert!(hold.holds(Some(&state(150, 200)), false), "replaying");
        assert!(hold.holds(Some(&state(150, 150)), false), "below the target");
        assert!(hold.holds(Some(&state(200, 200)), true), "rescanning");
        assert!(hold.holds(None, false), "no state");
        assert!(!hold.holds(Some(&state(200, 200)), false));
        assert!(!hold.holds(Some(&state(250, 250)), false));
    }

    #[test]
    fn a_stored_hold_reads_back_armed_and_one_that_does_not_read_holds() {
        let hold = RescanHold::new(99, 200);
        let back = RescanHold::decode(Some(&hold.encode()));
        assert_eq!(
            back,
            RescanHold {
                armed: true,
                ..hold
            }
        );
        let unread = RescanHold::decode(Some("x"));
        assert!(unread.holds(Some(&state(u32::MAX - 1, u32::MAX - 1)), false));
        assert!(RescanHold::decode(None).target == u32::MAX);
    }
}
