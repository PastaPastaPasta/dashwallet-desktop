//! The corpus for `bl190_avatar_fingerprints.gen.rs` (BL-190, DP4-03 D27): pages testnet's
//! DashPay profiles (read only), saves each avatar's bytes and its published hash and
//! fingerprint under `$OUT` with an `index.tsv`. Kept beside the fixture; to run it, copy it
//! to `examples/` and `OUT=<dir> cargo run -p dw-engine --example bl190_corpus`.

use std::num::NonZeroUsize;
use std::sync::Arc;

use dash_sdk::SdkBuilder;
use dash_sdk::drive::query::{OrderClause, WhereClause, WhereOperator};
use dash_sdk::platform::{DataContract, DocumentQuery, Fetch, FetchMany};
use dpp::document::{Document, DocumentV0Getters};
use dpp::platform_value::{Value, platform_value};
use dpp::prelude::Identifier;
use dpp::system_data_contracts::dashpay_contract::ID as DASHPAY_ID;
use rs_sdk_trusted_context_provider::TrustedHttpContextProvider;
use sha2::Digest;

#[tokio::main]
async fn main() {
    let out = std::env::var("OUT").expect("OUT");
    std::fs::create_dir_all(&out).unwrap();
    let provider = TrustedHttpContextProvider::new(
        dashcore::Network::Testnet,
        None,
        NonZeroUsize::new(100).unwrap(),
    )
    .unwrap();
    let sdk = SdkBuilder::new_testnet()
        .with_context_provider(provider)
        .build()
        .unwrap();
    let contract = Arc::new(
        DataContract::fetch(&sdk, DASHPAY_ID)
            .await
            .unwrap()
            .expect("contract"),
    );
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap();
    let mut index = String::new();
    let mut last: Option<Identifier> = None;
    let mut total = 0;
    loop {
        let mut q = DocumentQuery::new(contract.clone(), "profile")
            .unwrap()
            .with_order_by(OrderClause {
                field: "$ownerId".into(),
                ascending: true,
            })
            .with_limit(100);
        if let Some(l) = last {
            q = q.with_where(WhereClause {
                field: "$ownerId".into(),
                operator: WhereOperator::GreaterThan,
                value: platform_value!(l),
            });
        }
        let docs = Document::fetch_many(&sdk, q).await.unwrap();
        let docs: Vec<Document> = docs.into_values().flatten().collect();
        if docs.is_empty() {
            break;
        }
        for d in &docs {
            total += 1;
            let owner = d.owner_id();
            last = Some(if last.is_none_or(|l| owner > l) { owner } else { last.unwrap() });
            let p = d.properties();
            let Some(Value::Text(url)) = p.get("avatarUrl") else {
                continue;
            };
            let bytes = |k: &str| match p.get(k) {
                Some(Value::Bytes(b)) => hex::encode(b),
                Some(Value::Bytes32(b)) => hex::encode(b),
                Some(Value::Bytes20(b)) => hex::encode(b),
                Some(v) => format!("?{v:?}"),
                None => "-".into(),
            };
            let (hash, fp) = (bytes("avatarHash"), bytes("avatarFingerprint"));
            let name = owner.to_string(dpp::platform_value::string_encoding::Encoding::Base58);
            let (status, file, sha) = match http.get(url.as_str()).send().await {
                Ok(r) => {
                    let s = r.status().as_u16();
                    match r.bytes().await {
                        Ok(b) if s == 200 => {
                            let f = format!("{out}/{name}.bin");
                            std::fs::write(&f, &b).unwrap();
                            (s.to_string(), f, hex::encode(sha2::Sha256::digest(&b)))
                        }
                        _ => (s.to_string(), "-".into(), "-".into()),
                    }
                }
                Err(e) => (format!("err:{e}").replace('\t', " "), "-".into(), "-".into()),
            };
            let ua = d.updated_at().map(|t| t.to_string()).unwrap_or("-".into());
            index.push_str(&format!(
                "{name}\t{url}\t{hash}\t{fp}\t{status}\t{sha}\t{file}\t{ua}\n"
            ));
            eprintln!("{name} {status} {}", hash == sha);
        }
        eprintln!("page; total {total}");
    }
    std::fs::write(format!("{out}/index.tsv"), index).unwrap();
    eprintln!("profiles {total}");
}
