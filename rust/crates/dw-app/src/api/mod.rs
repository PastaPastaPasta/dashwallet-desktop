//! Every command the renderer can call, one module per domain.
//!
//! Each module declares its commands in one `ipc::commands!` block, which
//! also gives it `NAMES`, `invoke` and `specs`, and lists in `EXCLUDED` the
//! facade methods it deliberately does not bind, with the reason
//! (`coverage` checks that every facade method is in exactly one of the
//! two). Its commands are granted in `capabilities/<module>.json`, except
//! the self-test's: `selftest` is compiled, registered and granted only with
//! the `selftest` feature (`capabilities-selftest/selftest.json`, E0-13 D17).

use tauri::Runtime;
use tauri::ipc::Invoke;

use crate::ipc::CommandSpec;

macro_rules! domains {
    ($($domain:ident),+ $(,)?) => {
        $(pub mod $domain;)+

        /// The domains (each has a capability file named after it once it declares commands).
        #[cfg(test)]
        pub(crate) const DOMAINS: &[&str] = &[$(stringify!($domain)),+];

        /// The app's invoke handler: runs the command in the domain that
        /// declares it. `false` (Tauri's "command not found") for any other.
        pub(crate) fn invoke<R: Runtime>(invoke: Invoke<R>) -> bool {
            $(
                if $domain::NAMES.contains(&invoke.message.command()) {
                    return $domain::invoke(invoke);
                }
            )+
            false
        }

        /// Every command, with the domain that declares it.
        pub(crate) fn specs() -> Vec<(&'static str, CommandSpec)> {
            let mut all = Vec::new();
            $(all.extend($domain::specs().into_iter().map(|s| (stringify!($domain), s)));)+
            all
        }

        /// Facade methods left unbound, with the domain and the reason.
        pub(crate) fn excluded() -> Vec<(&'static str, &'static str, &'static str)> {
            let mut all = Vec::new();
            $(all.extend($domain::EXCLUDED.iter().map(|(m, why)| (stringify!($domain), *m, *why)));)+
            all
        }
    };
}

domains!(
    app, network, vault, wallets, chain, history, backup, send, coinjoin, identity, social,
    console, units, selftest,
);

/// Registered with the handler but granted by no capability: the U6 probe
/// that the ACL refuses a command no capability lists. Never add it to
/// `capabilities/`.
pub const UNGRANTED: &[&str] = &["probe_unlisted"];

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::Path;

    use super::*;

    fn capabilities(dir: &str) -> Vec<(String, serde_json::Value)> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(dir);
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .collect();
        files.sort();
        files
            .into_iter()
            .map(|p| {
                let stem = p.file_stem().unwrap().to_string_lossy().into_owned();
                let json = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
                (stem, json)
            })
            .collect()
    }

    /// Each domain's commands are granted in `capabilities/<domain>.json`
    /// (the self-test's in `capabilities-selftest/selftest.json`), to the
    /// main window only, and nothing else is granted: no command another
    /// file declares, no plugin beyond event listening. With `build.rs`
    /// deriving the permissions from the same files, a command without a
    /// grant cannot be called and a grant without a command does not build.
    #[test]
    fn every_command_is_granted_in_its_domain_file() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
        let listed: BTreeSet<String> = conf["app"]["security"]["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        let caps = capabilities("capabilities");
        let mut files: BTreeSet<String> = caps.iter().map(|(n, _)| n.clone()).collect();
        assert_eq!(
            listed, files,
            "tauri.conf.json must list every capability file"
        );
        let selftest = capabilities("capabilities-selftest");
        assert_eq!(
            selftest.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            ["selftest"]
        );
        files.insert("selftest".into());
        let specs = specs();
        for (name, cap) in caps.iter().chain(&selftest) {
            assert_eq!(cap["identifier"], name.as_str());
            assert_eq!(cap["windows"], serde_json::json!(["main"]), "{name}");
            assert!(
                DOMAINS.contains(&name.as_str()),
                "capabilities/{name}.json names no api module"
            );
            let mut granted = BTreeSet::new();
            for p in cap["permissions"].as_array().unwrap() {
                let p = p.as_str().expect("permissions are plain strings");
                match p.strip_prefix("allow-") {
                    Some(cmd) => {
                        granted.insert(cmd.replace('-', "_"));
                    }
                    None => assert!(
                        name == "app"
                            && ["core:event:allow-listen", "core:event:allow-unlisten"]
                                .contains(&p),
                        "{name} grants {p}"
                    ),
                }
            }
            let declared: BTreeSet<String> = specs
                .iter()
                .filter(|(d, s)| d == name && !UNGRANTED.contains(&s.name))
                .map(|(_, s)| s.name.to_string())
                .collect();
            assert_eq!(granted, declared, "capabilities/{name}.json");
        }
        for (domain, spec) in &specs {
            if !UNGRANTED.contains(&spec.name) {
                assert!(
                    files.contains(*domain),
                    "{} has no capability file",
                    spec.name
                );
            }
        }
    }

    /// What `generate_context!` embeds is tauri-build's copy of
    /// `capabilities/` (`OUT_DIR/capabilities.json`): the same capabilities
    /// in every build, and none grants a self-test command. Only `run` adds
    /// `selftest.json`, in a build with the feature (E0-13 D17).
    #[test]
    fn the_embedded_capabilities_grant_no_selftest_command() {
        let embedded: serde_json::Value =
            serde_json::from_str(include_str!(concat!(env!("OUT_DIR"), "/capabilities.json")))
                .unwrap();
        let ids: BTreeSet<String> = embedded.as_object().unwrap().keys().cloned().collect();
        let files: BTreeSet<String> = capabilities("capabilities")
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert_eq!(ids, files);
        let text = embedded.to_string();
        for name in selftest::NAMES {
            let permission = format!("allow-{}", name.replace('_', "-"));
            assert!(!text.contains(&permission), "{permission}");
        }
    }

    #[test]
    fn command_names_are_unique_and_dispatched() {
        let specs = specs();
        let mut seen = BTreeSet::new();
        for (_, s) in &specs {
            assert!(seen.insert(s.name), "{} declared twice", s.name);
        }
        assert!(seen.is_superset(&UNGRANTED.iter().copied().collect()));
    }
}
