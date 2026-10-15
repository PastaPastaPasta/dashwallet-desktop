//! The command manifest.
//!
//! Each domain module under `api/` declares its commands once, in one
//! [`commands!`] block. The block expands to:
//!
//! - the `#[tauri::command]` functions, each `async` and returning
//!   `Result<T, AppError>`;
//! - `NAMES` and `invoke`, the module's slice of the app's invoke handler
//!   (`api::invoke` dispatches on the command name);
//! - `specs()`, one [`CommandSpec`] per command: its arguments and result as
//!   ts-rs types, its doc comment and the engine facade methods it reaches.
//!
//! A block that starts with `#![cfg(..)]` compiles its functions and
//! registers them only under that condition; its specs are always there.
//!
//! `codegen` turns the specs into `apps/desktop/src/api/generated/commands.ts`,
//! so the TypeScript signature of every command comes from its Rust one, and
//! `coverage` checks the facade claims against the engine's sources.
//!
//! ```ignore
//! commands! {
//!     /// Balance buckets of one wallet; `null` until the scan reaches its birth height.
//!     #[facade("NetworkSession::balances")]
//!     fn balance(state, wallet_id: String) -> Option<Balances> {
//!         let id = parse_wallet_id(&wallet_id)?;
//!         Ok(state.session().await?.balances(&id)?.map(Balances::from))
//!     }
//! }
//! ```
//!
//! The first parameter names the `State<AppState>` binding; the others are
//! the IPC arguments, which the renderer passes in camelCase. Every command
//! is async: Tauri runs sync commands on the main thread, and no engine call
//! may block it.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use ts_rs::{Config, TS, TypeVisitor};

/// One command as the code generator and the coverage check see it.
pub struct CommandSpec {
    /// The IPC name (snake_case, as `invoke` sends it).
    pub name: &'static str,
    /// The Rust doc comment, one line per `///` line.
    pub docs: &'static [&'static str],
    /// Engine facade methods the command reaches, as `Type::method`.
    pub facade: &'static [&'static str],
    pub args: Vec<(&'static str, TypeSpec)>,
    /// The `Ok` type; every command rejects with `AppError`.
    pub ret: TypeSpec,
}

/// A Rust type crossing the IPC boundary, seen through ts-rs.
#[derive(Clone, Copy)]
pub struct TypeSpec {
    name: fn(&Config) -> String,
    export: fn(&Config, &mut Exports) -> Result<(), String>,
}

/// TypeScript identifier → file (relative to the generated directory) of
/// every named type a signature mentions.
pub type Imports = BTreeMap<String, PathBuf>;

/// What an export run has written so far.
#[derive(Default)]
pub struct Exports {
    /// The names signatures mention (`commands.ts` imports them).
    pub imports: Imports,
    /// Every named type reached, by output file, with its declaration and
    /// Rust name: ts-rs writes one file per TypeScript name, so two Rust
    /// types of one name would overwrite each other silently. (Distinct
    /// types may share a file when they declare the same thing, as ts-rs's
    /// `serde_json::Value` shadow does.)
    pub files: HashMap<PathBuf, (String, &'static str)>,
}

impl TypeSpec {
    pub fn of<T: TS + 'static>() -> Self {
        TypeSpec {
            name: T::name,
            export: export_with_imports::<T>,
        }
    }

    /// The type as written in a TypeScript signature (`Balances | null`).
    pub fn ts_name(&self, cfg: &Config) -> String {
        (self.name)(cfg)
    }

    /// Writes the type's declaration and its dependencies' into the
    /// config's directory, and records the names a signature must import.
    /// Fails when another Rust type already took one of the files.
    pub fn export(&self, cfg: &Config, exports: &mut Exports) -> Result<(), String> {
        (self.export)(cfg, exports)
    }
}

/// Exports `T` (and, through its generic arguments, the `X` of `Option<X>`
/// and `Vec<X>`) with all dependencies.
fn export_with_imports<T: TS + 'static>(cfg: &Config, exports: &mut Exports) -> Result<(), String> {
    struct Visit<'a> {
        cfg: &'a Config,
        exports: &'a mut Exports,
        /// Named types are recorded in `imports` only at the signature's level.
        top: bool,
        result: Result<(), String>,
    }
    impl TypeVisitor for Visit<'_> {
        fn visit<U: TS + 'static + ?Sized>(&mut self) {
            let Some(path) = U::output_path() else {
                // A wrapper (`Option`, `Vec`, `Box`): its arguments are what is named.
                U::visit_generics(self);
                return;
            };
            if self.top {
                self.exports
                    .imports
                    .insert(U::ident(self.cfg), path.clone());
            }
            let decl = U::decl(self.cfg);
            let rust = std::any::type_name::<U>();
            match self.exports.files.get(&path) {
                Some((seen, _)) if *seen == decl => return,
                Some((_, other)) => {
                    if self.result.is_ok() {
                        self.result =
                            Err(format!("{rust} and {other} are both {}", path.display()));
                    }
                    return;
                }
                None => {
                    self.exports.files.insert(path, (decl, rust));
                }
            }
            if self.result.is_ok() {
                self.result = U::export(self.cfg).map_err(|e| e.to_string());
            }
            let top = std::mem::replace(&mut self.top, false);
            U::visit_dependencies(self);
            self.top = top;
        }
    }
    let mut visit = Visit {
        cfg,
        exports,
        top: true,
        result: Ok(()),
    };
    visit.visit::<T>();
    visit.result
}

/// See the module documentation.
macro_rules! commands {
    // `#![cfg(..)]` first in the block compiles the functions and registers
    // them only under that condition (the self-test's, E0-13 D17). The specs
    // stay, so the generated bindings are the same in every build.
    (#![cfg($cfg:meta)] $($commands:tt)*) => {
        $crate::ipc::commands!(@cfg($cfg) $($commands)*);
    };
    (@cfg($cfg:meta) $(
        $(#[doc = $doc:literal])*
        #[facade($($facade:literal),* $(,)?)]
        fn $name:ident($state:ident $(, $arg:ident : $ty:ty)* $(,)?) -> $ret:ty $body:block
    )*) => {
        $(
            $(#[doc = $doc])*
            #[cfg($cfg)]
            #[tauri::command]
            pub async fn $name(
                $state: ::tauri::State<'_, $crate::state::AppState>,
                $($arg: $ty),*
            ) -> ::std::result::Result<$ret, $crate::error::AppError> {
                let _ = &$state;
                $body
            }
        )*

        /// The commands this module declares.
        pub(crate) const NAMES: &[&str] = &[$(stringify!($name)),*];

        /// Runs one of [`NAMES`]; `false` for any other command.
        #[cfg($cfg)]
        pub(crate) fn invoke<R: ::tauri::Runtime>(invoke: ::tauri::ipc::Invoke<R>) -> bool {
            let handler: fn(::tauri::ipc::Invoke<R>) -> bool =
                ::tauri::generate_handler![$($name),*];
            handler(invoke)
        }

        /// Without the condition none of [`NAMES`] is registered: `false`,
        /// Tauri's "command not found".
        #[cfg(not($cfg))]
        pub(crate) fn invoke<R: ::tauri::Runtime>(_: ::tauri::ipc::Invoke<R>) -> bool {
            false
        }

        /// What `codegen` and `coverage` read.
        pub(crate) fn specs() -> Vec<$crate::ipc::CommandSpec> {
            vec![$(
                $crate::ipc::CommandSpec {
                    name: stringify!($name),
                    docs: &[$($doc),*],
                    facade: &[$($facade),*],
                    args: vec![$((stringify!($arg), $crate::ipc::TypeSpec::of::<$ty>())),*],
                    ret: $crate::ipc::TypeSpec::of::<$ret>(),
                }
            ),*]
        }
    };
    ($($commands:tt)*) => {
        $crate::ipc::commands!(@cfg(all()) $($commands)*);
    };
}

pub(crate) use commands;
