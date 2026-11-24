//! What a `dp_registration` row means (DASHPAY §3.4 "Registration rows",
//! DP1-02): the phase names, the versioned `funding` and `initial_profile`
//! encodings, and the one engine path that writes the asset-lock outpoint
//! when the library tracks a lock. Not part of the facade.
//!
//! The rows travel in `.dwbackup` files between app versions and are
//! imported by column name, so both encodings are versioned and tagged: a
//! newer build reads every older version, and an older build reads a newer
//! row as [`Decoded::Unsupported`], which fails the flow with
//! `registration.unsupported` instead of panicking.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::registration::{InitialProfile, RegistrationPhase};

/// The `funding` encoding this build writes.
pub(crate) const FUNDING_VERSION: u64 = 1;
/// The `initial_profile` encoding this build writes.
pub(crate) const PROFILE_VERSION: u64 = 1;

/// The §3.4 phases as stored in `dp_registration.phase`. `Failed` is not a
/// phase on disk: a failed flow keeps the phase it stopped in, with `error`
/// set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Phase {
    Draft,
    KeysPrepared,
    FundingSent,
    ProofWaiting,
    IdentityRegistered,
    NameRequested,
    NameRegistered,
    Contested,
    ProfileCreated,
    Done,
}

impl Phase {
    pub(crate) const ALL: [Phase; 10] = [
        Phase::Draft,
        Phase::KeysPrepared,
        Phase::FundingSent,
        Phase::ProofWaiting,
        Phase::IdentityRegistered,
        Phase::NameRequested,
        Phase::NameRegistered,
        Phase::Contested,
        Phase::ProfileCreated,
        Phase::Done,
    ];

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Phase::Draft => "draft",
            Phase::KeysPrepared => "keys_prepared",
            Phase::FundingSent => "funding_sent",
            Phase::ProofWaiting => "proof_waiting",
            Phase::IdentityRegistered => "identity_registered",
            Phase::NameRequested => "name_requested",
            Phase::NameRegistered => "name_registered",
            Phase::Contested => "contested",
            Phase::ProfileCreated => "profile_created",
            Phase::Done => "done",
        }
    }

    /// `None` for a phase this build does not know (a newer row).
    pub(crate) fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.as_str() == text)
    }

    pub(crate) fn facade(self) -> RegistrationPhase {
        match self {
            Phase::Draft => RegistrationPhase::Draft,
            Phase::KeysPrepared => RegistrationPhase::KeysPrepared,
            Phase::FundingSent => RegistrationPhase::FundingSent,
            Phase::ProofWaiting => RegistrationPhase::ProofWaiting,
            Phase::IdentityRegistered => RegistrationPhase::IdentityRegistered,
            Phase::NameRequested => RegistrationPhase::NameRequested,
            Phase::NameRegistered => RegistrationPhase::NameRegistered,
            Phase::Contested => RegistrationPhase::Contested,
            Phase::ProfileCreated => RegistrationPhase::ProfileCreated,
            Phase::Done => RegistrationPhase::Done,
        }
    }

    /// Before the asset lock exists: nothing of the flow can be on the
    /// wire yet.
    pub(crate) fn unfunded(self) -> bool {
        self <= Phase::KeysPrepared
    }
}

/// How a flow is funded, as `funding` stores it (never a secret).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Funding {
    /// The locked amount from the accepted quote, so a resume before
    /// `FundingSent` rebuilds the same lock under the grant's cap instead
    /// of re-quoting across it.
    CoreBalance { lock_duffs: u64 },
    /// The identity is in the row's `identity` column only, set at Draft,
    /// so the two cannot disagree after a partial update.
    ExistingIdentity,
    /// The link itself is a bearer credential in the vault's link stash
    /// (DP5-01), never here. `inviter`: the identity the link's username
    /// resolved to at the start (DP5-02 D14), sent a contact request once
    /// the identity exists; the link is wiped before that step. `identity`:
    /// the id the claim creates, from the voucher's outpoint at the start
    /// (D26), the only identity the row adopts. `request_error`: the code
    /// the request to the inviter was refused with for good, written when
    /// that step settles the row `Done` (DP5-03 D8).
    Invitation {
        link_id: String,
        inviter: Option<String>,
        identity: String,
        request_error: Option<String>,
    },
    /// Developer builds: `key` is a `prepare_faucet_lock` key id (the
    /// engine re-derives the private key) and `proof` the faucet's public
    /// `assetLockProof` hex. The outpoint goes to `asset_lock_outpoint`.
    FaucetAssetLock { key: String, proof: String },
    /// Paid from the shielded pool (C-funding F3): the accepted quote's
    /// `denomination` in credits, and `fallback`, the wallet's own Platform
    /// receive address fixed at Draft, where Platform returns the payment
    /// less a penalty if it cannot create the identity (F13). `identity`:
    /// the id the create's notes give, once the call returns it (F11).
    Shielded {
        denomination: u64,
        fallback: String,
        identity: Option<String>,
    },
    /// Paid from the wallet's Platform addresses (C-funding F3): what the
    /// identity receives and what input 0 keeps for the fee, both from the
    /// accepted quote. `plan`: the inputs at their nonces and the identity
    /// they give, written by the funding step before anything is signed.
    PlatformAddresses {
        funding_credits: u64,
        fee_reserve_credits: u64,
        plan: Option<PlatformPlan>,
    },
}

/// What a Platform-funded row may have signed (C-funding F3, F8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlatformPlan {
    /// bech32m address, the nonce it signs at and its claim, in address
    /// order.
    pub(crate) inputs: Vec<(String, u32, u64)>,
    /// The id those inputs and nonces create.
    pub(crate) identity: String,
}

/// A stored value read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Decoded<T> {
    Known(T),
    /// A version or kind this build does not know, or a malformed value:
    /// the flow fails with `registration.unsupported`.
    Unsupported,
}

impl Funding {
    pub(crate) fn encode(&self) -> String {
        let v = FUNDING_VERSION;
        match self {
            Funding::CoreBalance { lock_duffs } => {
                json!({"v": v, "kind": "core_balance", "lock_duffs": lock_duffs})
            }
            Funding::ExistingIdentity => json!({"v": v, "kind": "existing_identity"}),
            Funding::Invitation {
                link_id,
                inviter,
                identity,
                request_error,
            } => {
                let mut value = json!({
                    "v": v, "kind": "invitation", "link_id": link_id, "identity": identity,
                });
                if let Some(inviter) = inviter {
                    value["inviter"] = json!(inviter);
                }
                if let Some(code) = request_error {
                    value["request_error"] = json!(code);
                }
                value
            }
            Funding::FaucetAssetLock { key, proof } => {
                json!({"v": v, "kind": "faucet_asset_lock", "key": key, "proof": proof})
            }
            Funding::Shielded {
                denomination,
                fallback,
                identity,
            } => {
                let mut value = json!({
                    "v": v, "kind": "shielded", "denomination": denomination, "fallback": fallback,
                });
                if let Some(identity) = identity {
                    value["identity"] = json!(identity);
                }
                value
            }
            Funding::PlatformAddresses {
                funding_credits,
                fee_reserve_credits,
                plan,
            } => {
                let mut value = json!({
                    "v": v, "kind": "platform_addresses", "funding_credits": funding_credits,
                    "fee_reserve_credits": fee_reserve_credits,
                });
                if let Some(plan) = plan {
                    let inputs: Vec<Value> = (plan.inputs.iter())
                        .map(|(a, n, c)| json!({"address": a, "nonce": n, "credits": c}))
                        .collect();
                    value["inputs"] = json!(inputs);
                    value["identity"] = json!(plan.identity);
                }
                value
            }
        }
        .to_string()
    }

    /// Reads every version this build knows. Fields a version does not
    /// define are ignored; a missing or mistyped field is `Unsupported`.
    pub(crate) fn decode(text: &str) -> Decoded<Self> {
        let decoded = (|| {
            let value: Value = serde_json::from_str(text).ok()?;
            if value.get("v")?.as_u64()? != 1 {
                return None;
            }
            let field = |name: &str| value.get(name).and_then(Value::as_str).map(str::to_owned);
            // Absent is `None`; present, it must be a string.
            let optional = |name: &str| match value.get(name) {
                None => Some(None),
                Some(v) => v.as_str().map(|s| Some(s.to_owned())),
            };
            Some(match value.get("kind")?.as_str()? {
                "core_balance" => Funding::CoreBalance {
                    lock_duffs: value.get("lock_duffs")?.as_u64()?,
                },
                "existing_identity" => Funding::ExistingIdentity,
                "invitation" => Funding::Invitation {
                    link_id: field("link_id")?,
                    inviter: optional("inviter")?,
                    identity: field("identity")?,
                    request_error: optional("request_error")?,
                },
                "faucet_asset_lock" => Funding::FaucetAssetLock {
                    key: field("key")?,
                    proof: field("proof")?,
                },
                "shielded" => Funding::Shielded {
                    denomination: value.get("denomination")?.as_u64()?,
                    fallback: field("fallback")?,
                    identity: optional("identity")?,
                },
                "platform_addresses" => Funding::PlatformAddresses {
                    funding_credits: value.get("funding_credits")?.as_u64()?,
                    fee_reserve_credits: value.get("fee_reserve_credits")?.as_u64()?,
                    plan: match (value.get("inputs"), optional("identity")?) {
                        (None, None) => None,
                        (Some(inputs), Some(identity)) => Some(PlatformPlan {
                            inputs: decode_inputs(inputs)?,
                            identity,
                        }),
                        _ => return None,
                    },
                },
                _ => return None,
            })
        })();
        decoded.map_or(Decoded::Unsupported, Decoded::Known)
    }
}

/// A plan's inputs: at least one, each address with a 32-bit nonce.
fn decode_inputs(value: &Value) -> Option<Vec<(String, u32, u64)>> {
    let inputs = (value.as_array()?.iter())
        .map(|i| {
            let address = i.get("address")?.as_str()?.to_owned();
            let nonce = u32::try_from(i.get("nonce")?.as_u64()?).ok()?;
            Some((address, nonce, i.get("credits")?.as_u64()?))
        })
        .collect::<Option<Vec<_>>>()?;
    (!inputs.is_empty()).then_some(inputs)
}

/// The profile entered at Draft, as `initial_profile` stores it: the avatar
/// by URL, hash and fingerprint (never a candidate id, which dies with the
/// session), nothing secret.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ProfileRecord {
    pub(crate) v: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) public_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) avatar_url: Option<String>,
    /// SHA-256 of the avatar's bytes, hex.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) avatar_hash: Option<String>,
    /// The dHash fingerprint, 16 hex digits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) avatar_fingerprint: Option<String>,
}

impl ProfileRecord {
    /// The record of `profile`, without an avatar: a candidate is resolved
    /// to its URL by DP4-02's `prepare_avatar`, and until then a profile
    /// naming one is refused before anything is stored.
    pub(crate) fn without_avatar(profile: &InitialProfile) -> Self {
        Self {
            v: PROFILE_VERSION,
            display_name: profile.display_name.clone(),
            public_message: profile.public_message.clone(),
            ..Self::default()
        }
    }

    pub(crate) fn encode(&self) -> String {
        serde_json::to_string(self).expect("a profile record serializes")
    }

    pub(crate) fn decode(text: &str) -> Decoded<Self> {
        match serde_json::from_str::<Self>(text) {
            Ok(record) if record.v == 1 => Decoded::Known(record),
            _ => Decoded::Unsupported,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phases_round_trip_and_order() {
        for p in Phase::ALL {
            assert_eq!(Phase::parse(p.as_str()), Some(p));
        }
        assert_eq!(Phase::parse("failed"), None, "not a stored phase");
        assert_eq!(Phase::parse("verified_on_mars"), None);
        assert!(Phase::KeysPrepared.unfunded() && !Phase::FundingSent.unfunded());
    }

    #[test]
    fn funding_is_versioned_and_tagged() {
        let all = [
            Funding::CoreBalance {
                lock_duffs: 3_000_000,
            },
            Funding::ExistingIdentity,
            Funding::Invitation {
                link_id: "abc".into(),
                inviter: None,
                identity: "Carol1dentity".into(),
                request_error: None,
            },
            Funding::Invitation {
                link_id: "abc".into(),
                inviter: Some("Bob1nviter".into()),
                identity: "Carol1dentity".into(),
                request_error: None,
            },
            Funding::Invitation {
                link_id: "abc".into(),
                inviter: Some("Bob1nviter".into()),
                identity: "Carol1dentity".into(),
                request_error: Some("contact.no_dashpay_keys".into()),
            },
            Funding::FaucetAssetLock {
                key: "k1".into(),
                proof: "00ff".into(),
            },
            Funding::Shielded {
                denomination: 3_000_000_000,
                fallback: "tdash1qown".into(),
                identity: None,
            },
            Funding::Shielded {
                denomination: 3_000_000_000,
                fallback: "tdash1qown".into(),
                identity: Some("Carol1dentity".into()),
            },
            Funding::PlatformAddresses {
                funding_credits: 25_000_000,
                fee_reserve_credits: 64_500_000,
                plan: None,
            },
            Funding::PlatformAddresses {
                funding_credits: 25_000_000,
                fee_reserve_credits: 64_500_000,
                plan: Some(PlatformPlan {
                    inputs: vec![
                        ("tdash1qa".into(), 4, 20_000_000),
                        ("tdash1qb".into(), 1, 5_000_000),
                    ],
                    identity: "Carol1dentity".into(),
                }),
            },
        ];
        for f in all {
            let text = f.encode();
            let value: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(value["v"], 1, "{text}");
            assert!(value["kind"].is_string(), "{text}");
            assert_eq!(Funding::decode(&text), Decoded::Known(f));
        }
        // ExistingIdentity never carries the id: the `identity` column does.
        let value: Value = serde_json::from_str(&Funding::ExistingIdentity.encode()).unwrap();
        assert_eq!(value, json!({"v": 1, "kind": "existing_identity"}));
    }

    #[test]
    fn a_newer_or_malformed_funding_is_refused_cleanly() {
        for text in [
            r#"{"v":2,"kind":"core_balance","lock_duffs":1}"#,
            r#"{"v":1,"kind":"platform_credit_line"}"#,
            // A Platform row always names its funding and reserve, and a
            // plan its inputs and identity together.
            r#"{"v":1,"kind":"platform_addresses"}"#,
            r#"{"v":1,"kind":"platform_addresses","funding_credits":1}"#,
            r#"{"v":1,"kind":"platform_addresses","funding_credits":1,"fee_reserve_credits":1,"identity":"C"}"#,
            r#"{"v":1,"kind":"platform_addresses","funding_credits":1,"fee_reserve_credits":1,"inputs":[]}"#,
            r#"{"v":1,"kind":"platform_addresses","funding_credits":1,"fee_reserve_credits":1,"inputs":[],"identity":"C"}"#,
            r#"{"v":1,"kind":"platform_addresses","funding_credits":1,"fee_reserve_credits":1,"inputs":[{"address":"a","nonce":4294967296,"credits":1}],"identity":"C"}"#,
            r#"{"v":1,"kind":"platform_addresses","funding_credits":1,"fee_reserve_credits":1,"inputs":[{"address":"a","credits":1}],"identity":"C"}"#,
            r#"{"v":1,"kind":"core_balance"}"#,
            r#"{"v":1,"kind":"core_balance","lock_duffs":-1}"#,
            r#"{"v":1,"kind":"core_balance","lock_duffs":"1"}"#,
            r#"{"kind":"core_balance","lock_duffs":1}"#,
            r#"{"v":1,"kind":"invitation"}"#,
            r#"{"v":1,"kind":"invitation","link_id":"abc","identity":"C","inviter":7}"#,
            r#"{"v":1,"kind":"invitation","link_id":"abc","identity":7}"#,
            r#"{"v":1,"kind":"invitation","link_id":"abc","identity":"C","request_error":1}"#,
            // No build wrote an invitation row without its identity (D26):
            // DP1-02 refused invitations at the start.
            r#"{"v":1,"kind":"invitation","link_id":"abc"}"#,
            // A shielded row always names its denomination and fallback.
            r#"{"v":1,"kind":"shielded","fallback":"tdash1qown"}"#,
            r#"{"v":1,"kind":"shielded","denomination":1}"#,
            r#"{"v":1,"kind":"shielded","denomination":1,"fallback":"f","identity":7}"#,
            "core_balance",
            "",
            "wallet",
            "{}",
        ] {
            assert_eq!(Funding::decode(text), Decoded::Unsupported, "{text}");
        }
        // A newer build's extra field within version 1 is ignored.
        assert_eq!(
            Funding::decode(r#"{"v":1,"kind":"core_balance","lock_duffs":5,"note":"x"}"#),
            Decoded::Known(Funding::CoreBalance { lock_duffs: 5 })
        );
    }

    #[test]
    fn profiles_are_versioned() {
        let record = ProfileRecord::without_avatar(&InitialProfile {
            display_name: Some("Alice".into()),
            public_message: None,
            avatar_candidate: Some("ignored".into()),
        });
        let text = record.encode();
        assert_eq!(text, r#"{"v":1,"display_name":"Alice"}"#);
        assert_eq!(ProfileRecord::decode(&text), Decoded::Known(record));
        assert_eq!(
            ProfileRecord::decode(r#"{"v":2,"display_name":"A"}"#),
            Decoded::Unsupported
        );
        assert_eq!(ProfileRecord::decode("{}"), Decoded::Unsupported);
    }
}
