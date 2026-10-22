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
    /// the identity exists; the link is wiped before that step.
    Invitation {
        link_id: String,
        inviter: Option<String>,
    },
    /// Developer builds: `key` is a `prepare_faucet_lock` key id (the
    /// engine re-derives the private key) and `proof` the faucet's public
    /// `assetLockProof` hex. The outpoint goes to `asset_lock_outpoint`.
    FaucetAssetLock { key: String, proof: String },
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
            Funding::Invitation { link_id, inviter } => match inviter {
                Some(inviter) => {
                    json!({"v": v, "kind": "invitation", "link_id": link_id, "inviter": inviter})
                }
                None => json!({"v": v, "kind": "invitation", "link_id": link_id}),
            },
            Funding::FaucetAssetLock { key, proof } => {
                json!({"v": v, "kind": "faucet_asset_lock", "key": key, "proof": proof})
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
            Some(match value.get("kind")?.as_str()? {
                "core_balance" => Funding::CoreBalance {
                    lock_duffs: value.get("lock_duffs")?.as_u64()?,
                },
                "existing_identity" => Funding::ExistingIdentity,
                "invitation" => Funding::Invitation {
                    link_id: field("link_id")?,
                    inviter: match value.get("inviter") {
                        None => None,
                        Some(inviter) => Some(inviter.as_str()?.to_owned()),
                    },
                },
                "faucet_asset_lock" => Funding::FaucetAssetLock {
                    key: field("key")?,
                    proof: field("proof")?,
                },
                _ => return None,
            })
        })();
        decoded.map_or(Decoded::Unsupported, Decoded::Known)
    }
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
            },
            Funding::Invitation {
                link_id: "abc".into(),
                inviter: Some("Bob1nviter".into()),
            },
            Funding::FaucetAssetLock {
                key: "k1".into(),
                proof: "00ff".into(),
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
            r#"{"v":1,"kind":"platform_addresses"}"#,
            r#"{"v":1,"kind":"core_balance"}"#,
            r#"{"v":1,"kind":"core_balance","lock_duffs":-1}"#,
            r#"{"v":1,"kind":"core_balance","lock_duffs":"1"}"#,
            r#"{"kind":"core_balance","lock_duffs":1}"#,
            r#"{"v":1,"kind":"invitation"}"#,
            r#"{"v":1,"kind":"invitation","link_id":"abc","inviter":7}"#,
            "core_balance",
            "",
            "wallet",
            "{}",
        ] {
            assert_eq!(Funding::decode(text), Decoded::Unsupported, "{text}");
        }
        // DP5-01's rows, before the inviter (DP5-02 D14).
        assert_eq!(
            Funding::decode(r#"{"v":1,"kind":"invitation","link_id":"abc"}"#),
            Decoded::Known(Funding::Invitation {
                link_id: "abc".into(),
                inviter: None
            })
        );
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
