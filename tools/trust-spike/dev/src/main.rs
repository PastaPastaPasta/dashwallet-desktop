//! E0-10a trust spike: the shared dash-spv probe (`../common/spv_probe.rs`)
//! built at rust-dashcore `dev`.

mod compat;
#[path = "../../common/spv_probe.rs"]
mod spv_probe;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("spv") => spv_probe::run(spv_probe::parse_spv_args(args, "dev")).await,
        _ => {
            eprintln!("usage: trust-spike-dev spv [options]");
            std::process::exit(2);
        }
    }
}
