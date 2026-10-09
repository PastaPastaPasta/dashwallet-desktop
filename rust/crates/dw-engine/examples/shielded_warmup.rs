//! Times the Orchard prover warm-up (E0-12, docs/research/shielded-cost.md).
//!
//! `CachedOrchardProver::warm_up` builds the Halo 2 proving key, which is all
//! the prover needs before its first proof. It touches no network. Each run of
//! this binary is a cold process, so it measures the first build; the second
//! call, served from the process-wide cache, is printed for contrast.
//!
//! ```sh
//! cargo run --release -p dw-engine --features shielded --example shielded_warmup
//! ```

use std::time::Instant;

use platform_wallet::wallet::shielded::CachedOrchardProver;

fn main() {
    let prover = CachedOrchardProver::new();
    println!("ready before warm_up: {}", prover.is_ready());

    let start = Instant::now();
    prover.warm_up();
    println!("warm_up_ms={:.0}", start.elapsed().as_secs_f64() * 1e3);
    println!("ready after warm_up: {}", prover.is_ready());

    let start = Instant::now();
    prover.warm_up();
    println!(
        "cached_warm_up_us={:.0}",
        start.elapsed().as_secs_f64() * 1e6
    );
}
