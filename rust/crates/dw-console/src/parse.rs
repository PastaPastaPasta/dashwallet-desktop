//! dash-qt's console command-line grammar, ported state for state from
//! `RPCConsole::RPCParseCommandLine` (src/qt/rpcconsole.cpp, Dash Core
//! v24.0.0-rc.2): whitespace or comma separators, `'…'` and `"…"` quoting
//! with escapes, nested calls `a(b(1) 2)` and result queries `[key]` /
//! `[0]`. The same pass computes the history text in which the arguments of
//! sensitive commands are replaced by `(…)`.

use std::future::Future;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use zeroize::Zeroizing;

use crate::json::Json;
use crate::{ConsoleFailure, is_sensitive};

/// Runs one command for the parser. `args` are the command's argument
/// strings as typed (or results of nested calls).
pub trait Executor {
    fn execute(
        &mut self,
        method: &str,
        args: &[Zeroizing<String>],
    ) -> impl Future<Output = Result<Json, ConsoleFailure>> + Send;
}

/// Never runs anything: the history-filter pass.
struct NoExec;

impl Executor for NoExec {
    #[allow(clippy::manual_async_fn)]
    fn execute(
        &mut self,
        _: &str,
        _: &[Zeroizing<String>],
    ) -> impl Future<Output = Result<Json, ConsoleFailure>> + Send {
        async { Ok(Json::Null) }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    EatingSpaces,
    EatingSpacesInArg,
    EatingSpacesInBrackets,
    Argument,
    SingleQuoted,
    DoubleQuoted,
    EscapeOuter,
    EscapeDoubleQuoted,
    CommandExecuted,
    CommandExecutedInner,
}

fn invalid_syntax() -> ConsoleFailure {
    ConsoleFailure::Parse("Invalid Syntax".into())
}

/// What one pass over a line produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    /// The result text (`strResult`).
    pub result: String,
    /// Whether the final result was a JSON value rather than a string.
    pub is_json: bool,
    /// The line with sensitive arguments replaced by `(…)`.
    pub filtered: String,
}

struct Parser<'e, E> {
    exec: Option<&'e mut E>,
    stack: Vec<Vec<Zeroizing<String>>>,
    depth_sensitive: u32,
    filter_begin: usize,
    ranges: Vec<(usize, usize)>,
}

impl<E: Executor> Parser<'_, E> {
    fn add_to_current_stack(&mut self, arg: Zeroizing<String>, chpos: usize) {
        if self.stack.last().is_none_or(Vec::is_empty)
            && self.depth_sensitive == 0
            && is_sensitive(&arg)
        {
            self.depth_sensitive = 1;
            self.filter_begin = chpos;
        }
        if self.stack.is_empty() {
            self.stack.push(Vec::new());
        }
        if let Some(top) = self.stack.last_mut() {
            top.push(arg);
        }
    }

    fn close_out_params(&mut self, chpos: usize) {
        if self.depth_sensitive > 0 {
            self.depth_sensitive -= 1;
            if self.depth_sensitive == 0 {
                self.ranges.push((self.filter_begin, chpos));
                self.filter_begin = 0;
            }
        }
        self.stack.pop();
    }
}

/// `RPCParseCommandLine` with `fExecute` = `exec.is_some()`.
async fn parse_line<E: Executor>(
    line: &str,
    exec: Option<&mut E>,
) -> Result<Parsed, ConsoleFailure> {
    let execute = exec.is_some();
    let mut p = Parser {
        exec,
        stack: vec![Vec::new()],
        depth_sensitive: 0,
        filter_begin: 0,
        ranges: Vec::new(),
    };
    let mut terminated = Zeroizing::new(line.to_string());
    if !terminated.ends_with('\n') {
        terminated.push('\n');
    }
    let mut state = State::EatingSpaces;
    let mut curarg = Zeroizing::new(String::new());
    let mut last = Json::Null;
    let mut result = String::new();
    let mut result_is_json = false;

    for (chpos, ch) in terminated.char_indices() {
        if matches!(state, State::CommandExecuted | State::CommandExecutedInner) {
            let mut break_parsing = true;
            if ch == '[' {
                curarg.clear();
                state = State::CommandExecutedInner;
            } else if state == State::CommandExecutedInner {
                if ch != ']' {
                    curarg.push(ch);
                } else {
                    if !curarg.is_empty() && execute {
                        last = last
                            .query(&curarg)
                            .ok_or_else(|| ConsoleFailure::Parse("Invalid result query".into()))?;
                    }
                    state = State::CommandExecuted;
                }
            } else {
                break_parsing = false;
                p.close_out_params(chpos);
                *curarg = match &last {
                    Json::Str(s) => s.clone(),
                    other => other.write(2),
                };
                if !curarg.is_empty() {
                    if !p.stack.is_empty() {
                        p.add_to_current_stack(std::mem::take(&mut curarg), chpos);
                    } else {
                        result = curarg.to_string();
                        result_is_json = !matches!(last, Json::Str(_));
                    }
                }
                curarg.clear();
                state = State::EatingSpaces;
            }
            if break_parsing {
                continue;
            }
        }
        match state {
            State::CommandExecuted
            | State::CommandExecutedInner
            | State::Argument
            | State::EatingSpacesInArg
            | State::EatingSpacesInBrackets
            | State::EatingSpaces => match ch {
                '"' => state = State::DoubleQuoted,
                '\'' => state = State::SingleQuoted,
                '\\' => state = State::EscapeOuter,
                '(' | ')' | '\n' => {
                    if state == State::EatingSpacesInArg {
                        return Err(invalid_syntax());
                    }
                    if state == State::Argument {
                        if ch == '(' && p.stack.last().is_some_and(|t| !t.is_empty()) {
                            if p.depth_sensitive > 0 {
                                p.depth_sensitive += 1;
                            }
                            p.stack.push(Vec::new());
                        }
                        if p.stack.is_empty() {
                            return Err(invalid_syntax());
                        }
                        p.add_to_current_stack(std::mem::take(&mut curarg), chpos);
                        state = State::EatingSpacesInBrackets;
                    }
                    if (ch == ')' || ch == '\n') && !p.stack.is_empty() {
                        if let Some(exec) = p.exec.as_deref_mut() {
                            let frame = p.stack.last().map(Vec::as_slice).unwrap_or_default();
                            let Some((method, args)) = frame.split_first() else {
                                return Err(invalid_syntax());
                            };
                            last = exec.execute(method, args).await?;
                        }
                        state = State::CommandExecuted;
                        curarg.clear();
                    }
                }
                ' ' | ',' | '\t' => {
                    if state == State::EatingSpacesInArg && curarg.is_empty() && ch == ',' {
                        return Err(invalid_syntax());
                    } else if state == State::Argument {
                        p.add_to_current_stack(std::mem::take(&mut curarg), chpos);
                    }
                    if matches!(state, State::EatingSpacesInBrackets | State::Argument) && ch == ','
                    {
                        state = State::EatingSpacesInArg;
                    } else {
                        state = State::EatingSpaces;
                    }
                }
                _ => {
                    curarg.push(ch);
                    state = State::Argument;
                }
            },
            State::SingleQuoted => match ch {
                '\'' => state = State::Argument,
                _ => curarg.push(ch),
            },
            State::DoubleQuoted => match ch {
                '"' => state = State::Argument,
                '\\' => state = State::EscapeDoubleQuoted,
                _ => curarg.push(ch),
            },
            State::EscapeOuter => {
                curarg.push(ch);
                state = State::Argument;
            }
            State::EscapeDoubleQuoted => {
                if ch != '"' && ch != '\\' {
                    curarg.push('\\');
                }
                curarg.push(ch);
                state = State::DoubleQuoted;
            }
        }
    }
    // The loop ran to the end of the terminated line.
    if state == State::CommandExecuted && !p.stack.is_empty() {
        p.close_out_params(terminated.len());
    }
    let mut filtered = line.to_string();
    for &(begin, end) in p.ranges.iter().rev() {
        let begin = begin.min(filtered.len());
        let end = end.min(filtered.len());
        filtered.replace_range(begin..end, "(…)");
    }
    let mut is_json = result_is_json;
    match state {
        State::CommandExecuted => {
            match &last {
                Json::Str(s) => result = s.clone(),
                other => {
                    is_json = true;
                    result = other.write(2);
                }
            }
            Ok(Parsed {
                result,
                is_json,
                filtered,
            })
        }
        State::Argument | State::EatingSpaces => Ok(Parsed {
            result,
            is_json,
            filtered,
        }),
        _ => Err(ConsoleFailure::Parse("unbalanced ' or \"".into())),
    }
}

/// dash-qt's history text for `line` (trimmed): sensitive commands'
/// arguments become `(…)`. `Parse` for a line that does not parse
/// ("Error: Invalid command line").
pub fn redact(line: &str) -> Result<String, ConsoleFailure> {
    let line = line.trim();
    let fut = pin!(parse_line::<NoExec>(line, None));
    match fut.poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(r) => r.map(|p| p.filtered),
        Poll::Pending => Err(ConsoleFailure::Parse("parser did not finish".into())),
    }
}

/// Parses and runs `line` with `exec`.
pub async fn execute<E: Executor>(line: &str, exec: &mut E) -> Result<Parsed, ConsoleFailure> {
    parse_line(line, Some(exec)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Records calls and answers from a table.
    #[derive(Default)]
    struct Fake {
        calls: Calls,
    }

    impl Executor for Fake {
        fn execute(
            &mut self,
            method: &str,
            args: &[Zeroizing<String>],
        ) -> impl Future<Output = Result<Json, ConsoleFailure>> + Send {
            let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
            self.calls.push((method.to_string(), args.clone()));
            let out = match method {
                "getblockhash" => Ok(Json::str(format!("hash{}", args[0]))),
                "getblock" => Ok(Json::obj([
                    ("hash", Json::str(args[0].clone())),
                    ("tx", Json::Arr(vec![Json::str("t0"), Json::str("t1")])),
                ])),
                "fail" => Err(ConsoleFailure::Rpc {
                    code: -8,
                    message: "bad".into(),
                }),
                _ => Ok(Json::Null),
            };
            async move { out }
        }
    }

    type Calls = Vec<(String, Vec<String>)>;

    fn run(line: &str) -> (Result<Parsed, ConsoleFailure>, Calls) {
        let mut f = Fake::default();
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let r = rt.block_on(execute(line, &mut f));
        (r, f.calls)
    }

    #[test]
    fn test_qt_145_plain_and_parenthesized_calls() {
        let (r, calls) = run("getblockhash 0");
        assert_eq!(r.unwrap().result, "hash0");
        assert_eq!(calls, vec![("getblockhash".into(), vec!["0".into()])]);
        let (r, _) = run("getblockhash(0)");
        assert_eq!(r.unwrap().result, "hash0");
        let (r, _) = run("getblockhash,0");
        assert_eq!(r.unwrap().result, "hash0");
    }

    #[test]
    fn test_qt_145_nested_calls_and_queries() {
        let (r, calls) = run("getblock(getblockhash(0) 1)");
        let p = r.unwrap();
        assert!(p.is_json);
        assert_eq!(
            calls[1],
            ("getblock".into(), vec!["hash0".into(), "1".into()])
        );
        assert!(p.result.contains("\"tx\": ["));
        let (r, _) = run("getblock(getblockhash(0),1)[tx][1]");
        assert_eq!(r.unwrap().result, "t1");
        let (r, _) = run("getblock(getblockhash(0))[tx]");
        assert_eq!(r.unwrap().result, "[\n  \"t0\",\n  \"t1\"\n]");
        let (r, _) = run("getblockhash(0)[x]");
        assert!(
            matches!(r, Err(ConsoleFailure::Parse(_))),
            "query on a string"
        );
    }

    #[test]
    fn test_qt_145_quotes_and_escapes() {
        let (_, calls) = run(r#"setlabel "a b" 'c d' e\ f "q\"\\\n""#);
        assert_eq!(
            calls[0].1,
            vec!["a b", "c d", "e f", "q\"\\\\n"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        );
        let (r, _) = run("setlabel \"open");
        assert!(matches!(r, Err(ConsoleFailure::Parse(_))));
        let (r, _) = run("a(b,,c)");
        assert!(matches!(r, Err(ConsoleFailure::Parse(_))));
    }

    #[test]
    fn rpc_errors_propagate() {
        let (r, _) = run("fail 1");
        assert!(matches!(r, Err(ConsoleFailure::Rpc { code: -8, .. })));
    }

    #[test]
    fn test_qt_145_history_redaction_matches_dash_qt() {
        assert_eq!(
            redact("walletpassphrase secret 60").unwrap(),
            "walletpassphrase(…)"
        );
        assert_eq!(
            redact("WalletPassphrase  secret 60  ").unwrap(),
            "WalletPassphrase(…)"
        );
        assert_eq!(redact("getbalance").unwrap(), "getbalance");
        assert_eq!(
            redact("signmessagewithprivkey(abc,def)").unwrap(),
            "signmessagewithprivkey(…)"
        );
        assert_eq!(
            redact("getblock(upgradetohd(\"words\" pass))").unwrap(),
            "getblock(upgradetohd(…))"
        );
        assert_eq!(
            redact("help walletpassphrase").unwrap(),
            "help walletpassphrase"
        );
        assert!(redact("walletpassphrase \"unbalanced").is_err());
    }
}
