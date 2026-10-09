//! The command tests run as "a command whose call is still a stub", shared
//! by the unit tests and, through `#[path]`, the binary tests. It needs no
//! wallet. The DP task that implements its call picks another stub here,
//! once for every test.
#![allow(dead_code)]

/// The command's arguments.
pub const STUB_ARGS: &[&str] = &["invite", "pending"];

/// The facade call it reaches, which its `platform.not_implemented` error
/// names.
pub const STUB_CALL: &str = "NetworkSession.pending_invitations";

/// A session request running it, with `id` as raw JSON.
pub fn stub_request(id: &str) -> String {
    format!(
        r#"{{"args":{},"id":{id}}}"#,
        serde_json::to_string(STUB_ARGS).unwrap()
    )
}
