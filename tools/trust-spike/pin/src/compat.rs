//! rust-dashcore `40268cc0`: `run()` blocks until `stop()`, and the
//! masternode list engine is public.

use std::sync::Arc;

use dashcore::sml::llmq_type::network::NetworkLLMQExt;
use serde_json::{Value, json};
use tokio::task::JoinHandle;

use crate::spv_probe::{Client, quorum_row};

pub fn start(client: Arc<Client>) -> JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(error) = client.run().await {
            eprintln!("dash-spv run ended: {error}");
        }
    })
}

pub async fn stop(client: &Client, runner: JoinHandle<()>) {
    let _ = client.stop().await;
    let _ = tokio::time::timeout(std::time::Duration::from_secs(20), runner).await;
}

/// The lists the engine holds and the Platform quorums of its newest list.
pub async fn engine_stats(client: &Client) -> Value {
    let Ok(engine) = client.masternode_list_engine() else {
        return Value::Null;
    };
    let engine = engine.read().await;
    let lists = &engine.masternode_lists;
    let platform_type = engine.network.platform_type();
    let platform: Vec<Value> = lists
        .values()
        .next_back()
        .and_then(|list| list.quorums.get(&platform_type))
        .into_iter()
        .flat_map(|quorums| quorums.values())
        .map(quorum_row)
        .collect();
    json!({
        "lists": lists.len(),
        "oldest": lists.keys().next(),
        "newest": lists.keys().next_back(),
        "platform_type": platform_type as u8,
        "platform_quorums_newest": platform,
    })
}
