//! The command tests run as "a command the engine refuses", shared by the
//! unit tests and, through `#[path]`, the binary tests. It needs no wallet
//! and no network. Until DP5-02 it was a stubbed call; every call that
//! needs no wallet has a body now, so it is a refused argument.
#![allow(dead_code)]

/// The command's arguments.
pub const STUB_ARGS: &[&str] = &["invite", "status", "l"];

/// Its error's code.
pub const STUB_CODE: &str = "invalid_argument";

/// Its error's `params.detail`, which its message quotes.
pub const STUB_DETAIL: &str = "not a stashed link id";

/// A session request running it, with `id` as raw JSON.
pub fn stub_request(id: &str) -> String {
    format!(
        r#"{{"args":{},"id":{id}}}"#,
        serde_json::to_string(STUB_ARGS).unwrap()
    )
}
