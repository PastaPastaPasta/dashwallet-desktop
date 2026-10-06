//! Core-named console commands over dw-engine (QT-145). Each command
//! answers with the result shape Dash Core's RPC has, limited to the fields
//! an SPV wallet knows: a field the engine cannot know is left out, never
//! filled with a guess. Errors use Core's codes and messages where Core has
//! the same failure (`RPC_WALLET_NOT_FOUND`, `RPC_INVALID_ADDRESS_OR_KEY`,
//! …). Commands that spend or sign stop with `AuthorizationRequired` until
//! the host supplies a grant.

use std::future::Future;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use dashcore::Address;
use dw_engine::{
    BookPurpose, CoinFilter, EngineError, HistoryFilter, HistoryQuery, HistorySort, NetworkSession,
    Recipient, RescanFrom, TxRecord, TxStatusKind, TxType, WalletId,
};
use dw_vault::{GrantPurpose, UnlockScope, VaultError};
use zeroize::Zeroizing;

use crate::json::Json;
use crate::parse::{Executor, Parsed, execute};
use crate::{CATEGORIES, COMMANDS, ConsoleFailure, command};

/// Core RPC error codes used here (src/rpc/protocol.h).
mod code {
    pub const MISC_ERROR: i32 = -1;
    pub const TYPE_ERROR: i32 = -3;
    pub const WALLET_ERROR: i32 = -4;
    pub const INVALID_ADDRESS_OR_KEY: i32 = -5;
    pub const WALLET_INSUFFICIENT_FUNDS: i32 = -6;
    pub const INVALID_PARAMETER: i32 = -8;
    pub const WALLET_UNLOCK_NEEDED: i32 = -13;
    pub const WALLET_PASSPHRASE_INCORRECT: i32 = -14;
    pub const WALLET_NOT_FOUND: i32 = -18;
    pub const WALLET_ALREADY_LOADED: i32 = -35;
    pub const INTERNAL_ERROR: i32 = -32603;
}

fn rpc(code: i32, message: impl Into<String>) -> ConsoleFailure {
    ConsoleFailure::Rpc {
        code,
        message: message.into(),
    }
}

/// dash-qt's `help-console` text.
pub fn help_console_text() -> &'static str {
    "\nThis console accepts RPC commands using the standard syntax.\n   example:    getblockhash 0\n\nThis console can also accept RPC commands using the parenthesized syntax.\n   example:    getblockhash(0)\n\nCommands may be nested when specified with the parenthesized syntax.\n   example:    getblock(getblockhash(0) 1)\n\nA space or a comma can be used to delimit arguments for either syntax.\n   example:    getblockhash 0\n               getblockhash,0\n\nNamed results can be queried with a non-quoted key string in brackets using the parenthesized syntax.\n   example:    getblock(getblockhash(0) 1)[tx]\n\nResults without keys can be queried with an integer in brackets using the parenthesized syntax.\n   example:    getblock(getblockhash(0),1)[tx][0]\n\n"
}

/// Maps an engine failure to Core's RPC error for the same condition, or
/// passes it on.
fn engine_failure(e: EngineError) -> ConsoleFailure {
    match e {
        EngineError::WalletNotFound(_) => rpc(
            code::WALLET_NOT_FOUND,
            "Requested wallet does not exist or is not loaded",
        ),
        EngineError::TxNotFound(_) => rpc(
            code::INVALID_ADDRESS_OR_KEY,
            "Invalid or non-wallet transaction id",
        ),
        EngineError::InvalidAddress(_) => rpc(code::INVALID_ADDRESS_OR_KEY, "Invalid Dash address"),
        EngineError::AddressNotMine(_) | EngineError::AddressNoKey(_) => {
            rpc(code::WALLET_ERROR, "Private key not available")
        }
        EngineError::TxActionRefused(_) => rpc(
            code::INVALID_ADDRESS_OR_KEY,
            "Transaction not eligible for abandonment",
        ),
        EngineError::RescanInProgress => rpc(
            code::WALLET_ERROR,
            "Wallet is currently rescanning. Abort existing rescan or wait.",
        ),
        EngineError::Vault(VaultError::Locked | VaultError::MixingOnly) => rpc(
            code::WALLET_UNLOCK_NEEDED,
            "Error: Please enter the wallet passphrase with walletpassphrase first.",
        ),
        EngineError::Vault(VaultError::WrongPassphrase { .. }) => rpc(
            code::WALLET_PASSPHRASE_INCORRECT,
            "Error: The wallet passphrase entered was incorrect.",
        ),
        EngineError::Send(
            dw_engine::SendFailure::AmountExceedsBalance { .. }
            | dw_engine::SendFailure::AmountWithFeeExceedsBalance { .. },
        ) => rpc(code::WALLET_INSUFFICIENT_FUNDS, "Insufficient funds"),
        EngineError::Send(dw_engine::SendFailure::InvalidAddress { .. }) => {
            rpc(code::INVALID_ADDRESS_OR_KEY, "Invalid Dash address")
        }
        other => ConsoleFailure::Engine(other),
    }
}

/// Seconds from `start` (UNIX seconds) until now; 0 when the clock is
/// earlier than `start` or than the epoch.
fn seconds_since(start: u64) -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs().saturating_sub(start))
}

trait EngineResult<T> {
    fn rpc(self) -> Result<T, ConsoleFailure>;
}

impl<T> EngineResult<T> for Result<T, EngineError> {
    fn rpc(self) -> Result<T, ConsoleFailure> {
        self.map_err(engine_failure)
    }
}

/// One console line's context: the session, the wallet the console
/// selector names, and the grant answering an earlier
/// `AuthorizationRequired`.
pub struct ConsoleContext {
    pub session: Arc<NetworkSession>,
    pub wallet: Option<WalletId>,
    pub grant_id: Option<String>,
}

impl ConsoleContext {
    /// Parses and runs one line. `help-console` is answered before
    /// parsing, as dash-qt does.
    pub async fn run(&mut self, line: &str) -> Result<Parsed, ConsoleFailure> {
        let line = line.trim();
        if line == "help-console" {
            return Ok(Parsed {
                result: help_console_text().to_string(),
                is_json: false,
                filtered: line.to_string(),
            });
        }
        execute(line, self).await
    }

    fn wallet(&self) -> Result<WalletId, ConsoleFailure> {
        self.wallet.ok_or(ConsoleFailure::WalletRequired)
    }

    /// The grant for a command that needs `purpose`, or the request for
    /// one. Each grant answers one command.
    fn take_grant(&mut self, purpose: GrantPurpose) -> Result<String, ConsoleFailure> {
        self.grant_id
            .take()
            .ok_or(ConsoleFailure::AuthorizationRequired {
                purpose,
                wallet: self.wallet,
            })
    }
}

impl Executor for ConsoleContext {
    fn execute(
        &mut self,
        method: &str,
        args: &[Zeroizing<String>],
    ) -> impl Future<Output = Result<Json, ConsoleFailure>> + Send {
        let method = method.to_string();
        let args: Vec<Zeroizing<String>> = args.to_vec();
        async move { self.dispatch(&method, &args).await }
    }
}

fn arg(args: &[Zeroizing<String>], i: usize) -> Option<&str> {
    args.get(i).map(|a| a.as_str())
}

fn usage(name: &str) -> ConsoleFailure {
    let text = command(name).map(|c| c.usage).unwrap_or(name);
    rpc(code::MISC_ERROR, text)
}

fn arity(
    name: &str,
    args: &[Zeroizing<String>],
    min: usize,
    max: usize,
) -> Result<(), ConsoleFailure> {
    if args.len() < min || args.len() > max {
        Err(usage(name))
    } else {
        Ok(())
    }
}

fn parse_u32(s: &str) -> Result<u32, ConsoleFailure> {
    s.trim().parse().map_err(|_| {
        rpc(
            code::TYPE_ERROR,
            format!("JSON value is not an integer as expected: {s}"),
        )
    })
}

fn parse_bool(s: &str) -> Result<bool, ConsoleFailure> {
    match s.trim() {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        other => Err(rpc(
            code::TYPE_ERROR,
            format!("Expected type bool, got {other}"),
        )),
    }
}

/// Core `AmountFromValue`: a positive DASH amount with at most 8 decimals.
fn parse_amount(s: &str) -> Result<u64, ConsoleFailure> {
    let bad = || rpc(code::TYPE_ERROR, "Invalid amount");
    let s = s.trim();
    let (int, frac) = s.split_once('.').unwrap_or((s, ""));
    if int.is_empty() && frac.is_empty()
        || frac.len() > 8
        || !int.bytes().all(|b| b.is_ascii_digit())
        || !frac.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(bad());
    }
    let whole: u64 = if int.is_empty() {
        0
    } else {
        int.parse().map_err(|_| bad())?
    };
    let frac_val: u64 = if frac.is_empty() {
        0
    } else {
        format!("{frac:0<8}").parse().map_err(|_| bad())?
    };
    let duffs = whole
        .checked_mul(100_000_000)
        .and_then(|v| v.checked_add(frac_val))
        .ok_or_else(|| rpc(code::TYPE_ERROR, "Amount out of range"))?;
    if duffs == 0 {
        return Err(rpc(code::TYPE_ERROR, "Invalid amount for send"));
    }
    Ok(duffs)
}

fn i64_of(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

/// Core's `listtransactions` category of a record.
fn category(r: &TxRecord) -> &'static str {
    match r.tx_type {
        TxType::Generated if r.status.kind == TxStatusKind::Immature => "immature",
        TxType::Generated if r.status.kind == TxStatusKind::NotAccepted => "orphan",
        TxType::Generated => "generate",
        _ if r.amount < 0 => "send",
        _ => "receive",
    }
}

/// Core's per-entry wallet transaction fields (`WalletTxToJSON`) that the
/// engine knows.
fn tx_fields(r: &TxRecord, out: &mut Vec<(String, Json)>) {
    out.push(("confirmations".into(), Json::int(r.status.confirmations)));
    out.push(("instantlock".into(), Json::Bool(r.status.instant_locked)));
    out.push(("chainlock".into(), Json::Bool(r.status.chain_locked)));
    if let Some(h) = r.block_height {
        out.push(("blockheight".into(), Json::int(h)));
    }
    out.push(("txid".into(), Json::str(r.txid.clone())));
    if let Some(t) = r.timestamp {
        out.push(("time".into(), Json::int(t)));
    }
}

fn record_json(r: &TxRecord) -> Json {
    let mut f: Vec<(String, Json)> = Vec::new();
    if let Some(a) = &r.address {
        f.push(("address".into(), Json::str(a.clone())));
    }
    f.push(("category".into(), Json::str(category(r))));
    f.push(("amount".into(), Json::amount(r.amount)));
    if let Some(l) = &r.label {
        f.push(("label".into(), Json::str(l.clone())));
    }
    if r.amount < 0
        && let Some(fee) = r.fee
    {
        f.push(("fee".into(), Json::amount(-i64_of(fee))));
    }
    tx_fields(r, &mut f);
    if r.amount < 0 {
        f.push((
            "abandoned".into(),
            Json::Bool(r.status.kind == TxStatusKind::Abandoned),
        ));
    }
    Json::Obj(f)
}

impl ConsoleContext {
    /// Every history record of the wallet, oldest first.
    async fn all_records(&self, wallet: WalletId) -> Result<Vec<TxRecord>, ConsoleFailure> {
        let mut out = Vec::new();
        let mut cursor = None;
        loop {
            let page = self
                .session
                .history_page(
                    wallet,
                    HistoryQuery {
                        filter: HistoryFilter::default(),
                        sort: HistorySort::OldestFirst,
                        cursor,
                        limit: dw_engine::history::MAX_PAGE,
                    },
                )
                .await
                .rpc()?;
            out.extend(page.records);
            match page.next_cursor {
                Some(c) => cursor = Some(c),
                None => return Ok(out),
            }
        }
    }

    fn balances(&self, wallet: WalletId) -> Result<dw_engine::WalletBalances, ConsoleFailure> {
        self.session.balances(&wallet).rpc()?.ok_or_else(|| {
            rpc(
                code::WALLET_ERROR,
                "Balance is not known yet: the scan has not reached the wallet's birth height",
            )
        })
    }

    async fn dispatch(
        &mut self,
        method: &str,
        args: &[Zeroizing<String>],
    ) -> Result<Json, ConsoleFailure> {
        let spec =
            command(method).ok_or_else(|| ConsoleFailure::NotAvailable(method.to_string()))?;
        if !spec.available {
            return Err(ConsoleFailure::NotAvailable(method.to_string()));
        }
        let s = Arc::clone(&self.session);
        match method {
            "help" => {
                arity(method, args, 0, 1)?;
                match arg(args, 0) {
                    None => Ok(Json::Str(help_index())),
                    Some(name) => match command(name) {
                        Some(c) if c.available => Ok(Json::str(c.usage)),
                        Some(c) => Err(ConsoleFailure::NotAvailable(c.name.to_string())),
                        None => Err(rpc(
                            code::MISC_ERROR,
                            format!("help: unknown command: {name}"),
                        )),
                    },
                }
            }
            "help-console" => Ok(Json::str(help_console_text())),
            "uptime" => {
                arity(method, args, 0, 0)?;
                let info = s.node_info().rpc()?;
                Ok(Json::int(seconds_since(info.startup_time)))
            }
            "getblockcount" => {
                arity(method, args, 0, 0)?;
                let tip = s.sync_snapshot().rpc()?.tip_height;
                tip.map(Json::int)
                    .ok_or_else(|| rpc(code::MISC_ERROR, "Block height is not known yet"))
            }
            "getbestchainlock" => {
                arity(method, args, 0, 0)?;
                let cl = s
                    .node_info()
                    .rpc()?
                    .best_chainlock
                    .ok_or_else(|| rpc(code::INTERNAL_ERROR, "Unable to find any ChainLock"))?;
                Ok(Json::obj([
                    ("blockhash", Json::str(cl.block_hash)),
                    ("height", Json::int(cl.height)),
                    ("known_block", Json::Bool(true)),
                ]))
            }
            "getconnectioncount" => {
                arity(method, args, 0, 0)?;
                Ok(Json::int(s.sync_snapshot().rpc()?.connected_peers))
            }
            "getpeerinfo" => {
                arity(method, args, 0, 0)?;
                let peers = s.peers().rpc()?;
                Ok(Json::Arr(
                    peers
                        .into_iter()
                        .enumerate()
                        .map(|(i, p)| {
                            let mut f = vec![
                                ("id".to_string(), Json::int(i as u64)),
                                ("addr".to_string(), Json::str(p.address)),
                            ];
                            if let Some(t) = p.connected_since {
                                f.push(("conntime".into(), Json::int(t)));
                            }
                            f.push(("inbound".into(), Json::Bool(p.inbound)));
                            Json::Obj(f)
                        })
                        .collect(),
                ))
            }
            "getnetworkinfo" => {
                arity(method, args, 0, 0)?;
                let info = s.node_info().rpc()?;
                let snap = s.sync_snapshot().rpc()?;
                Ok(Json::obj([
                    ("subversion", Json::str(info.user_agent)),
                    ("networkactive", Json::Bool(snap.running)),
                    (
                        "connections",
                        Json::int(info.connections_in + info.connections_out),
                    ),
                    ("connections_in", Json::int(info.connections_in)),
                    ("connections_out", Json::int(info.connections_out)),
                    ("localaddresses", Json::Arr(Vec::new())),
                ]))
            }
            "listwallets" => {
                arity(method, args, 0, 0)?;
                Ok(Json::Arr(
                    s.wallet_infos()
                        .rpc()?
                        .into_iter()
                        .map(|w| Json::Str(w.name))
                        .collect(),
                ))
            }
            "loadwallet" => {
                arity(method, args, 1, 2)?;
                let name = arg(args, 0).unwrap_or_default();
                let state = s
                    .wallet_load_states()
                    .rpc()?
                    .into_iter()
                    .find(|w| w.name == name)
                    .ok_or_else(|| {
                        rpc(
                            code::WALLET_NOT_FOUND,
                            "Requested wallet does not exist or is not loaded",
                        )
                    })?;
                if state.loaded {
                    return Err(rpc(
                        code::WALLET_ALREADY_LOADED,
                        format!("Wallet \"{name}\" is already loaded."),
                    ));
                }
                s.load_wallet(state.wallet_id).await.rpc()?;
                Ok(Json::obj([
                    ("name", Json::str(name)),
                    ("warning", Json::str("")),
                ]))
            }
            "unloadwallet" => {
                arity(method, args, 0, 2)?;
                let id = match arg(args, 0) {
                    Some(name) => s
                        .wallet_infos()
                        .rpc()?
                        .into_iter()
                        .find(|w| w.name == name)
                        .map(|w| w.wallet_id)
                        .ok_or_else(|| {
                            rpc(
                                code::WALLET_NOT_FOUND,
                                "Requested wallet does not exist or is not loaded",
                            )
                        })?,
                    None => self.wallet()?,
                };
                s.unload_wallet(id).await.rpc()?;
                Ok(Json::obj([("warning", Json::str(""))]))
            }
            "getwalletinfo" => {
                arity(method, args, 0, 0)?;
                let w = self.wallet()?;
                let info = s.wallet_info(&w).rpc()?;
                let txcount = {
                    let mut ids: Vec<String> = self
                        .all_records(w)
                        .await?
                        .into_iter()
                        .map(|r| r.txid)
                        .collect();
                    ids.sort_unstable();
                    ids.dedup();
                    ids.len()
                };
                let mut f = vec![("walletname".to_string(), Json::str(info.name.clone()))];
                if let Some(b) = info.balances {
                    f.push(("balance".into(), Json::amount(i64_of(b.confirmed))));
                    f.push(("coinjoin_balance".into(), Json::amount(i64_of(b.coinjoin))));
                    f.push((
                        "unconfirmed_balance".into(),
                        Json::amount(i64_of(b.unconfirmed)),
                    ));
                    f.push(("immature_balance".into(), Json::amount(i64_of(b.immature))));
                }
                f.push(("txcount".into(), Json::int(txcount as u64)));
                f.push(("private_keys_enabled".into(), Json::Bool(!info.watch_only)));
                let scanning = match s.rescan_progress().rpc()? {
                    Some(p) => {
                        let mut o = vec![(
                            "duration".to_string(),
                            Json::int(seconds_since(p.started_at)),
                        )];
                        if let (Some(cur), Some(target)) = (p.current_height, p.target_height)
                            && target > p.from_height
                        {
                            let done = f64::from(cur.saturating_sub(p.from_height))
                                / f64::from(target - p.from_height);
                            o.push((
                                "progress".into(),
                                Json::Num(format!("{:.8}", done.min(1.0))),
                            ));
                        }
                        Json::Obj(o)
                    }
                    None => Json::Bool(false),
                };
                f.push(("scanning".into(), scanning));
                Ok(Json::Obj(f))
            }
            "getbalance" => {
                arity(method, args, 0, 4)?;
                let b = self.balances(self.wallet()?)?;
                Ok(Json::amount(i64_of(b.confirmed)))
            }
            "getunconfirmedbalance" => {
                arity(method, args, 0, 0)?;
                let b = self.balances(self.wallet()?)?;
                Ok(Json::amount(i64_of(b.unconfirmed)))
            }
            "getbalances" => {
                arity(method, args, 0, 0)?;
                let b = self.balances(self.wallet()?)?;
                Ok(Json::obj([(
                    "mine",
                    Json::obj([
                        ("trusted", Json::amount(i64_of(b.confirmed))),
                        ("untrusted_pending", Json::amount(i64_of(b.unconfirmed))),
                        ("immature", Json::amount(i64_of(b.immature))),
                        ("coinjoin", Json::amount(i64_of(b.coinjoin))),
                    ]),
                )]))
            }
            "getnewaddress" => {
                arity(method, args, 0, 1)?;
                let w = self.wallet()?;
                let label = arg(args, 0).filter(|l| !l.is_empty()).map(str::to_string);
                Ok(Json::Str(
                    s.next_receive_address(w, label).await.rpc()?.address,
                ))
            }
            "listunspent" => {
                arity(method, args, 0, 1)?;
                let w = self.wallet()?;
                let minconf = arg(args, 0).map(parse_u32).transpose()?.unwrap_or(1);
                let coins = s
                    .utxos(
                        w,
                        CoinFilter {
                            include_locked: false,
                            fully_mixed_only: false,
                            min_confirmations: Some(minconf),
                        },
                    )
                    .await
                    .rpc()?;
                Ok(Json::Arr(
                    coins
                        .into_iter()
                        .map(|c| {
                            let mut f = vec![
                                ("txid".to_string(), Json::str(c.outpoint.txid.to_string())),
                                ("vout".into(), Json::int(c.outpoint.vout)),
                                ("address".into(), Json::str(c.address)),
                            ];
                            if let Some(l) = c.label {
                                f.push(("label".into(), Json::str(l)));
                            }
                            f.push(("amount".into(), Json::amount(i64_of(c.amount))));
                            f.push(("confirmations".into(), Json::int(c.confirmations)));
                            f.push(("spendable".into(), Json::Bool(c.spendable)));
                            Json::Obj(f)
                        })
                        .collect(),
                ))
            }
            "lockunspent" => {
                arity(method, args, 1, 2)?;
                let w = self.wallet()?;
                let unlock = parse_bool(arg(args, 0).unwrap_or_default())?;
                let outpoints = match arg(args, 1) {
                    Some(text) => parse_outpoints(text)?,
                    None if unlock => s.locked_outpoints(w).await.rpc()?,
                    None => {
                        return Err(rpc(
                            code::INVALID_PARAMETER,
                            "Invalid parameter, expected locked UTXOs",
                        ));
                    }
                };
                if unlock {
                    s.unlock_outpoints(w, outpoints).await.rpc()?;
                } else {
                    s.lock_outpoints(w, outpoints).await.map_err(|e| match e {
                        EngineError::OutpointNotFound(_) => rpc(
                            code::INVALID_PARAMETER,
                            "Invalid parameter, unknown transaction",
                        ),
                        other => engine_failure(other),
                    })?;
                }
                Ok(Json::Bool(true))
            }
            "listlockunspent" => {
                arity(method, args, 0, 0)?;
                let w = self.wallet()?;
                Ok(Json::Arr(
                    s.locked_outpoints(w)
                        .await
                        .rpc()?
                        .into_iter()
                        .map(|o| {
                            Json::obj([
                                ("txid", Json::str(o.txid.to_string())),
                                ("vout", Json::int(o.vout)),
                            ])
                        })
                        .collect(),
                ))
            }
            "listtransactions" => {
                arity(method, args, 0, 4)?;
                let w = self.wallet()?;
                let label = arg(args, 0).unwrap_or("*");
                let count = arg(args, 1).map(parse_u32).transpose()?.unwrap_or(10) as usize;
                let skip = arg(args, 2).map(parse_u32).transpose()?.unwrap_or(0) as usize;
                let records: Vec<TxRecord> = self
                    .all_records(w)
                    .await?
                    .into_iter()
                    .filter(|r| label == "*" || r.label.as_deref() == Some(label))
                    .collect();
                // Core: the `count` most recent after skipping `skip`, oldest
                // first.
                let end = records.len().saturating_sub(skip);
                let start = end.saturating_sub(count);
                Ok(Json::Arr(
                    records[start..end].iter().map(record_json).collect(),
                ))
            }
            "gettransaction" => {
                arity(method, args, 1, 3)?;
                let w = self.wallet()?;
                let txid = arg(args, 0).unwrap_or_default().to_string();
                let d = s.tx_detail(w, txid).await.rpc()?;
                let net: i64 = d.records.iter().map(|r| r.amount).sum::<i64>()
                    + d.records
                        .first()
                        .filter(|r| r.amount < 0)
                        .and(d.fee)
                        .map(i64_of)
                        .unwrap_or(0);
                let mut f = vec![("amount".to_string(), Json::amount(net))];
                if let (Some(fee), Some(first)) = (d.fee, d.records.first())
                    && first.amount < 0
                {
                    f.push(("fee".into(), Json::amount(-i64_of(fee))));
                }
                f.push(("confirmations".into(), Json::int(d.status.confirmations)));
                f.push(("instantlock".into(), Json::Bool(d.status.instant_locked)));
                f.push(("chainlock".into(), Json::Bool(d.status.chain_locked)));
                if let Some(h) = &d.block_hash {
                    f.push(("blockhash".into(), Json::str(h.clone())));
                }
                if let Some(h) = d.block_height {
                    f.push(("blockheight".into(), Json::int(h)));
                }
                f.push(("txid".into(), Json::str(d.txid.clone())));
                if let Some(t) = d.timestamp {
                    f.push(("time".into(), Json::int(t)));
                }
                if d.status.kind == TxStatusKind::Abandoned {
                    f.push(("abandoned".into(), Json::Bool(true)));
                }
                f.push((
                    "details".into(),
                    Json::Arr(
                        d.records
                            .iter()
                            .map(|r| {
                                let mut e = Vec::new();
                                if let Some(a) = &r.address {
                                    e.push(("address".to_string(), Json::str(a.clone())));
                                }
                                e.push(("category".into(), Json::str(category(r))));
                                e.push(("amount".into(), Json::amount(r.amount)));
                                if let Some(l) = &r.label {
                                    e.push(("label".into(), Json::str(l.clone())));
                                }
                                Json::Obj(e)
                            })
                            .collect(),
                    ),
                ));
                f.push(("hex".into(), Json::str(d.raw_hex)));
                Ok(Json::Obj(f))
            }
            "abandontransaction" => {
                arity(method, args, 1, 1)?;
                let w = self.wallet()?;
                let txid = dashcore::Txid::from_str(arg(args, 0).unwrap_or_default())
                    .map_err(|_| rpc(code::INVALID_PARAMETER, "txid must be hexadecimal string"))?;
                s.abandon_transaction(w, txid).await.rpc()?;
                Ok(Json::Null)
            }
            "rescanblockchain" => {
                arity(method, args, 0, 2)?;
                self.wallet()?;
                if arg(args, 1).is_some() {
                    return Err(ConsoleFailure::NotAvailable(
                        "rescanblockchain with stop_height".into(),
                    ));
                }
                let start = arg(args, 0).map(parse_u32).transpose()?.unwrap_or(0);
                let from = if start == 0 {
                    RescanFrom::Genesis
                } else {
                    RescanFrom::Height(start)
                };
                s.rescan(from).await.map_err(|e| match e {
                    EngineError::HeightOutOfRange(_) => {
                        rpc(code::INVALID_PARAMETER, "Invalid start_height")
                    }
                    other => engine_failure(other),
                })?;
                let mut f = vec![("start_height".to_string(), Json::int(start))];
                if let Some(tip) = s.sync_snapshot().rpc()?.tip_height {
                    f.push(("stop_height".into(), Json::int(tip)));
                }
                Ok(Json::Obj(f))
            }
            "abortrescan" => {
                arity(method, args, 0, 0)?;
                Ok(Json::Bool(s.cancel_rescan().await.rpc()?))
            }
            "setlabel" => {
                arity(method, args, 2, 2)?;
                let w = self.wallet()?;
                let address = arg(args, 0).unwrap_or_default().to_string();
                let label = arg(args, 1).unwrap_or_default().to_string();
                let saved = s
                    .save_address_book_entry(
                        w,
                        address.clone(),
                        label.clone(),
                        BookPurpose::Send,
                        true,
                    )
                    .await;
                match saved {
                    Ok(_) => {}
                    Err(EngineError::Labels(dw_engine::LabelsFailure::OwnAddress)) => {
                        s.save_address_book_entry(w, address, label, BookPurpose::Receive, true)
                            .await
                            .rpc()?;
                    }
                    Err(EngineError::Labels(dw_engine::LabelsFailure::InvalidAddress)) => {
                        return Err(rpc(code::INVALID_ADDRESS_OR_KEY, "Invalid Dash address"));
                    }
                    Err(e) => return Err(engine_failure(e)),
                }
                Ok(Json::Null)
            }
            "validateaddress" => {
                arity(method, args, 1, 1)?;
                let text = arg(args, 0).unwrap_or_default();
                let network = s.network().core_network();
                match Address::from_str(text)
                    .ok()
                    .and_then(|a| a.require_network(network).ok())
                {
                    Some(a) => Ok(Json::obj([
                        ("isvalid", Json::Bool(true)),
                        ("address", Json::str(a.to_string())),
                        (
                            "scriptPubKey",
                            Json::str(hex::encode(a.script_pubkey().as_bytes())),
                        ),
                        ("isscript", Json::Bool(a.script_pubkey().is_p2sh())),
                    ])),
                    None => Ok(Json::obj([("isvalid", Json::Bool(false))])),
                }
            }
            "verifymessage" => {
                arity(method, args, 3, 3)?;
                let network = s.network().core_network();
                match dw_message::verify_message(
                    arg(args, 0).unwrap_or_default(),
                    arg(args, 1).unwrap_or_default(),
                    arg(args, 2).unwrap_or_default().as_bytes(),
                    network,
                ) {
                    Ok(()) => Ok(Json::Bool(true)),
                    Err(e) => match e.rpc_message() {
                        Some(m) => Err(rpc(
                            match e {
                                dw_message::VerifyError::AddressNoKey => code::TYPE_ERROR,
                                _ => code::INVALID_ADDRESS_OR_KEY,
                            },
                            m,
                        )),
                        None => Ok(Json::Bool(false)),
                    },
                }
            }
            "signmessage" => {
                arity(method, args, 2, 2)?;
                let w = self.wallet()?;
                let grant = self.take_grant(GrantPurpose::SignMessage)?;
                let sig = s
                    .sign_message(
                        w,
                        arg(args, 0).unwrap_or_default().to_string(),
                        arg(args, 1).unwrap_or_default().as_bytes().to_vec(),
                        grant,
                    )
                    .await
                    .rpc()?;
                Ok(Json::Str(sig))
            }
            "sendtoaddress" => {
                arity(method, args, 2, 5)?;
                let w = self.wallet()?;
                let address = arg(args, 0).unwrap_or_default().to_string();
                let amount = parse_amount(arg(args, 1).unwrap_or_default())?;
                let comment = arg(args, 2).filter(|c| !c.is_empty()).map(str::to_string);
                let subtract = arg(args, 4).map(parse_bool).transpose()?.unwrap_or(false);
                let draft = s.new_tx_draft(w).rpc()?;
                draft
                    .set_recipients(vec![Recipient {
                        address,
                        amount,
                        subtract_fee_from_amount: subtract,
                        label: None,
                        message: comment,
                    }])
                    .rpc()?;
                // Checked before a grant is asked for, so a bad address or
                // amount does not cost an authorization.
                draft.estimate().await.rpc()?;
                let grant = self.take_grant(GrantPurpose::Spend { max_duffs: amount })?;
                let prepared = draft.prepare(grant).await.rpc()?;
                let outcome = draft.broadcast(prepared).await.rpc()?;
                Ok(Json::Str(outcome.txid))
            }
            "walletlock" => {
                arity(method, args, 0, 0)?;
                s.lock_vault().rpc()?;
                Ok(Json::Null)
            }
            "walletpassphrase" => {
                arity(method, args, 2, 3)?;
                let pass = Zeroizing::new(arg(args, 0).unwrap_or_default().as_bytes().to_vec());
                if pass.is_empty() {
                    return Err(rpc(code::INVALID_PARAMETER, "passphrase can not be empty"));
                }
                let timeout = arg(args, 1)
                    .unwrap_or_default()
                    .trim()
                    .parse::<i64>()
                    .map_err(|_| rpc(code::TYPE_ERROR, "Expected type number for timeout"))?;
                if timeout < 0 {
                    return Err(rpc(code::INVALID_PARAMETER, "Timeout cannot be negative."));
                }
                // Core caps the timeout at 100000000 s.
                let timeout = timeout.min(100_000_000) as u64;
                let scope = match arg(args, 2).map(parse_bool).transpose()? {
                    Some(true) => UnlockScope::MixingOnly,
                    _ => UnlockScope::Full,
                };
                s.vault_op(move |v| v.unlock(&pass, scope)).await.rpc()?;
                // The Qt RPC timer relocks after the timeout.
                let relock = Arc::clone(&s);
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(timeout)).await;
                    if let Err(e) = relock.lock_vault() {
                        tracing::debug!(error = %e, "walletpassphrase relock skipped");
                    }
                });
                Ok(Json::Null)
            }
            other => Err(ConsoleFailure::NotAvailable(other.to_string())),
        }
    }
}

/// `help` without arguments: the available commands by category.
fn help_index() -> String {
    let mut out = String::new();
    for cat in CATEGORIES {
        let names: Vec<&str> = COMMANDS
            .iter()
            .filter(|c| c.category == cat && c.available)
            .map(|c| c.usage)
            .collect();
        if names.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!("== {cat} ==\n"));
        for n in names {
            out.push_str(n);
            out.push('\n');
        }
    }
    out.pop();
    out
}

/// `[{"txid":"…","vout":n},…]` as typed in the console.
fn parse_outpoints(text: &str) -> Result<Vec<dashcore::OutPoint>, ConsoleFailure> {
    let bad = || {
        rpc(
            code::INVALID_PARAMETER,
            "Invalid parameter, expected object",
        )
    };
    let value: serde_json::Value = serde_json::from_str(text).map_err(|_| bad())?;
    let items = value.as_array().ok_or_else(bad)?;
    items
        .iter()
        .map(|o| {
            let txid = o
                .get("txid")
                .and_then(|t| t.as_str())
                .and_then(|t| dashcore::Txid::from_str(t).ok())
                .ok_or_else(|| rpc(code::INVALID_PARAMETER, "txid must be hexadecimal string"))?;
            let vout = o
                .get("vout")
                .and_then(|v| v.as_u64())
                .and_then(|v| u32::try_from(v).ok())
                .ok_or_else(|| {
                    rpc(
                        code::INVALID_PARAMETER,
                        "Invalid parameter, vout cannot be negative",
                    )
                })?;
            Ok(dashcore::OutPoint::new(txid, vout))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_parse_like_core() {
        assert_eq!(parse_amount("1").unwrap(), 100_000_000);
        assert_eq!(parse_amount("0.5").unwrap(), 50_000_000);
        assert_eq!(parse_amount("0.00000001").unwrap(), 1);
        assert!(parse_amount("0.000000001").is_err());
        assert!(parse_amount("-1").is_err());
        assert!(parse_amount("0").is_err());
        assert!(parse_amount("abc").is_err());
    }

    #[test]
    fn outpoints_parse_from_console_json() {
        let t = "11".repeat(32);
        let v = parse_outpoints(&format!("[{{\"txid\":\"{t}\",\"vout\":1}}]")).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].vout, 1);
        assert!(parse_outpoints("{}").is_err());
    }

    #[test]
    fn help_lists_available_commands_by_category() {
        let h = help_index();
        assert!(h.starts_with("== Blockchain ==\ngetbestchainlock\ngetblockcount"));
        assert!(h.contains("== Wallet ==\nabandontransaction \"txid\""));
        assert!(
            !h.contains("getblockhash"),
            "unavailable commands are not listed"
        );
        assert!(!h.ends_with('\n'));
    }
}
