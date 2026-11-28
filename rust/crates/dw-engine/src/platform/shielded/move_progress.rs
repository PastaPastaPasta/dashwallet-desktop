use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use crate::{
    DashNetwork, EngineEvent, EventSink, MoveProgressStatus, MoveStage, NetworkSession, WalletId,
};

use super::{ShieldOutcome, ShieldState, ShieldedError, ShieldedSpendState};
use crate::platform::PlatformError;

tokio::task_local! {
    static MOVE_PROGRESS: Option<Arc<MoveProgress>>;
}

struct MoveProgress {
    sink: Arc<dyn EventSink>,
    network: DashNetwork,
    wallet: WalletId,
    id: u32,
    stage: Mutex<MoveStage>,
    refreshed: AtomicBool,
}

impl MoveProgress {
    fn emit(&self, stage: MoveStage, status: MoveProgressStatus) {
        self.sink.emit(EngineEvent::MoveProgress {
            network: self.network.clone(),
            wallet_id: self.wallet,
            progress_id: self.id,
            stage,
            status,
        });
    }
}

pub(in crate::platform) fn inherit<F: Future>(call: F) -> impl Future<Output = F::Output> {
    let progress = MOVE_PROGRESS.try_with(Option::clone).ok().flatten();
    MOVE_PROGRESS.scope(progress, call)
}

pub(in crate::platform) fn balance_refreshed() {
    let _ = MOVE_PROGRESS.try_with(|progress| {
        if let Some(progress) = progress {
            progress.refreshed.store(true, Ordering::Relaxed);
        }
    });
}

pub(in crate::platform) fn advance(stage: MoveStage) {
    let _ = MOVE_PROGRESS.try_with(|progress| {
        if let Some(progress) = progress {
            let mut current = progress
                .stage
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if stage > *current {
                *current = stage;
                drop(current);
                progress.emit(stage, MoveProgressStatus::Running);
            }
        }
    });
}

pub(in crate::platform) async fn report<T>(
    session: &NetworkSession,
    wallet: WalletId,
    id: Option<u32>,
    call: impl Future<Output = Result<T, ShieldedError>>,
    outcome: impl FnOnce(&T) -> MoveProgressStatus,
) -> Result<T, ShieldedError> {
    let Some(id) = id else { return call.await };
    let progress = Arc::new(MoveProgress {
        sink: Arc::clone(&session.sink),
        network: session.network().clone(),
        wallet,
        id,
        stage: Mutex::new(MoveStage::CheckTransfer),
        refreshed: AtomicBool::new(false),
    });
    progress.emit(MoveStage::CheckTransfer, MoveProgressStatus::Running);
    let result = MOVE_PROGRESS.scope(Some(Arc::clone(&progress)), call).await;
    let stage = *progress
        .stage
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let status = match &result {
        Ok(_)
            if stage == MoveStage::RefreshShielded
                && !progress.refreshed.load(Ordering::Relaxed) =>
        {
            MoveProgressStatus::Pending
        }
        Ok(value) => outcome(value),
        Err(ShieldedError::Platform(PlatformError::BroadcastUnknown { .. })) => {
            MoveProgressStatus::Pending
        }
        Err(error) => MoveProgressStatus::Failed {
            code: error.code().into(),
        },
    };
    progress.emit(stage, status);
    result
}

pub(in crate::platform) fn shield_outcome(outcome: &ShieldOutcome) -> MoveProgressStatus {
    match &outcome.state {
        ShieldState::Applied { .. } => MoveProgressStatus::Done,
        ShieldState::ProofWaiting | ShieldState::ConsumptionUnknown => MoveProgressStatus::Pending,
        ShieldState::NotApplied { error } => MoveProgressStatus::Failed {
            code: error.clone(),
        },
    }
}

pub(in crate::platform) fn spend_outcome(state: ShieldedSpendState) -> MoveProgressStatus {
    match state {
        ShieldedSpendState::Done => MoveProgressStatus::Done,
        ShieldedSpendState::Pending => MoveProgressStatus::Pending,
    }
}
