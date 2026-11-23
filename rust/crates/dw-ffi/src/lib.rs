//! UniFFI facade (`libdashwallet_core`). Type mapping only: every behaviour
//! lives in dw-engine. One module per domain under `api/`; add new domains by
//! appending to `api/mod.rs` (DESIGN-opus §5.3 rule 3).

uniffi::setup_scaffolding!("dashwallet_core");

mod api;

pub use api::*;
