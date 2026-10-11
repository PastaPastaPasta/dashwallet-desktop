//! Chain and sync: SPV, peers and tools.

use crate::dto::SyncStatus;
use crate::ipc::commands;

pub(crate) const EXCLUDED: &[(&str, &str)] = &[];

commands! {
    #[facade("NetworkSession::sync_snapshot")]
    fn sync_status(state) -> SyncStatus {
        Ok(state.session().await?.sync_snapshot()?.into())
    }
}
