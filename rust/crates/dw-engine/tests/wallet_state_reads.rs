//! #263: engine code reads a wallet through `wallet_read::info_of` /
//! `info_mut_of`, never `PlatformWallet::state`, `state_mut` or
//! `state_blocking`, which panic once the wallet left the manager (a
//! removal's lock hand-over). Test code may use them.

use std::fs;
use std::path::{Path, PathBuf};

/// Matched with whitespace removed, so a call split over lines is found.
const PANICKING: [&str; 5] = [
    ".state().await",
    ".state_mut().await",
    ".state_blocking()",
    ".state_mut_blocking()",
    "PlatformWallet::state",
];

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if path.is_dir() {
            if name != "tests" && !name.ends_with("_tests") {
                sources(&path, out);
            }
        } else if name.ends_with(".rs") && name != "tests.rs" && !name.ends_with("_tests.rs") {
            out.push(path);
        }
    }
}

/// `src` without its inline `#[cfg(test)] mod … { … }` blocks.
fn without_test_modules(src: &str) -> String {
    let mut out = String::new();
    let mut lines = src.lines().peekable();
    while let Some(line) = lines.next() {
        let next_opens_module = lines.peek().is_some_and(|next| {
            let next = next.trim_start();
            (next.starts_with("mod ") || next.contains(" mod ")) && next.trim_end().ends_with('{')
        });
        if line.trim() == "#[cfg(test)]" && next_opens_module {
            let mut depth = 0i32;
            for inner in lines.by_ref() {
                depth += inner.matches('{').count() as i32 - inner.matches('}').count() as i32;
                if depth <= 0 {
                    break;
                }
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

#[test]
fn engine_code_reads_wallets_fallibly() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    sources(&src, &mut files);
    assert!(files.len() > 50, "src/ not found");
    let mut hits = Vec::new();
    for file in files {
        let code = without_test_modules(&fs::read_to_string(&file).unwrap());
        let flat: String = (code.lines())
            .map(|line| line.split("//").next().unwrap_or(""))
            .flat_map(str::split_whitespace)
            .collect();
        for p in PANICKING {
            let n = flat.matches(p).count();
            if n > 0 {
                let rel = file.strip_prefix(&src).unwrap().display();
                hits.push(format!("{rel}: {n} × {p}"));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "a panicking wallet read (use wallet_read::info_of / info_mut_of):\n{}",
        hits.join("\n")
    );
}

#[test]
fn the_scan_skips_test_modules_only() {
    let src = "fn a() { w.state()\n    .await; }\n#[cfg(test)]\nmod tests {\n    fn b() {\n        w.state().await;\n    }\n}\nfn c() {}\n";
    let kept = without_test_modules(src);
    assert!(kept.contains("fn a()") && kept.contains("fn c()"));
    assert_eq!(kept.matches(".state()").count(), 1);
}
