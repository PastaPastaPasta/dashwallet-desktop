//! Platform reachability check (`dwcli platform-status`): one proof-verified
//! fetch of the DPNS system contract. It exercises DAPI, TLS (including a
//! custom CA), the quorum context and proof verification end to end, with no
//! wallet or identity involved.

use std::sync::Arc;

use dash_sdk::platform::Fetch;
use dpp::data_contract::accessors::v0::DataContractV0Getters;
use dpp::platform_value::string_encoding::Encoding;
use dpp::prelude::DataContract;
use dpp::system_data_contracts::SystemDataContract;

use crate::{EngineError, NetworkSession};

/// What the DPNS fetch proved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformStatus {
    /// Base58 id of the DPNS contract.
    pub dpns_contract_id: String,
    /// The contract's own version counter.
    pub dpns_contract_version: u32,
    /// Base58 owner identity of the contract.
    pub dpns_contract_owner: String,
    /// Protocol version the SDK settled on after the fetch (it starts at the
    /// per-network floor, or `initial_protocol_version`, and ratchets up).
    pub protocol_version: u32,
}

impl NetworkSession {
    /// Fetches the DPNS data contract with proof verification. Runs on the
    /// engine runtime, so proof descent gets its 8 MiB stack whatever thread
    /// the caller polls from.
    pub async fn platform_status(self: &Arc<Self>) -> Result<PlatformStatus, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let sdk = this.manager()?.sdk_arc();
            let id = SystemDataContract::DPNS.id();
            let contract = DataContract::fetch(&sdk, id).await?.ok_or_else(|| {
                EngineError::Sdk(format!(
                    "the DPNS contract {} was not found",
                    id.to_string(Encoding::Base58)
                ))
            })?;
            Ok(PlatformStatus {
                dpns_contract_id: contract.id().to_string(Encoding::Base58),
                dpns_contract_version: contract.version(),
                dpns_contract_owner: contract.owner_id().to_string(Encoding::Base58),
                protocol_version: sdk.protocol_version_number(),
            })
        })
        .await
    }
}
