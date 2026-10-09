//! The `args` of a `dashpay session` request, checked against the DashPay
//! command grammar before clap sees them (review DW-E0-09 r2 finding 1).
//!
//! clap copies its arguments into ordinary allocations, freed unwiped. So
//! each token is checked first, in place in its zeroizing buffer: it must
//! be a subcommand of the command reached so far, an option of that command
//! (`--name`, `--name value` or `--name=value`), or a value of the type the
//! option or the next positional takes ([`Ty`]). Anything else (an unknown
//! option, a surplus value, a value of the wrong form) refuses the request
//! with the token's position, never its text. The command tree and the
//! option names come from clap's own definition; this module adds only each
//! value's type.
//!
//! Every value is public by contract (m4 §1): clap copies it unwiped, and
//! no form check can tell a key from an id of the same shape, so a
//! credential travels only in `input`. Free text ([`Ty::Text`]: names,
//! notes, a search), a URL or a path that looks like a bearer input is
//! refused anyway, as a diagnostic.

use std::sync::OnceLock;

use clap::{Arg, ArgAction, Command, CommandFactory};
use zeroize::Zeroizing;

use super::RequestArgs;

/// A value's type.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Ty {
    /// A wallet id: 64 lower-case hex characters.
    WalletId,
    /// An identity or contact id: Base58 of 32 bytes.
    Identity,
    /// An id the engine issued (a draft, an invitation link, a faucet key,
    /// an activity cursor): 1–64 of `[0-9a-z_-]`.
    EngineId,
    /// What `dispatch-status` takes: a txid or state-transition hash (64
    /// lower-case hex) or a funding step id (`registration/<id>/funding`,
    /// `topup/<id>/funding`).
    Artifact,
    /// A Dash address: 26–35 Base58 characters.
    Address,
    /// A decimal integer that fits a `u64`, without a leading zero.
    U64,
    /// The same, for a `u32`.
    U32,
    /// One of these names.
    Enum(&'static [&'static str]),
    /// A DPNS label: 3–63 of `[A-Za-z0-9-]`, not starting or ending with
    /// `-`.
    Label,
    /// What `name search` takes: 1–63 of `[A-Za-z0-9-]`.
    LabelText,
    /// A DPNS name: a label, with `.dash` or without.
    Username,
    /// An `http(s)://` URL: up to 2048 printable ASCII characters, not
    /// bearer-shaped.
    Url,
    /// A file path: 1–4096 of `[A-Za-z0-9._/ +@,~-]`, not bearer-shaped.
    Path,
    /// Free text up to this many bytes, without control characters but LF
    /// and tab, not bearer-shaped.
    Text(usize),
}

const SECTIONS: &[&str] = &["requests", "contacts", "pending", "hidden"];
const SORTS: &[&str] = &["display_name", "username", "date_added", "last_activity"];
const FILTERS: &[&str] = &["all", "sent", "received"];
const SIZES: &[&str] = &["small", "large"];

/// The type of `arg`'s values in the command `cmd` (the leaf subcommand's
/// name); `None` for an argument with no type, which refuses it.
fn value_type(cmd: &str, arg: &str) -> Option<Ty> {
    Some(match (cmd, arg) {
        (_, "wallet") => Ty::WalletId,
        (_, "identity" | "existing_identity" | "contact") => Ty::Identity,
        ("activity", "cursor") => Ty::EngineId,
        (_, "draft" | "invitation_id" | "link_id" | "faucet_key") => Ty::EngineId,
        (_, "artifact") => Ty::Artifact,
        (_, "to") => Ty::Address,
        (_, "cursor" | "up_to" | "max_duffs" | "max_credits" | "duffs" | "credits" | "amount") => {
            Ty::U64
        }
        (_, "limit") => Ty::U32,
        (_, "sections") => Ty::Enum(SECTIONS),
        (_, "sort") => Ty::Enum(SORTS),
        (_, "filter") => Ty::Enum(FILTERS),
        (_, "size") => Ty::Enum(SIZES),
        // The checklist reports the rules a bad label breaks.
        ("check", "label") => Ty::Text(256),
        (_, "prefix") => Ty::LabelText,
        (_, "label" | "temporary_label") => Ty::Label,
        (_, "username") => Ty::Username,
        (_, "avatar_url") => Ty::Url,
        (_, "input_file" | "faucet_proof_file" | "avatar_file") => Ty::Path,
        (_, "display_name" | "alias" | "text") => Ty::Text(256),
        (_, "public_message" | "note") => Ty::Text(1024),
        _ => return None,
    })
}

impl Ty {
    /// What a value of this type is, for a refusal.
    fn describe(self) -> &'static str {
        match self {
            Ty::WalletId => "a wallet id (64 lower-case hex characters)",
            Ty::Identity => "an identity id (Base58 of 32 bytes)",
            Ty::EngineId => "an id the engine issued (1-64 of [0-9a-z_-])",
            Ty::Artifact => "a txid, a state-transition hash or a funding step id",
            Ty::Address => "a Dash address",
            Ty::U64 | Ty::U32 => "a decimal number",
            Ty::Enum(_) => "one of the listed names",
            Ty::Label => "a DPNS label (3-63 of [A-Za-z0-9-], no edge hyphen)",
            Ty::LabelText => "label text (1-63 of [A-Za-z0-9-])",
            Ty::Username => "a DPNS name",
            Ty::Url => "an http(s) URL that is not a bearer input",
            Ty::Path => "a file path (of [A-Za-z0-9._/ +@,~-]) that is not a bearer input",
            Ty::Text(_) => "text within its length, without control characters or a bearer input",
        }
    }

    /// Whether `v` is a value of this type. Reads `v` in place, with no
    /// copy.
    fn accepts(self, v: &str) -> bool {
        let b = v.as_bytes();
        match self {
            Ty::WalletId => is_hex64(b),
            Ty::Identity => b.len() <= 44 && base58_len(b) == Some(32),
            Ty::EngineId => is_engine_id(b),
            Ty::Artifact => {
                is_hex64(b)
                    || ["registration/", "topup/"].iter().any(|p| {
                        v.strip_prefix(p)
                            .and_then(|r| r.strip_suffix("/funding"))
                            .is_some_and(|id| is_engine_id(id.as_bytes()))
                    })
            }
            Ty::Address => (26..=35).contains(&b.len()) && b.iter().all(|c| BASE58.contains(c)),
            Ty::U64 => is_decimal(b) && v.parse::<u64>().is_ok(),
            Ty::U32 => is_decimal(b) && v.parse::<u32>().is_ok(),
            Ty::Enum(names) => names.contains(&v),
            Ty::Label => is_label(b),
            Ty::LabelText => (1..=63).contains(&b.len()) && b.iter().all(is_label_char),
            Ty::Username => match b.len().checked_sub(5) {
                Some(n) if b[n..].eq_ignore_ascii_case(b".dash") => is_label(&b[..n]),
                _ => is_label(b),
            },
            Ty::Url => {
                let lower = |p: &[u8]| b.len() >= p.len() && b[..p.len()].eq_ignore_ascii_case(p);
                b.len() <= 2048
                    && (lower(b"https://") || lower(b"http://"))
                    && b.iter().all(|c| (0x21..0x7f).contains(c))
                    && !bearer_shaped(b)
            }
            Ty::Path => {
                (1..=4096).contains(&b.len())
                    && b.iter()
                        .all(|c| c.is_ascii_alphanumeric() || b"._/ +@,~-".contains(c))
                    && !bearer_shaped(b)
            }
            Ty::Text(cap) => {
                b.len() <= cap
                    && !v.chars().any(|c| c.is_control() && c != '\n' && c != '\t')
                    && !bearer_shaped(b)
            }
        }
    }
}

const BASE58: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

/// The length of what `s` decodes to as Base58, or `None` if it is not
/// Base58 or decodes to more than 33 bytes. The number is built in a
/// zeroizing buffer on the stack.
fn base58_len(s: &[u8]) -> Option<usize> {
    let mut n = Zeroizing::new([0u8; 33]);
    for c in s {
        let mut carry = BASE58.iter().position(|a| a == c)? as u32;
        for byte in n.iter_mut().rev() {
            carry += u32::from(*byte) * 58;
            *byte = carry as u8;
            carry >>= 8;
        }
        if carry != 0 {
            return None;
        }
    }
    let ones = s.iter().take_while(|&&c| c == b'1').count();
    let zeros = n.iter().take_while(|&&b| b == 0).count();
    Some(ones + n.len() - zeros)
}

fn is_hex64(b: &[u8]) -> bool {
    b.len() == 64 && b.iter().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}

fn is_engine_id(b: &[u8]) -> bool {
    (1..=64).contains(&b.len())
        && b.iter()
            .all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'z' | b'_' | b'-'))
}

fn is_decimal(b: &[u8]) -> bool {
    !b.is_empty() && b.iter().all(u8::is_ascii_digit) && (b == b"0" || b[0] != b'0')
}

fn is_label_char(c: &u8) -> bool {
    c.is_ascii_alphanumeric() || *c == b'-'
}

fn is_label(b: &[u8]) -> bool {
    (3..=63).contains(&b.len())
        && b.iter().all(is_label_char)
        && b[0] != b'-'
        && b[b.len() - 1] != b'-'
}

/// Whether a value looks like a bearer input: the patterns DASHPAY §3.8's
/// log redaction uses (`dashpay://invite`, `dapk=`), a `dash:?` payload and
/// the mobile invitation link's host and key fields. A diagnostic only: the grammar, not
/// this list, keeps credentials away from clap.
fn bearer_shaped(b: &[u8]) -> bool {
    const PATTERNS: [&[u8]; 7] = [
        b"dashpay://invite",
        b"dash:?",
        b"dapk=",
        b"?pk=",
        b"&pk=",
        b"assetlocktx=",
        b"invitations.dashpay",
    ];
    PATTERNS
        .iter()
        .any(|p| b.windows(p.len()).any(|w| w.eq_ignore_ascii_case(p)))
}

/// The options and values a token may be: options that set a value, and
/// flags.
fn is_option(arg: &Arg) -> bool {
    !arg.is_positional()
        && matches!(
            arg.get_action(),
            ArgAction::Set | ArgAction::Append | ArgAction::SetTrue
        )
}

fn takes_value(arg: &Arg) -> bool {
    matches!(arg.get_action(), ArgAction::Set | ArgAction::Append)
}

/// Checks a request's arguments; the refusal names a position and the
/// command's own names, never a token.
pub(super) fn check(args: &[Zeroizing<String>]) -> Result<(), String> {
    static ROOT: OnceLock<Command> = OnceLock::new();
    let mut cmd = ROOT.get_or_init(|| {
        let mut root = RequestArgs::command();
        root.build();
        root
    });
    let mut path = Vec::new();
    let mut positionals = 0;
    let mut tokens = args.iter().map(|a| a.as_str()).enumerate();
    while let Some((i, tok)) = tokens.next() {
        let at = i + 1;
        let here = || format!("`{}`", path.join(" "));
        if let Some(option) = tok.strip_prefix("--") {
            let (name, inline) = match option.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None => (option, None),
            };
            let arg = cmd
                .get_arguments()
                .find(|a| is_option(a) && a.get_long() == Some(name))
                .ok_or_else(|| format!("argument {at} is not an option of {}", here()))?;
            let long = arg.get_long().unwrap_or_default();
            if !takes_value(arg) {
                if inline.is_some() {
                    return Err(format!("argument {at}: --{long} takes no value"));
                }
                continue;
            }
            let value = match inline {
                Some(v) => v,
                None => match tokens.next() {
                    Some((_, v)) if !v.starts_with('-') => v,
                    Some(_) => {
                        return Err(format!(
                            "argument {at}: --{long} needs a value (one that starts with `-` \
                             goes after `=`)"
                        ));
                    }
                    None => return Err(format!("argument {at}: --{long} needs a value")),
                },
            };
            check_value(cmd, arg, value)
                .map_err(|what| format!("argument {at}: --{long} takes {what}"))?;
        } else if tok.starts_with('-') {
            return Err(format!("argument {at} is not an option of {}", here()));
        } else if cmd.has_subcommands() {
            let sub = cmd
                .get_subcommands()
                .find(|s| s.get_name() == tok && s.get_name() != "help")
                .ok_or_else(|| {
                    if path.is_empty() {
                        format!("argument {at} is not a DashPay command")
                    } else {
                        format!("argument {at} is not a command of {}", here())
                    }
                })?;
            path.push(sub.get_name());
            cmd = sub;
            positionals = 0;
        } else {
            let arg = cmd
                .get_positionals()
                .nth(positionals)
                .ok_or_else(|| format!("argument {at} is surplus: {} takes no more", here()))?;
            positionals += 1;
            check_value(cmd, arg, tok).map_err(|what| {
                format!(
                    "argument {at}: <{}> takes {what}",
                    arg.get_id().as_str().to_uppercase()
                )
            })?;
        }
    }
    Ok(())
}

fn check_value(cmd: &Command, arg: &Arg, value: &str) -> Result<(), &'static str> {
    let ty = value_type(cmd.get_name(), arg.get_id().as_str()).ok_or("no value dwcli can check")?;
    if ty.accepts(value) {
        Ok(())
    } else {
        Err(ty.describe())
    }
}

#[cfg(test)]
mod tests {
    use dw_engine::platform::{ActivityFilter, AvatarSize, ContactSection, ContactSort};
    use serde::Serialize;

    use super::*;

    fn args(tokens: &[&str]) -> Vec<Zeroizing<String>> {
        tokens
            .iter()
            .map(|t| Zeroizing::new(t.to_string()))
            .collect()
    }

    /// Every argument that takes a value, in every DashPay command, has a
    /// type, and every positional takes one value.
    #[test]
    fn every_value_has_a_type() {
        fn walk(cmd: &Command, seen: &mut usize) {
            for arg in cmd
                .get_arguments()
                .filter(|a| takes_value(a) || a.is_positional())
            {
                let id = arg.get_id().as_str();
                assert!(
                    value_type(cmd.get_name(), id).is_some(),
                    "{} {id} has no type",
                    cmd.get_name()
                );
                if arg.is_positional() {
                    assert_eq!(arg.get_num_args().map(|n| n.max_values()), Some(1));
                }
                *seen += 1;
            }
            for sub in cmd.get_subcommands().filter(|s| s.get_name() != "help") {
                walk(sub, seen);
            }
        }
        let mut root = RequestArgs::command();
        root.build();
        let mut seen = 0;
        walk(&root, &mut seen);
        assert!(seen > 50, "{seen}");
    }

    /// The enum types list exactly the facade enums' serde names.
    #[test]
    fn enum_types_are_the_facade_names() {
        fn names<T: Serialize>(all: &[T]) -> Vec<String> {
            all.iter()
                .map(|v| serde_json::to_value(v).unwrap().as_str().unwrap().into())
                .collect()
        }
        // An exhaustive match per enum: a new variant fails to compile here.
        let sections = [
            ContactSection::Requests,
            ContactSection::Contacts,
            ContactSection::Pending,
            ContactSection::Hidden,
        ];
        for s in sections {
            match s {
                ContactSection::Requests
                | ContactSection::Contacts
                | ContactSection::Pending
                | ContactSection::Hidden => {}
            }
        }
        let sorts = [
            ContactSort::DisplayName,
            ContactSort::Username,
            ContactSort::DateAdded,
            ContactSort::LastActivity,
        ];
        for s in sorts {
            match s {
                ContactSort::DisplayName
                | ContactSort::Username
                | ContactSort::DateAdded
                | ContactSort::LastActivity => {}
            }
        }
        let filters = [
            ActivityFilter::All,
            ActivityFilter::Sent,
            ActivityFilter::Received,
        ];
        for f in filters {
            match f {
                ActivityFilter::All | ActivityFilter::Sent | ActivityFilter::Received => {}
            }
        }
        let sizes = [AvatarSize::Small, AvatarSize::Large];
        for s in sizes {
            match s {
                AvatarSize::Small | AvatarSize::Large => {}
            }
        }
        assert_eq!(names(&sections), SECTIONS);
        assert_eq!(names(&sorts), SORTS);
        assert_eq!(names(&filters), FILTERS);
        assert_eq!(names(&sizes), SIZES);
    }

    const ID1: &str = "29d2S7vB453rNYFdR5Ycwt7y9haRT5fwVwL9zTmBhfV2";

    #[test]
    fn values_are_checked_by_type() {
        let hex = "ab".repeat(32);
        let upper = hex.to_uppercase();
        let (z64, z65) = ("z".repeat(64), "z".repeat(65));
        let (a63, a64) = ("a".repeat(63), "a".repeat(64));
        let cases: &[(Ty, &[&str], &[&str])] = &[
            (Ty::WalletId, &[&hex], &[&upper, "ab", ""]),
            (
                Ty::Identity,
                &[
                    ID1,
                    "11111111111111111111111111111111",
                    "JEKNVnkbo3jma5nREBBJCDoXFVeKkD56V3xKrvRmWxFG",
                ],
                &[
                    "I",
                    "4uQeVj5tqViQh7yWWGStvkEG1Zmhx6uasJtWCJziofL",
                    "2K3n5t4wSaF5mj27Tw9vStXWLWyRjjiH5Cp3CFLpKVCr1c",
                    "29d2S7vB453rNYFdR5Ycwt7y9haRT5fwVwL9zTmBhfV0",
                    "",
                ],
            ),
            (
                Ty::EngineId,
                &["d1", "a_b-c", &z64],
                &["", "D1", "a/b", &z65],
            ),
            (
                Ty::Artifact,
                &[&hex, "registration/d1/funding", "topup/7/funding"],
                &["registration//funding", "topup/7", "x/7/funding", "AB"],
            ),
            (
                Ty::Address,
                &["yTw4tvqFpDXagzVgq6v2WShbBBUYoMnCXp"],
                &["yA", "0Tw4tvqFpDXagzVgq6v2WShbBBUYoMnCXp"],
            ),
            (
                Ty::U64,
                &["0", "7", "18446744073709551615"],
                &["", "01", "-1", "+1", "1.5", "18446744073709551616"],
            ),
            (Ty::U32, &["4294967295"], &["4294967296"]),
            (Ty::Enum(SIZES), &["small", "large"], &["Small", "huge", ""]),
            (
                Ty::Label,
                &["abc", "a-1", &a63],
                &["ab", "-ab", "ab-", "a_b", "a.b", &a64],
            ),
            (Ty::LabelText, &["a", "-a-"], &["", "a.b", &a64]),
            (
                Ty::Username,
                &["alice", "alice.dash", "Alice.DASH"],
                &[".dash", "a.b.dash"],
            ),
            (
                Ty::Url,
                &["https://x/a.png", "HTTP://x"],
                &[
                    "ftp://x",
                    "https://x y",
                    "dash:?du=a",
                    "javascript:x",
                    "https://invitations.dashpay.io/applink?du=a&assetlocktx=b&pk=c",
                ],
            ),
            (
                Ty::Path,
                &["/tmp/x", "a b/c.d", "~/x-y_z+1@2,3"],
                &[
                    "",
                    "/x?y",
                    "dash:?du=a",
                    "a=b",
                    "/x\n",
                    "/invitations.dashpay.io/x",
                ],
            ),
            (
                Ty::Text(12),
                &["", "Al", "Ünï 😀", "a\nb\tc", "dash: fan"],
                &["1234567890123", "a\rb", "a\u{7}b", "dash:?x", "a?pk=b"],
            ),
        ];
        for (ty, good, bad) in cases {
            for v in *good {
                assert!(ty.accepts(v), "{ty:?} refused {v:?}");
            }
            for v in *bad {
                assert!(!ty.accepts(v), "{ty:?} accepted {v:?}");
            }
        }
    }

    #[test]
    fn bearer_shapes() {
        for s in [
            "dashpay://invite?x",
            "DASH:?du=a&DAPK=b",
            "https://invitations.dashpay.io/applink?du=a&assetlocktx=b&pk=c",
            "x?pk=1",
            "dash:?invite=x",
        ] {
            assert!(bearer_shaped(s.as_bytes()), "{s}");
        }
        for s in ["dashpay://user?id=a&username=b", "alice", "pkg=1"] {
            assert!(!bearer_shaped(s.as_bytes()), "{s}");
        }
    }

    #[test]
    fn requests_follow_the_command_grammar() {
        let w = "ab".repeat(32);
        let wallet_eq = format!("--wallet={w}");
        for ok in [
            &["identity", "list"][..],
            &["identity", "--wallet", &w, "list"],
            &["identity", "list", &wallet_eq],
            &[
                "contact",
                "list",
                "--section",
                "hidden",
                "--section",
                "pending",
            ],
            &["contact", "details", ID1, "--note=-1", "--hidden"],
            &["pay-contact", ID1, "--amount", "5", "--subtract-fee"],
            &["invite", "stash", "--input-file", "/dev/shm/x"],
            &["dashpay", "session"],
            &["identity"],
            &["name", "check", "a_b.c"],
            &["dashpay", "dispatch-status", "registration/d1/funding"],
            &["contact", "list", "--text", "Al", "--sort=username"],
            &["contact", "activity", ID1, "--cursor", "c-2"],
            &[
                "pay-contact",
                ID1,
                "--amount",
                "5",
                "--note",
                "for lunch\nthanks",
            ],
            &[
                "identity",
                "register",
                "--label",
                "alice",
                "--temporary-label",
                "alice-1",
                "--faucet-key",
                "k1",
                "--faucet-proof-file",
                "/dev/shm/p.hex",
            ],
            &[
                "profile",
                "set",
                "--display-name",
                "Al",
                "--public-message",
                "hi",
                "--avatar-file",
                "/dev/shm/a.png",
            ],
        ] {
            assert_eq!(check(&args(ok)), Ok(()), "{ok:?}");
        }
        for (bad, why) in [
            (&["teleport"][..], "argument 1 is not a DashPay command"),
            (
                &["identity", "lisst"],
                "argument 2 is not a command of `identity`",
            ),
            (
                &["identity", "help"],
                "argument 2 is not a command of `identity`",
            ),
            (
                &["invite", "stash", "x"],
                "argument 3 is surplus: `invite stash` takes no more",
            ),
            (
                &["invite", "stash", "--link", "x"],
                "argument 3 is not an option of `invite stash`",
            ),
            (
                &["invite", "stash", "--"],
                "argument 3 is not an option of `invite stash`",
            ),
            (
                &["invite", "stash", "-x"],
                "argument 3 is not an option of `invite stash`",
            ),
            (
                &["invite", "--help"],
                "argument 2 is not an option of `invite`",
            ),
            (
                &["identity", "list", "--spv=yes"],
                "argument 3: --spv takes no value",
            ),
            (
                &["identity", "list", "--wallet"],
                "argument 3: --wallet needs a value",
            ),
            (
                &["contact", "details", ID1, "--note", "-1"],
                "argument 4: --note needs a value (one that starts with `-` goes after `=`)",
            ),
            (
                &["identity", "set-main", "x"],
                "argument 3: <IDENTITY> takes an identity id (Base58 of 32 bytes)",
            ),
            (
                &["contact", "list", "--sort", "bogus"],
                "argument 3: --sort takes one of the listed names",
            ),
        ] {
            assert_eq!(check(&args(bad)), Err(why.to_string()), "{bad:?}");
        }
    }
}
