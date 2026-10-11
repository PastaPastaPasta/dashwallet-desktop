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

use std::collections::BTreeMap;
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
    export: fn(&Config, &mut Imports) -> Result<(), ts_rs::ExportError>,
}

/// TypeScript identifier → file (relative to the generated directory) of
/// every named type a signature mentions.
pub type Imports = BTreeMap<String, PathBuf>;

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
    pub fn export(&self, cfg: &Config, imports: &mut Imports) -> Result<(), ts_rs::ExportError> {
        (self.export)(cfg, imports)
    }
}

/// Exports `T` (and, through its generic arguments, the `X` of `Option<X>`
/// and `Vec<X>`) with all dependencies.
fn export_with_imports<T: TS + 'static>(
    cfg: &Config,
    imports: &mut Imports,
) -> Result<(), ts_rs::ExportError> {
    struct Visit<'a> {
        cfg: &'a Config,
        imports: &'a mut Imports,
        result: Result<(), ts_rs::ExportError>,
    }
    impl TypeVisitor for Visit<'_> {
        fn visit<U: TS + 'static + ?Sized>(&mut self) {
            if let Some(path) = U::output_path() {
                self.imports.insert(U::ident(self.cfg), path);
                if self.result.is_ok() {
                    self.result = U::export_all(self.cfg);
                }
            } else {
                // A wrapper (`Option`, `Vec`, `Box`): its arguments are what is named.
                U::visit_generics(self);
            }
        }
    }
    let mut visit = Visit {
        cfg,
        imports,
        result: Ok(()),
    };
    visit.visit::<T>();
    visit.result
}

/// See the module documentation.
macro_rules! commands {
    ($(
        $(#[doc = $doc:literal])*
        #[facade($($facade:literal),* $(,)?)]
        fn $name:ident($state:ident $(, $arg:ident : $ty:ty)* $(,)?) -> $ret:ty $body:block
    )*) => {
        $(
            $(#[doc = $doc])*
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
        pub(crate) fn invoke<R: ::tauri::Runtime>(invoke: ::tauri::ipc::Invoke<R>) -> bool {
            let handler: fn(::tauri::ipc::Invoke<R>) -> bool =
                ::tauri::generate_handler![$($name),*];
            handler(invoke)
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
}

pub(crate) use commands;
