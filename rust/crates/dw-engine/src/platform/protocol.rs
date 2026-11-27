//! The network's Platform protocol version, for the features and prices
//! that follow it (release 0.1: the contested-name lock, the shielded pool's
//! gate) and for `NetworkSession::protocol_compatibility` (`startup.rs`).
//!
//! The SDK starts at the network's floor (`dash_sdk::sdk::min_protocol_version`,
//! 14 on every network at the pin) or the session's
//! `initial_protocol_version`, and every proof-verified response's metadata
//! ratchets it up to at most `PlatformVersion::latest()`. [`NetworkSession::protocol_version`]
//! reads that clamped value offline; [`NetworkSession::refresh_protocol_version`]
//! asks Platform once (a proven `getEpochsInfo`) and keeps returning the
//! clamped SDK version so an upgraded network blocks no flow.
//!
//! [`ProtocolRuntime`] records the unclamped version Platform actually runs,
//! from `ExtendedEpochInfo::fetch_current`'s proved epoch (which decodes raw
//! `u32` bytes and verifies the quorum signature without `PlatformVersion::get`),
//! but only when every quorum key that read's proofs used was SPV-verified
//! (`trust::scoped`). A key the trusted service served in Degraded mode
//! proves nothing about the network's rules, so a version seen under one, or
//! ratcheted by any other response through
//! `SdkBuilder::with_protocol_version_observer`, is logged and never raises it.
//! Before the first verified answer it is `None` ("unknown").

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use dash_sdk::platform::fetch_current_no_parameters::FetchCurrent;
use dash_sdk::sdk::ProtocolVersionObserver;
use dpp::block::extended_epoch_info::ExtendedEpochInfo;
use dpp::block::extended_epoch_info::v0::ExtendedEpochInfoV0Getters;
use dpp::version::PlatformVersion;
use tokio::runtime::Handle;
use tokio::task::JoinSet;

use super::errors::PlatformError;
use super::trust;
use crate::events::SessionHub;
use crate::session::Manager;
use crate::{NetworkSession, PlatformChange, WalletId};

#[cfg(test)]
pub(crate) type TestProtocolFetch = Arc<dyn Fn() -> Result<u32, PlatformError> + Send + Sync>;

type SessionReach = (Weak<Manager>, Weak<SessionHub>);

/// Tracks the highest Platform protocol version proved under SPV-verified
/// quorum keys in this session (`0` = unknown), and the highest seen only
/// under unverified ones (for the log), and runs background epoch refreshes
/// while SPV runs.
pub(crate) struct ProtocolRuntime {
    observed: AtomicU32,
    unverified: AtomicU32,
    reach: Mutex<Option<SessionReach>>,
    tasks: Mutex<Option<JoinSet<()>>>,
    pass: tokio::sync::Mutex<()>,
    #[cfg(test)]
    pub(crate) test_fetch: Mutex<Option<TestProtocolFetch>>,
    #[cfg(test)]
    pub(crate) hold_fetch: Mutex<Option<Arc<crate::coins::TestPause>>>,
}

fn guard<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Default for ProtocolRuntime {
    fn default() -> Self {
        Self {
            observed: AtomicU32::new(0),
            unverified: AtomicU32::new(0),
            reach: Mutex::new(None),
            tasks: Mutex::new(Some(JoinSet::new())),
            pass: tokio::sync::Mutex::new(()),
            #[cfg(test)]
            test_fetch: Mutex::new(None),
            #[cfg(test)]
            hold_fetch: Mutex::new(None),
        }
    }
}

impl ProtocolRuntime {
    pub(crate) fn attach(&self, manager: Weak<Manager>, hub: Weak<SessionHub>) {
        *guard(&self.reach) = Some((manager, hub));
    }

    pub(crate) fn observed(&self) -> Option<u32> {
        let v = self.observed.load(Ordering::Acquire);
        (v > 0).then_some(v)
    }

    pub(crate) fn observer(self: &Arc<Self>) -> ProtocolVersionObserver {
        let this = Arc::clone(self);
        Arc::new(move |version| this.record(version, false))
    }

    /// `verified`: every quorum key the proof used was SPV-verified. Only
    /// then does `version` count for `protocol_compatibility` and a
    /// pre-check's `credits.rules_unsupported`.
    pub(crate) fn record(&self, version: u32, verified: bool) {
        if version == 0 {
            return;
        }
        if !verified {
            let previous = self.unverified.fetch_max(version, Ordering::AcqRel);
            if previous < version && version > self.observed.load(Ordering::Acquire) {
                tracing::debug!(
                    target: "dash_sdk::protocol_version",
                    version,
                    "protocol version not attributed to SPV-verified quorum keys \
                     (a response's metadata, or a read under a key SPV has not \
                     verified); not used for compatibility or refusals"
                );
            }
            return;
        }
        let previous = self.observed.fetch_max(version, Ordering::AcqRel);
        if previous >= version {
            return;
        }
        let reach = guard(&self.reach).clone();
        let Some((manager, hub)) = reach else {
            return;
        };
        let Some(hub) = hub.upgrade() else {
            return;
        };
        if let Some(manager) = manager.upgrade() {
            for id in manager.list_wallet_ids_blocking() {
                hub.pump
                    .mark_platform(WalletId(id), [PlatformChange::Startup]);
            }
        }
    }

    pub(crate) async fn fetch(&self, sdk: &dash_sdk::Sdk) -> Option<u32> {
        let (fetched, tally) = trust::scoped(self.read_network(sdk)).await;
        if let Some(version) = fetched {
            self.record(version, tally.verified());
        }
        fetched
    }

    async fn read_network(&self, sdk: &dash_sdk::Sdk) -> Option<u32> {
        #[cfg(test)]
        {
            crate::coins::pause_at(&self.hold_fetch).await;
            let hook = guard(&self.test_fetch).clone();
            if let Some(hook) = hook {
                return hook().ok().filter(|&v| v > 0);
            }
        }
        fetch_epoch_protocol_version(sdk).await
    }

    pub(crate) fn kick(self: &Arc<Self>, rt: &Handle) {
        #[cfg(test)]
        if guard(&self.test_fetch).is_none() {
            return;
        }
        let this = Arc::clone(self);
        let mut tasks = guard(&self.tasks);
        let Some(set) = tasks.as_mut() else {
            return;
        };
        while set.try_join_next().is_some() {}
        set.spawn_on(
            async move {
                let Ok(_pass) = this.pass.try_lock() else {
                    return;
                };
                let sdk = guard(&this.reach)
                    .as_ref()
                    .and_then(|(m, _)| m.upgrade())
                    .map(|m| m.sdk_arc());
                let Some(sdk) = sdk else {
                    return;
                };
                let _ = this.fetch(&sdk).await;
            },
            rt,
        );
    }

    pub(crate) fn stop(&self) {
        if let Some(set) = guard(&self.tasks).as_mut() {
            set.abort_all();
            while set.try_join_next().is_some() {}
        }
    }

    pub(crate) async fn close(&self) {
        let set = guard(&self.tasks).take();
        if let Some(mut set) = set {
            set.abort_all();
            while set.join_next().await.is_some() {}
        }
    }
}

impl NetworkSession {
    /// The network's protocol version as the SDK last proved it (the floor
    /// before any proof). In-memory read.
    #[allow(dead_code, reason = "lane E1's shielded gate reads it")]
    pub(crate) fn protocol_version(&self) -> Result<u32, PlatformError> {
        let _op = self.try_enter()?;
        Ok(self.manager()?.sdk_arc().protocol_version_number())
    }

    /// The network's protocol version after one proven read of Platform's
    /// current epoch; the version learned so far when that read fails, and
    /// clamped to at most `PlatformVersion::latest()` so an upgraded network
    /// blocks no flow while `protocol_compatibility` records the real version.
    pub(crate) async fn refresh_protocol_version(self: &Arc<Self>) -> Result<u32, PlatformError> {
        let this = Arc::clone(self);
        Ok(self
            .on_runtime(async move {
                #[cfg(test)]
                crate::coins::pause_at(&this.platform.hold_refresh).await;
                let _op = this.enter().await?;
                let sdk = this.manager()?.sdk_arc();
                // A close does not wait for this read: its guard would hold
                // the close's gate for as long as Platform takes to answer.
                tokio::select! {
                    biased;
                    () = this.gate.until_closed() => return Err(this.not_open()),
                    _ = this.platform.protocol.fetch(&sdk) => {}
                }
                Ok(sdk.protocol_version_number())
            })
            .await?)
    }

    pub(crate) fn kick_protocol_refresh(&self) {
        if self.platform.is_running() {
            self.platform.protocol.kick(&self.rt);
        }
    }
}

/// Queries the newest started epoch (proved) and returns its unclamped
/// `protocol_version`. Inside `ExtendedEpochInfo::fetch_current`, verified
/// response metadata also ratchets `sdk`'s clamped version upward when known.
async fn fetch_epoch_protocol_version(sdk: &dash_sdk::Sdk) -> Option<u32> {
    if !sdk.prove() {
        return None;
    }
    match ExtendedEpochInfo::fetch_current(sdk).await {
        Ok(epoch) => Some(epoch.protocol_version()).filter(|&v| v > 0),
        Err(error) => {
            tracing::warn!(
                target: "dash_sdk::protocol_version",
                %error,
                version = sdk.protocol_version_number(),
                "proven protocol-version refresh failed; keeping the highest \
                 proof-verified version learned so far"
            );
            None
        }
    }
}

/// `sdk`'s protocol version after one proven refresh; the one it holds when
/// the refresh fails (the SDK logs why).
pub(crate) async fn refreshed(sdk: &dash_sdk::Sdk) -> u32 {
    sdk.refresh_protocol_version()
        .await
        .unwrap_or_else(|_| sdk.protocol_version_number())
}

/// The rules at `protocol_version`; a version this build does not know is
/// `internal` (the SDK never ratchets past the latest it knows).
pub(crate) fn platform_version(
    protocol_version: u32,
) -> Result<&'static PlatformVersion, PlatformError> {
    PlatformVersion::get(protocol_version).map_err(|e| PlatformError::Internal {
        detail: format!("protocol version {protocol_version}: {e}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::ProtocolCompatibility;
    use crate::{DashNetwork, Engine, EngineConfig, EngineEvent, EventSink, SessionOptions};
    use dash_sdk::SdkBuilder;
    use dash_sdk::platform::LimitQuery;
    use dash_sdk::platform::types::epoch::EpochQuery;
    use dash_sdk::query_types::ExtendedEpochInfos;
    use dpp::block::extended_epoch_info::v0::ExtendedEpochInfoV0;
    use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};

    struct Collect(Mutex<Vec<EngineEvent>>);

    /// An epoch read whose proofs used the given quorum keys, verified or not.
    fn fetch_under(keys: &'static [bool], version: u32) -> TestProtocolFetch {
        Arc::new(move || {
            for &verified in keys {
                trust::note_lookup(verified);
            }
            Ok(version)
        })
    }

    fn verified_fetch(version: u32) -> TestProtocolFetch {
        fetch_under(&[true], version)
    }

    impl EventSink for Collect {
        fn emit(&self, event: EngineEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    fn test_engine() -> (tempfile::TempDir, Engine, Arc<Collect>) {
        let dir = dw_testutil::private_tempdir();
        let sink = Arc::new(Collect(Mutex::new(Vec::new())));
        let engine = Engine::new(
            EngineConfig {
                data_root: dir.path().join("data"),
                worker_threads: Some(2),
                vault: VaultConfig {
                    kdf: KdfPolicy::Fixed(KdfParams::TEST),
                    os_store: Arc::new(MemoryOsStore::new()),
                    ..VaultConfig::default()
                },
            },
            Arc::clone(&sink) as Arc<dyn EventSink>,
        )
        .unwrap();
        (dir, engine, sink)
    }

    #[tokio::test]
    async fn fetch_reads_unclamped_epoch_version_above_supported_without_moving_sdk_past_latest() {
        let supported = PlatformVersion::latest().protocol_version;
        let upgraded = supported + 1;
        let epoch = ExtendedEpochInfo::from(ExtendedEpochInfoV0 {
            index: 0,
            first_block_time: 1_000,
            first_block_height: 10,
            first_core_block_height: 5,
            fee_multiplier_permille: 1_000,
            protocol_version: upgraded,
        });
        let mut sdk = SdkBuilder::new_mock().build().unwrap();
        sdk.mock()
            .expect_fetch::<ExtendedEpochInfo, _>(
                LimitQuery {
                    query: EpochQuery::genesis(),
                    limit: Some(1),
                    start_info: None,
                },
                Some(epoch.clone()),
            )
            .await
            .unwrap();
        sdk.mock()
            .expect_fetch_many::<_, ExtendedEpochInfo, _, ExtendedEpochInfos>(
                LimitQuery {
                    query: EpochQuery::ascending_from(0),
                    limit: Some(2),
                    start_info: None,
                },
                Some(ExtendedEpochInfos::from_iter([(0, Some(epoch))])),
            )
            .await
            .unwrap();

        // The mock answers without a quorum lookup: the version is read, but
        // nothing SPV verified proved it, so it is not observed.
        let runtime = ProtocolRuntime::default();
        assert_eq!(runtime.observed(), None);
        assert_eq!(runtime.fetch(&sdk).await, Some(upgraded));
        assert_eq!(runtime.observed(), None);
        assert!(sdk.protocol_version_number() <= supported);
    }

    #[test]
    fn protocol_compatibility_tracks_unknown_equal_upgraded_no_platform_and_close() {
        let (_dir, engine, _sink) = test_engine();
        let supported = PlatformVersion::latest().protocol_version;
        let opts = SessionOptions {
            dapi_addresses: vec!["https://127.0.0.1:1".into()],
            quorum_url: Some("https://127.0.0.1:1".into()),
            spv_peers: vec!["127.0.0.1:1".into()],
            ..Default::default()
        };
        let session = engine
            .block_on(engine.open_network(DashNetwork::Regtest, opts.clone()))
            .unwrap();
        let wallet = engine
            .block_on(session.vault_op(|v| {
                v.create(None)?;
                Ok(())
            }))
            .and_then(|()| engine.block_on(session.create_wallet(12)))
            .unwrap()
            .wallet_id;

        assert_eq!(
            session.protocol_compatibility().unwrap(),
            ProtocolCompatibility {
                network_version: None,
                supported_version: supported,
            }
        );

        // Equal to supported: recorded, never regresses, and signals the wallet.
        *session.platform.protocol.test_fetch.lock().unwrap() = Some(verified_fetch(supported));
        let clamped = engine.block_on(session.refresh_protocol_version()).unwrap();
        assert!(clamped <= supported);
        assert_eq!(
            session.protocol_compatibility().unwrap(),
            ProtocolCompatibility {
                network_version: Some(supported),
                supported_version: supported,
            }
        );

        // Upgraded past supported: compatibility reports `supported + 1`, while
        // `refresh_protocol_version` stays `<= supported` so flows do not fail.
        let upgraded = supported + 1;
        *session.hub.pump.platform_marks.lock().unwrap() = Some(Vec::new());
        *session.platform.protocol.test_fetch.lock().unwrap() = Some(verified_fetch(upgraded));
        let clamped_after = engine.block_on(session.refresh_protocol_version()).unwrap();
        assert!(clamped_after <= supported);
        assert_eq!(
            session.protocol_compatibility().unwrap(),
            ProtocolCompatibility {
                network_version: Some(upgraded),
                supported_version: supported,
            }
        );
        let marks = session
            .hub
            .pump
            .platform_marks
            .lock()
            .unwrap()
            .take()
            .unwrap();
        assert!(
            marks
                .iter()
                .any(|(w, changes)| *w == wallet && changes.contains(&PlatformChange::Startup))
        );

        // The same proved value again does not signal. Deleting the
        // `previous >= version` return would emit `Platform{Startup}` here.
        *session.hub.pump.platform_marks.lock().unwrap() = Some(Vec::new());
        let _ = engine.block_on(session.refresh_protocol_version()).unwrap();
        assert!(
            session
                .hub
                .pump
                .platform_marks
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .is_empty(),
            "refreshing the same version marked wallets"
        );

        // A lower proved value, from the epoch read and from the observer,
        // does not replace a higher one. `store` in place of `fetch_max`
        // would report `supported` here.
        *session.platform.protocol.test_fetch.lock().unwrap() = Some(verified_fetch(supported));
        let _ = engine.block_on(session.refresh_protocol_version()).unwrap();
        assert_eq!(
            session.protocol_compatibility().unwrap().network_version,
            Some(upgraded)
        );
        session.platform.protocol.record(supported, true);
        assert_eq!(
            session.protocol_compatibility().unwrap().network_version,
            Some(upgraded)
        );

        // A failed refresh keeps the proved version already learned.
        *session.platform.protocol.test_fetch.lock().unwrap() =
            Some(Arc::new(|| Err(PlatformError::Unavailable)));
        let _ = engine.block_on(session.refresh_protocol_version()).unwrap();
        assert_eq!(
            session.protocol_compatibility().unwrap().network_version,
            Some(upgraded)
        );

        engine
            .block_on(engine.close_network(DashNetwork::Regtest))
            .unwrap();
        assert_eq!(
            session.protocol_compatibility().unwrap_err().code(),
            "network_not_open"
        );

        // `no_platform`: `network_version` stays `None` even if `record` is called.
        let no_plat = engine
            .block_on(engine.open_network(
                DashNetwork::Regtest,
                SessionOptions {
                    no_platform: true,
                    ..opts
                },
            ))
            .unwrap();
        no_plat.platform.protocol.record(upgraded, true);
        assert_eq!(
            no_plat.protocol_compatibility().unwrap(),
            ProtocolCompatibility {
                network_version: None,
                supported_version: supported,
            }
        );
        engine
            .block_on(engine.close_network(DashNetwork::Regtest))
            .unwrap();
    }

    #[test]
    fn protocol_refresh_kick_observes_the_hook_and_close_joins_its_task() {
        let (_dir, engine, _sink) = test_engine();
        let upgraded = PlatformVersion::latest().protocol_version + 1;
        let session = engine
            .block_on(engine.open_network(
                DashNetwork::Regtest,
                SessionOptions {
                    dapi_addresses: vec!["https://127.0.0.1:1".into()],
                    quorum_url: Some("https://127.0.0.1:1".into()),
                    spv_peers: vec!["127.0.0.1:1".into()],
                    ..Default::default()
                },
            ))
            .unwrap();

        let calls = Arc::new(AtomicU32::new(0));
        let hook_calls = Arc::clone(&calls);
        *session.platform.protocol.test_fetch.lock().unwrap() = Some(Arc::new(move || {
            hook_calls.fetch_add(1, Ordering::SeqCst);
            trust::note_lookup(true);
            Ok(upgraded)
        }));

        let protocol = Arc::clone(&session.platform.protocol);
        protocol.kick(&session.rt);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while protocol.observed() != Some(upgraded) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(protocol.observed(), Some(upgraded));
        let seen = calls.load(Ordering::SeqCst);
        assert!(seen >= 1);

        engine
            .block_on(engine.close_network(DashNetwork::Regtest))
            .unwrap();
        assert!(
            guard(&protocol.tasks).is_none(),
            "protocol refresh task outlived close"
        );
        protocol.kick(&session.rt);
        assert_eq!(calls.load(Ordering::SeqCst), seen);
        assert_eq!(protocol.observed(), Some(upgraded));
    }

    /// #855: a version proved only under quorum keys SPV has not verified (the
    /// trusted service's, in Degraded mode) never raises `network_version` or
    /// marks a wallet, from the epoch read, a read mixing verified and
    /// unverified keys, a read with no lookup, or the SDK's observer; a failed
    /// refresh afterwards keeps the verified version, and only a verified read
    /// of the higher version reports it.
    #[test]
    fn a_version_proved_only_under_unverified_keys_is_never_reported() {
        let (_dir, engine, _sink) = test_engine();
        let supported = PlatformVersion::latest().protocol_version;
        let upgraded = supported + 1;
        let session = engine
            .block_on(engine.open_network(
                DashNetwork::Regtest,
                SessionOptions {
                    dapi_addresses: vec!["https://127.0.0.1:1".into()],
                    quorum_url: Some("https://127.0.0.1:1".into()),
                    spv_peers: vec!["127.0.0.1:1".into()],
                    ..Default::default()
                },
            ))
            .unwrap();
        let _wallet = engine
            .block_on(session.vault_op(|v| {
                v.create(None)?;
                Ok(())
            }))
            .and_then(|()| engine.block_on(session.create_wallet(12)))
            .unwrap();
        let reported = || session.protocol_compatibility().unwrap().network_version;
        let set =
            |f: TestProtocolFetch| *session.platform.protocol.test_fetch.lock().unwrap() = Some(f);
        let refresh = || engine.block_on(session.refresh_protocol_version()).unwrap();
        let unmarked = || {
            !session
                .hub
                .pump
                .platform_marks
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .iter()
                .any(|(_, changes)| changes.contains(&PlatformChange::Startup))
        };
        *session.hub.pump.platform_marks.lock().unwrap() = Some(Vec::new());

        set(fetch_under(&[false], upgraded));
        assert!(refresh() <= supported);
        assert_eq!(reported(), None, "the fallback key alone");
        set(fetch_under(&[], upgraded));
        refresh();
        assert_eq!(reported(), None, "no quorum lookup");
        session.platform.protocol.observer()(upgraded);
        assert_eq!(reported(), None, "the SDK's observer");
        assert!(unmarked(), "an unverified version marked a wallet");

        set(verified_fetch(supported));
        refresh();
        assert_eq!(reported(), Some(supported));
        *session.hub.pump.platform_marks.lock().unwrap() = Some(Vec::new());

        let mixed: [&'static [bool]; 3] = [&[false], &[true, false], &[false, true]];
        for keys in mixed {
            set(fetch_under(keys, upgraded));
            refresh();
            assert_eq!(reported(), Some(supported), "keys {keys:?}");
        }
        set(Arc::new(|| Err(PlatformError::Unavailable)));
        refresh();
        assert_eq!(
            reported(),
            Some(supported),
            "a failed refresh after the fallback's version"
        );
        assert!(unmarked(), "an unverified version marked a wallet");

        set(fetch_under(&[true, true], upgraded));
        refresh();
        assert_eq!(reported(), Some(upgraded));
        assert!(!unmarked(), "the verified upgrade marks the wallet");

        engine
            .block_on(engine.close_network(DashNetwork::Regtest))
            .unwrap();
    }
}
