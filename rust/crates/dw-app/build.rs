//! Each command gets an `allow-<name>` permission. The commands are the ones
//! `capabilities/*.json` grant, plus the never-granted U6 probe; the test
//! `api::tests::every_command_is_granted` checks that this is exactly the
//! set `src/api` declares, so a command cannot exist without a grant.

use std::path::Path;

/// Registered but granted by no capability (`api::UNGRANTED`).
const UNGRANTED: &[&str] = &["probe_unlisted"];

fn granted_commands(dir: &Path) -> Vec<String> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .expect("capabilities/")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    let mut commands = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file).expect("capability file");
        // App permissions are the bare `"allow-<name>"` strings; plugin
        // permissions carry a `plugin:` prefix.
        for piece in text.split("\"allow-").skip(1) {
            let name = piece.split('"').next().unwrap_or_default();
            commands.push(name.replace('-', "_"));
        }
    }
    commands
}

fn main() {
    println!("cargo:rerun-if-changed=capabilities");
    let mut commands = granted_commands(Path::new("capabilities"));
    commands.extend(UNGRANTED.iter().map(|s| s.to_string()));
    let commands: Vec<&'static str> = commands
        .into_iter()
        .map(|s| &*Box::leak(s.into_boxed_str()))
        .collect();
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(Box::leak(commands.into_boxed_slice())),
    ))
    .expect("tauri build script");
}
