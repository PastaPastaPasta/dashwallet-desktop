//! Local RPC console of dashwallet-desktop (QT-145, DESIGN-opus §1.14).
//!
//! [`parse`] is dash-qt's command-line grammar and history redaction;
//! [`exec`] answers Core-named commands from dw-engine with Core's result
//! shapes, written by [`json`] as dashd prints them. Commands that need a
//! full node, or that this console does not offer yet, answer
//! [`ConsoleFailure::NotAvailable`] ("Not available in SPV mode.") instead
//! of an invented result.

pub mod exec;
mod m3_governance;
pub mod json;
pub mod parse;

pub use exec::{ConsoleContext, help_console_text};
pub use json::Json;
pub use parse::{Parsed, redact};

/// Why a console line failed.
#[derive(Debug, thiserror::Error)]
pub enum ConsoleFailure {
    /// dash-qt "Error: Invalid command line" / "Parse error".
    #[error("parse error: {0}")]
    Parse(String),
    /// A Core RPC error, shown as "message (code N)".
    #[error("{message} (code {code})")]
    Rpc { code: i32, message: String },
    /// The command needs a full node or is not offered.
    #[error("{0} is not available in SPV mode")]
    NotAvailable(String),
    /// The command spends, signs or reveals: the host authorizes `purpose`
    /// for `wallet` and runs the line again with the grant.
    #[error("authorization required")]
    AuthorizationRequired {
        purpose: dw_vault::GrantPurpose,
        wallet: Option<dw_engine::WalletId>,
    },
    /// A wallet command with no wallet selected.
    #[error("no wallet selected")]
    WalletRequired,
    /// An engine failure with no Core RPC equivalent.
    #[error("engine: {0}")]
    Engine(dw_engine::EngineError),
}

/// Core RPC help categories, in `help` order.
pub const CATEGORIES: [&str; 10] = [
    "Blockchain",
    "Control",
    "Evo",
    "Governance",
    "CoinJoin",
    "Network",
    "Rawtransactions",
    "Util",
    "Wallet",
    "Masternode",
];

/// One console command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandSpec {
    pub name: &'static str,
    pub category: &'static str,
    /// Arguments are redacted in the echo and history.
    pub sensitive: bool,
    /// Answered by this console; otherwise "Not available in SPV mode."
    pub available: bool,
    /// Argument synopsis for `help <command>`.
    pub usage: &'static str,
}

const fn c(
    name: &'static str,
    category: &'static str,
    available: bool,
    usage: &'static str,
) -> CommandSpec {
    CommandSpec {
        name,
        category,
        sensitive: false,
        available,
        usage,
    }
}

/// `c` for a command whose arguments are redacted.
const fn s(
    name: &'static str,
    category: &'static str,
    available: bool,
    usage: &'static str,
) -> CommandSpec {
    CommandSpec {
        sensitive: true,
        ..c(name, category, available, usage)
    }
}

/// dash-qt's history filter (`historyFilter` in rpcconsole.cpp).
pub const SENSITIVE: [&str; 9] = [
    "importprivkey",
    "importmulti",
    "sethdseed",
    "signmessagewithprivkey",
    "signrawtransactionwithkey",
    "upgradetohd",
    "walletpassphrase",
    "walletpassphrasechange",
    "encryptwallet",
];

/// Whether dash-qt redacts the arguments of `name` (case-insensitive).
pub fn is_sensitive(name: &str) -> bool {
    SENSITIVE.iter().any(|s| s.eq_ignore_ascii_case(name))
}

/// Every command, sorted by name.
pub const COMMANDS: &[CommandSpec] = &[
    c(
        "abandontransaction",
        "Wallet",
        true,
        "abandontransaction \"txid\"",
    ),
    c("abortrescan", "Wallet", true, "abortrescan"),
    c("addnode", "Network", false, ""),
    c("backupwallet", "Wallet", false, ""),
    c("bls", "Evo", false, ""),
    c("clearbanned", "Network", false, ""),
    c("coinjoin", "CoinJoin", false, ""),
    c("coinjoinsalt", "CoinJoin", false, ""),
    c("decoderawtransaction", "Rawtransactions", false, ""),
    c("disconnectnode", "Network", false, ""),
    c("dumpwallet", "Wallet", false, ""),
    s("encryptwallet", "Wallet", false, ""),
    c("estimatesmartfee", "Util", false, ""),
    c("getaddednodeinfo", "Network", false, ""),
    c("getbalance", "Wallet", true, "getbalance"),
    c("getbalances", "Wallet", true, "getbalances"),
    c("getbestblockhash", "Blockchain", false, ""),
    c("getbestchainlock", "Blockchain", true, "getbestchainlock"),
    c("getblock", "Blockchain", false, ""),
    c("getblockchaininfo", "Blockchain", false, ""),
    c("getblockcount", "Blockchain", true, "getblockcount"),
    c("getblockhash", "Blockchain", false, ""),
    c("getblockheader", "Blockchain", false, ""),
    c("getchaintips", "Blockchain", false, ""),
    c("getcoinjoininfo", "CoinJoin", false, ""),
    c("getconnectioncount", "Network", true, "getconnectioncount"),
    c("getgovernanceinfo", "Governance", true, "getgovernanceinfo"),
    c("getmemoryinfo", "Control", false, ""),
    c("getmempoolinfo", "Blockchain", false, ""),
    c("getnettotals", "Network", false, ""),
    c("getnetworkinfo", "Network", true, "getnetworkinfo"),
    c(
        "getnewaddress",
        "Wallet",
        true,
        "getnewaddress ( \"label\" )",
    ),
    c("getpeerinfo", "Network", true, "getpeerinfo"),
    c("getrawchangeaddress", "Wallet", false, ""),
    c("getrawmempool", "Blockchain", false, ""),
    c("getrawtransaction", "Rawtransactions", false, ""),
    c("getrpcinfo", "Control", false, ""),
    c(
        "getsuperblockbudget",
        "Governance",
        true,
        "getsuperblockbudget index",
    ),
    c("gettransaction", "Wallet", true, "gettransaction \"txid\""),
    c("gettxout", "Blockchain", false, ""),
    c(
        "getunconfirmedbalance",
        "Wallet",
        true,
        "getunconfirmedbalance",
    ),
    c("getwalletinfo", "Wallet", true, "getwalletinfo"),
    c(
        "gobject",
        "Governance",
        true,
        "gobject \"list|count|get|getcurrentvotes|vote-many\" ( ... )",
    ),
    c("help", "Control", true, "help ( \"command\" )"),
    c("help-console", "Control", true, "help-console"),
    s("importmulti", "Wallet", false, ""),
    s("importprivkey", "Wallet", false, ""),
    c("importwallet", "Wallet", false, ""),
    c("keypoolrefill", "Wallet", false, ""),
    c("listaddressbalances", "Wallet", false, ""),
    c("listbanned", "Network", false, ""),
    c("listdescriptors", "Wallet", false, ""),
    c("listlockunspent", "Wallet", true, "listlockunspent"),
    c(
        "listtransactions",
        "Wallet",
        true,
        "listtransactions ( \"label\" count skip )",
    ),
    c("listunspent", "Wallet", true, "listunspent ( minconf )"),
    c("listwallets", "Wallet", true, "listwallets"),
    c("loadwallet", "Wallet", true, "loadwallet \"filename\""),
    c(
        "lockunspent",
        "Wallet",
        true,
        "lockunspent unlock ( [{\"txid\":\"hex\",\"vout\":n},...] )",
    ),
    c("logging", "Control", false, ""),
    c("masternode", "Masternode", false, ""),
    c("masternodelist", "Masternode", false, ""),
    c("ping", "Network", false, ""),
    c("protx", "Evo", false, ""),
    c("quorum", "Evo", false, ""),
    c(
        "rescanblockchain",
        "Wallet",
        true,
        "rescanblockchain ( start_height )",
    ),
    c("sendmany", "Wallet", false, ""),
    c("sendrawtransaction", "Rawtransactions", false, ""),
    c(
        "sendtoaddress",
        "Wallet",
        true,
        "sendtoaddress \"address\" amount ( \"comment\" \"comment_to\" subtractfeefromamount )",
    ),
    c("setban", "Network", false, ""),
    s("sethdseed", "Wallet", false, ""),
    c("setlabel", "Wallet", true, "setlabel \"address\" \"label\""),
    c("setnetworkactive", "Network", false, ""),
    c(
        "signmessage",
        "Wallet",
        true,
        "signmessage \"address\" \"message\"",
    ),
    s("signmessagewithprivkey", "Util", false, ""),
    s("signrawtransactionwithkey", "Rawtransactions", false, ""),
    c("stop", "Control", false, ""),
    c(
        "unloadwallet",
        "Wallet",
        true,
        "unloadwallet ( \"wallet_name\" )",
    ),
    s("upgradetohd", "Wallet", false, ""),
    c("uptime", "Control", true, "uptime"),
    c(
        "validateaddress",
        "Util",
        true,
        "validateaddress \"address\"",
    ),
    c("verifychainlock", "Blockchain", false, ""),
    c("verifyislock", "Blockchain", false, ""),
    c(
        "verifymessage",
        "Util",
        true,
        "verifymessage \"address\" \"signature\" \"message\"",
    ),
    c("voteraw", "Governance", false, ""),
    c("walletlock", "Wallet", true, "walletlock"),
    s(
        "walletpassphrase",
        "Wallet",
        true,
        "walletpassphrase \"passphrase\" timeout",
    ),
    s("walletpassphrasechange", "Wallet", false, ""),
];

/// The command named `name` (case-sensitive, as Core).
pub fn command(name: &str) -> Option<&'static CommandSpec> {
    COMMANDS.iter().find(|c| c.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_qt_145_command_table_is_sorted_and_marks_dash_qt_sensitive_commands() {
        let names: Vec<&str> = COMMANDS.iter().map(|c| c.name).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted);
        assert!(COMMANDS.len() >= 80, "{}", COMMANDS.len());
        for c in COMMANDS {
            assert_eq!(c.sensitive, is_sensitive(c.name), "{}", c.name);
            assert!(CATEGORIES.contains(&c.category), "{}", c.name);
            assert_eq!(c.available, !c.usage.is_empty(), "{}", c.name);
        }
        assert_eq!(SENSITIVE.len(), 9);
        assert!(is_sensitive("WALLETPASSPHRASE"));
    }
}
