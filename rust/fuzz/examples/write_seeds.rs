//! Writes each target's seeds to `<dir>/<target>/` (scripts/fuzz.sh).

fn main() {
    let dir = std::env::args_os()
        .nth(1)
        .map(std::path::PathBuf::from)
        .expect("usage: write_seeds <dir>");
    for name in dw_fuzz::TARGETS {
        let target = dir.join(name);
        std::fs::create_dir_all(&target).unwrap();
        let seeds = dw_fuzz::seeds::seeds(name);
        for (i, seed) in seeds.iter().enumerate() {
            std::fs::write(target.join(format!("seed-{i:03}")), seed).unwrap();
        }
        println!("{name}: {} seeds", seeds.len());
    }
}
