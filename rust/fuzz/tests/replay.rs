//! Every target's seeds, and every input saved under `regressions/<target>/`
//! (a minimised fuzz finding, kept after its fix), through the harness on
//! stable. The engine targets run with `--features engine`.

use std::path::Path;

fn runs(name: &str) -> bool {
    cfg!(feature = "engine") || !dw_fuzz::ENGINE_TARGETS.contains(&name)
}

#[test]
fn seeds_run_clean() {
    for name in dw_fuzz::TARGETS.into_iter().filter(|n| runs(n)) {
        let seeds = dw_fuzz::seeds::seeds(name);
        assert!(seeds.len() >= 4, "{name}: {} seeds", seeds.len());
        for seed in seeds {
            assert!(dw_fuzz::run(name, &seed), "{name}");
        }
    }
}

#[test]
fn regressions_run_clean() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("regressions");
    for name in dw_fuzz::TARGETS.into_iter().filter(|n| runs(n)) {
        let Ok(dir) = std::fs::read_dir(root.join(name)) else {
            continue;
        };
        for entry in dir {
            let path = entry.unwrap().path();
            eprintln!("{}", path.display());
            assert!(dw_fuzz::run(name, &std::fs::read(&path).unwrap()), "{name}");
        }
    }
}
