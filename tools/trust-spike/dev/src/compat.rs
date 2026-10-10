//! rust-dashcore `dev`: `run()` spawns the sync and returns, `stop()` returns
//! nothing, and the engine is crate-private, so only the newest list is visible.

use std::sync::Arc;

use dashcore::sml::llmq_type::network::NetworkLLMQExt;
use serde_json::{Value, json};
use tokio::task::JoinHandle;

use crate::spv_probe::{Client, quorum_row};

pub fn start(client: Arc<Client>) -> JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(error) = client.run().await {
            eprintln!("dash-spv run failed: {error}");
        }
    })
}

pub async fn stop(client: &Client, runner: JoinHandle<()>) {
    client.stop().await;
    let _ = tokio::time::timeout(std::time::Duration::from_secs(20), runner).await;
}

/// The Platform quorums of the newest list (the engine is not public at dev).
pub async fn engine_stats(client: &Client) -> Value {
    let Some(list) = client.latest_masternode_list().await else {
        return Value::Null;
    };
    let platform_type = client.network().await.platform_type();
    let platform: Vec<Value> = list
        .quorums
        .get(&platform_type)
        .into_iter()
        .flat_map(|quorums| quorums.values())
        .map(|quorum| quorum_row(quorum))
        .collect();
    json!({"newest": list.known_height, "platform_type": platform_type as u8,
           "platform_quorums_newest": platform})
}
