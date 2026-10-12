//! The local RPC console (QT-145, Tools → Console): dash-qt's console
//! grammar (`dw-console`) over engine calls, with Core's command names.
//! Commands that need a full node answer "not available" (DESIGN-opus
//! §1.14). Failures carry the `console.*` codes.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use zeroize::Zeroizing;

use crate::ipc::commands;
use crate::state::parse_wallet_id;

pub(crate) const EXCLUDED: &[(&str, &str)] = &[];

/// One console command, for `help` and tab completion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ConsoleCommand {
    pub name: String,
    /// Core's help category ("Wallet", "Network", "Control", …).
    pub category: String,
    /// dash-qt redacts its arguments in the echo and the history.
    pub sensitive: bool,
    /// Answered by this console; `false`: "Not available in SPV mode."
    pub available: bool,
    /// The argument synopsis `help <command>` shows.
    pub usage: String,
}

impl From<&dw_console::CommandSpec> for ConsoleCommand {
    fn from(c: &dw_console::CommandSpec) -> Self {
        let dw_console::CommandSpec {
            name,
            category,
            sensitive,
            available,
            usage,
        } = *c;
        ConsoleCommand {
            name: name.into(),
            category: category.into(),
            sensitive,
            available,
            usage: usage.into(),
        }
    }
}

/// A command's result as dashd prints it: JSON pretty-printed with a
/// 2-space indent, or plain text for string results (`help`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ConsoleOutput {
    pub text: String,
    pub is_json: bool,
    /// The line with sensitive arguments replaced by `(…)`: the only form
    /// the console may echo or keep in its history.
    pub filtered: String,
}

commands! {
    /// Every console command, sorted by name.
    #[facade()]
    fn console_commands(state) -> Vec<ConsoleCommand> {
        Ok(dw_console::COMMANDS.iter().map(Into::into).collect())
    }

    /// dash-qt's history redaction: the arguments of a sensitive command
    /// (`walletpassphrase`, `importprivkey`, …) become `(…)`.
    #[facade()]
    fn console_redact(state, line: String) -> String {
        let line = Zeroizing::new(line);
        Ok(dw_console::redact(&line)?)
    }

    /// Parses and runs one console line (nested calls, `[key]` indexing and
    /// quoting as dash-qt) against `walletId` (`null`: no wallet). `grantId`
    /// answers an earlier `console.authorization_required` (its `data`
    /// names the purpose and wallet) and is spent by the first command that
    /// needs one.
    #[facade("NetworkSession::run_on_engine")]
    fn console_execute(
        state,
        wallet_id: Option<String>,
        line: String,
        grant_id: Option<String>,
    ) -> ConsoleOutput {
        let line = Zeroizing::new(line);
        let wallet = wallet_id.as_deref().map(parse_wallet_id).transpose()?;
        let session = state.session().await?;
        let mut ctx = dw_console::ConsoleContext {
            session: Arc::clone(&session),
            wallet,
            grant_id,
        };
        // The console's commands spawn tasks and use tokio timers: they run
        // on the engine runtime.
        let out = session
            .run_on_engine(async move { ctx.run(&line).await })
            .await??;
        let dw_console::Parsed {
            result,
            is_json,
            filtered,
        } = out;
        Ok(ConsoleOutput {
            text: result,
            is_json,
            filtered,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_list_is_complete_and_sorted() {
        let all: Vec<ConsoleCommand> = dw_console::COMMANDS.iter().map(Into::into).collect();
        assert_eq!(all.len(), dw_console::COMMANDS.len());
        assert!(all.windows(2).all(|w| w[0].name < w[1].name));
        let pass = all.iter().find(|c| c.name == "walletpassphrase").unwrap();
        assert!(pass.sensitive);
    }
}
