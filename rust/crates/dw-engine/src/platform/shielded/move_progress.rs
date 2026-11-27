use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};

use crate::{
    DashNetwork, EngineEvent, EventSink, MoveProgressStatus, MoveStage, NetworkSession, WalletId,
};

use super::{ShieldOutcome, ShieldState, ShieldedError, ShieldedSpendState};

tokio::task_local! {
    static MOVE_PROGRESS: Option<Arc<MoveProgress>>;
}

struct MoveProgress {
    sink: Arc<dyn EventSink>,
    network: DashNetwork,
    wallet: WalletId,
    id: u32,
    stage: Mutex<MoveStage>,
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
    });
    progress.emit(MoveStage::CheckTransfer, MoveProgressStatus::Running);
    let result = MOVE_PROGRESS.scope(Some(Arc::clone(&progress)), call).await;
    let status = match &result {
        Ok(value) => outcome(value),
        Err(error) => MoveProgressStatus::Failed {
            code: error.code().into(),
        },
    };
    let stage = *progress
        .stage
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
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
