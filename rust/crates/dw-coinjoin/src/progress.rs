//! dash-qt's CoinJoin progress (QT-042): `OverviewPage::updateCoinJoinProgress`,
//! `src/qt/overviewpage.cpp` (v24.0.0-rc.2). The arithmetic is done in
//! `f32` with `ceilf`, as dash-qt does, so the values match its tooltip
//! digit for digit. Golden vectors: `testdata/coinjoin_progress.json`.

/// 1 DASH in duffs.
const COIN: u64 = 100_000_000;

/// The balances the formula reads, duffs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProgressInputs {
    /// Whole wallet balance (`getCachedBalance().balance`); 0 short-cuts to
    /// "No inputs detected".
    pub balance: u64,
    /// `getAnonymizableBalance(false, false)`.
    pub anonymizable: u64,
    /// `anonymized_balance` (fully mixed).
    pub anonymized: u64,
    /// `denominated_trusted + denominated_untrusted_pending`.
    pub denominated: u64,
    /// `getNormalizedAnonymizedBalance()`.
    pub normalized_anonymized: u64,
    /// Options: mixing rounds.
    pub rounds: u32,
    /// Options: target amount, whole DASH.
    pub target_dash: u32,
}

/// The progress and its tooltip parts, percent.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Progress {
    pub overall: f32,
    pub denominated: f32,
    pub partially_mixed: f32,
    pub mixed: f32,
    /// `min(anonymizable + anonymized, target)`; 0 when nothing to mix
    /// ("No inputs detected").
    pub max_to_anonymize: u64,
}

/// `nMaxToAnonymize` (overviewpage.cpp: `nAnonymizableBalance +
/// anonymized_balance`, limited to the target amount).
pub fn max_to_anonymize(anonymizable: u64, anonymized: u64, target_dash: u32) -> u64 {
    (anonymizable + anonymized).min(u64::from(target_dash) * COIN)
}

/// The formula. With a zero balance or nothing to anonymize every value is
/// 0, as dash-qt shows an empty bar.
pub fn progress(i: &ProgressInputs) -> Progress {
    if i.balance == 0 {
        return Progress::default();
    }
    let max = max_to_anonymize(i.anonymizable, i.anonymized, i.target_dash);
    if max == 0 {
        return Progress::default();
    }
    let part = |x: u64| -> f32 {
        let p = x as f32 / max as f32;
        (if p > 1.0 { 1.0 } else { p }) * 100.0
    };
    let denom_part = part(i.denominated);
    let anon_norm_part = part(i.normalized_anonymized);
    let anon_full_part = part(i.anonymized);

    let denom_weight: f32 = 1.0;
    let anon_norm_weight = i.rounds as f32;
    let anon_full_weight: f32 = 2.0;
    let full_weight = denom_weight + anon_norm_weight + anon_full_weight;
    let calc = |p: f32, w: f32| ((p * w / full_weight) * 100.0).ceil() / 100.0;
    let mut overall = calc(denom_part, denom_weight)
        + calc(anon_norm_part, anon_norm_weight)
        + calc(anon_full_part, anon_full_weight);
    if overall >= 100.0 {
        overall = 100.0;
    }
    Progress {
        overall,
        denominated: denom_part,
        partially_mixed: anon_norm_part,
        mixed: anon_full_part,
        max_to_anonymize: max,
    }
}

/// `GetAverageAnonymizedRounds` (wallet/coinjoin.cpp:535-558): the mean of
/// the capped rounds of the denominated coins, `f32` like Core.
pub fn average_rounds(capped_rounds: &[i32]) -> f32 {
    if capped_rounds.is_empty() {
        return 0.0;
    }
    let total: i32 = capped_rounds.iter().sum();
    total as f32 / capped_rounds.len() as f32
}

/// `GetNormalizedAnonymizedBalance` (wallet/coinjoin.cpp:560-580):
/// Σ `value · capped_rounds / rounds` with integer division per coin.
/// Negative capped rounds (not denominated) are never passed in.
pub fn normalized_anonymized(coins: &[(u64, i32)], rounds: u32) -> u64 {
    coins
        .iter()
        .map(|(value, r)| {
            let r = (*r).max(0) as u64;
            value * r / u64::from(rounds.max(1))
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One vector of `testdata/coinjoin_progress.json`.
    #[derive(Debug)]
    struct Vector {
        name: String,
        inputs: ProgressInputs,
        overall: f32,
        denominated: f32,
        partially_mixed: f32,
        mixed: f32,
    }

    /// The golden file is hand-written JSON with flat objects; parsed
    /// here without a JSON dependency.
    fn parse(text: &str) -> Vec<Vector> {
        let field = |obj: &str, key: &str| -> String {
            let pat = format!("\"{key}\":");
            let start = obj.find(&pat).unwrap_or_else(|| panic!("{key} missing")) + pat.len();
            let rest = obj[start..].trim_start();
            if let Some(stripped) = rest.strip_prefix('"') {
                stripped[..stripped.find('"').unwrap()].to_string()
            } else {
                rest[..rest.find([',', '}']).unwrap()].trim().to_string()
            }
        };
        text.split("{\"name\"")
            .skip(1)
            .map(|chunk| {
                let obj = format!("{{\"name\"{}", &chunk[..chunk.find('}').unwrap() + 1]);
                let u = |k: &str| field(&obj, k).parse::<u64>().unwrap();
                let f = |k: &str| field(&obj, k).parse::<f32>().unwrap();
                Vector {
                    name: field(&obj, "name"),
                    inputs: ProgressInputs {
                        balance: u("balance"),
                        anonymizable: u("anonymizable"),
                        anonymized: u("anonymized"),
                        denominated: u("denominated"),
                        normalized_anonymized: u("normalized_anonymized"),
                        rounds: u("rounds") as u32,
                        target_dash: u("target_dash") as u32,
                    },
                    overall: f("overall"),
                    denominated: f("denominated_percent"),
                    partially_mixed: f("partially_mixed_percent"),
                    mixed: f("mixed_percent"),
                }
            })
            .collect()
    }

    #[test]
    fn test_qt_042_progress_matches_dash_qt_golden_vectors() {
        let text = include_str!("../../../../testdata/coinjoin_progress.json");
        let vectors = parse(text);
        assert!(
            vectors.len() >= 8,
            "golden file has {} vectors",
            vectors.len()
        );
        for v in vectors {
            let p = progress(&v.inputs);
            assert_eq!(p.overall, v.overall, "{}: overall", v.name);
            assert_eq!(p.denominated, v.denominated, "{}: denominated", v.name);
            assert_eq!(p.partially_mixed, v.partially_mixed, "{}: partial", v.name);
            assert_eq!(p.mixed, v.mixed, "{}: mixed", v.name);
        }
    }

    #[test]
    fn normalized_and_average_rounds() {
        // 1 DASH at 2 of 4 rounds counts half; 0.1 at 4 counts fully.
        let coins = [(100_001_000, 2), (10_000_100, 4)];
        assert_eq!(normalized_anonymized(&coins, 4), 50_000_500 + 10_000_100);
        assert_eq!(average_rounds(&[2, 4, 3]), 3.0);
        assert_eq!(average_rounds(&[]), 0.0);
    }
}
