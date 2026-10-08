//! Platform reachability check (`dwcli platform-status`): one proof-verified
//! fetch of the DPNS system contract. It exercises DAPI, TLS (including a
//! custom CA), the quorum context and proof verification end to end, with no
//! wallet or identity involved.

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
    /// Fetches the DPNS data contract with proof verification.
    pub async fn platform_status(&self) -> Result<PlatformStatus, EngineError> {
        let _op = self.enter().await?;
        let manager = self.manager()?;
        let sdk = manager.sdk_arc();
        let id = SystemDataContract::DPNS.id();
        let contract = DataContract::fetch(&sdk, id)
            .await?
            .ok_or_else(|| EngineError::Sdk(format!("the DPNS contract {id} was not found")))?;
        Ok(PlatformStatus {
            dpns_contract_id: contract.id().to_string(Encoding::Base58),
            dpns_contract_version: contract.version(),
            dpns_contract_owner: contract.owner_id().to_string(Encoding::Base58),
            protocol_version: sdk.protocol_version_number(),
        })
    }
}
