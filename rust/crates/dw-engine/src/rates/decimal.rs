//! Prices as exact decimals: an integer mantissa and a power-of-ten scale.
//! A price is fiat per one DASH, stored and passed on as the decimal string
//! [`Price::to_string`] gives; the renderer converts amounts with it
//! (`apps/desktop/src/format/fiat.ts`), so nothing here rounds an amount.

use std::cmp::Ordering;
use std::fmt;

/// At most this many digits before the decimal point.
pub(crate) const MAX_INT_DIGITS: usize = 15;
/// At most this many after it.
pub(crate) const MAX_SCALE: u32 = 12;

/// A positive decimal `mantissa / 10^scale`, normalised: no trailing zero
/// in the fraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Price {
    mantissa: u128,
    scale: u32,
}

impl Price {
    /// `^[0-9]{1,15}(\.[0-9]{1,12})?$` and above zero; anything else
    /// (signs, exponents, spaces, more digits) is `None`.
    pub(crate) fn parse(text: &str) -> Option<Price> {
        let (int, frac) = match text.split_once('.') {
            Some((i, f)) => (i, Some(f)),
            None => (text, None),
        };
        if int.is_empty() || int.len() > MAX_INT_DIGITS || !int.bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        let frac = frac.unwrap_or("");
        if frac.len() > MAX_SCALE as usize
            || !frac.bytes().all(|b| b.is_ascii_digit())
            || (text.contains('.') && frac.is_empty())
        {
            return None;
        }
        let digits = format!("{int}{frac}");
        let mantissa: u128 = digits.parse().ok()?;
        Price::new(mantissa, frac.len() as u32)
    }

    /// A JSON number's text (`^[0-9]{1,15}(\.[0-9]+)?$`), rounded half to
    /// even to [`MAX_SCALE`] decimals: a float such as
    /// `49.158414369999996` carries more digits than a price keeps.
    pub(crate) fn parse_rounded(text: &str) -> Option<Price> {
        let (int, frac) = text.split_once('.').unwrap_or((text, ""));
        if text.contains('.') && frac.is_empty() || !frac.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        if frac.len() <= MAX_SCALE as usize {
            return Price::parse(text);
        }
        if int.is_empty() || int.len() > MAX_INT_DIGITS || !int.bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        let (kept, rest) = frac.split_at(MAX_SCALE as usize);
        // Only an all-zero head fails to parse here: it may still round up to the last place.
        let kept = Price::parse(&format!("{int}.{kept}"))
            .map(|p| p.mantissa * 10u128.pow(MAX_SCALE - p.scale))
            .unwrap_or(0);
        let first = rest.as_bytes()[0] - b'0';
        let sticky = rest.bytes().skip(1).any(|b| b != b'0');
        let up = first > 5 || (first == 5 && (sticky || kept & 1 == 1));
        Price::new(kept + u128::from(up), MAX_SCALE)
    }

    /// `mantissa / 10^scale`, normalised; `None` for zero or out of range.
    pub(crate) fn new(mut mantissa: u128, mut scale: u32) -> Option<Price> {
        if mantissa == 0 {
            return None;
        }
        while scale > 0 && mantissa.is_multiple_of(10) {
            mantissa /= 10;
            scale -= 1;
        }
        let p = Price { mantissa, scale };
        if scale > MAX_SCALE || p.int_digits() > MAX_INT_DIGITS {
            return None;
        }
        Some(p)
    }

    fn int_digits(&self) -> usize {
        let int = self.mantissa / 10u128.pow(self.scale);
        int.to_string().len()
    }

    /// `self × other`, rounded half to even to [`MAX_SCALE`] decimals.
    pub(crate) fn mul(self, other: Price) -> Option<Price> {
        let m = self.mantissa.checked_mul(other.mantissa)?;
        rescale(m, self.scale + other.scale)
    }

    /// `self / other`, rounded half to even to [`MAX_SCALE`] decimals.
    pub(crate) fn div(self, other: Price) -> Option<Price> {
        // self.m / 10^s1 / (o.m / 10^s2) = self.m × 10^(s2 + 12 − s1) / o.m, at scale 12.
        let shift = (other.scale + MAX_SCALE).checked_sub(self.scale)?;
        let num = self.mantissa.checked_mul(10u128.checked_pow(shift)?)?;
        Price::new(div_half_even(num, other.mantissa), MAX_SCALE)
    }

    /// `self / other` compared with `num / den` (exact, by cross products).
    pub(crate) fn ratio_cmp(self, other: Price, num: u128, den: u128) -> Option<Ordering> {
        // self.m × 10^s2 × den  vs  other.m × 10^s1 × num
        let left = self
            .mantissa
            .checked_mul(10u128.checked_pow(other.scale)?)?
            .checked_mul(den)?;
        let right = other
            .mantissa
            .checked_mul(10u128.checked_pow(self.scale)?)?
            .checked_mul(num)?;
        Some(left.cmp(&right))
    }

    /// `|self − other| / other > num / den`.
    pub(crate) fn moved_more_than(self, other: Price, num: u128, den: u128) -> Option<bool> {
        let up = self.ratio_cmp(other, den + num, den)? == Ordering::Greater;
        let down = match den.checked_sub(num) {
            Some(lo) if lo > 0 => self.ratio_cmp(other, lo, den)? == Ordering::Less,
            _ => false,
        };
        Some(up || down)
    }
}

/// `m / 10^scale` rounded half to even to at most [`MAX_SCALE`] decimals.
fn rescale(m: u128, scale: u32) -> Option<Price> {
    if scale <= MAX_SCALE {
        return Price::new(m, scale);
    }
    let div = 10u128.checked_pow(scale - MAX_SCALE)?;
    Price::new(div_half_even(m, div), MAX_SCALE)
}

fn div_half_even(n: u128, d: u128) -> u128 {
    let q = n / d;
    let r = n % d;
    match (r * 2).cmp(&d) {
        Ordering::Less => q,
        Ordering::Greater => q + 1,
        Ordering::Equal => q + (q & 1),
    }
}

impl fmt::Display for Price {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let p = 10u128.pow(self.scale);
        let int = self.mantissa / p;
        if self.scale == 0 {
            return write!(f, "{int}");
        }
        let frac = self.mantissa % p;
        write!(f, "{int}.{frac:0width$}", width = self.scale as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Price {
        Price::parse(s).unwrap_or_else(|| panic!("{s}"))
    }

    #[test]
    fn parse_accepts_plain_positive_decimals_only() {
        assert_eq!(p("49.0967").to_string(), "49.0967");
        assert_eq!(p("0.00062134").to_string(), "0.00062134");
        assert_eq!(p("7873.9500").to_string(), "7873.95");
        assert_eq!(p("12").to_string(), "12");
        assert_eq!(
            p("999999999999999.999999999999").to_string(),
            "999999999999999.999999999999"
        );
        for bad in [
            "",
            "0",
            "0.000",
            "-1",
            "+1",
            "1e3",
            "1.",
            ".5",
            " 1",
            "1 ",
            "1,5",
            "NaN",
            "1.0000000000001",
            "1000000000000000",
            "0x10",
        ] {
            assert_eq!(Price::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn json_numbers_round_half_to_even_to_twelve_decimals() {
        let r = |s: &str| Price::parse_rounded(s).map(|p| p.to_string());
        assert_eq!(r("49.158414369999996").as_deref(), Some("49.15841437"));
        assert_eq!(r("49.27936182").as_deref(), Some("49.27936182"));
        assert_eq!(r("1.0000000000005").as_deref(), Some("1"));
        assert_eq!(r("1.0000000000015").as_deref(), Some("1.000000000002"));
        assert_eq!(r("1.00000000000050001").as_deref(), Some("1.000000000001"));
        assert_eq!(r("0.0000000000004"), None, "rounds to zero");
        assert_eq!(r("1e-7"), None);
        assert_eq!(r("-1.5"), None);
        assert_eq!(r("80000").as_deref(), Some("80000"));
    }

    #[test]
    fn mul_and_div_round_half_to_even_at_twelve_decimals() {
        assert_eq!(p("2.5").mul(p("4")).unwrap().to_string(), "10");
        assert_eq!(p("1").div(p("3")).unwrap().to_string(), "0.333333333333");
        assert_eq!(p("2").div(p("3")).unwrap().to_string(), "0.666666666667");
        // 0.0000000000005 → ties go to the even neighbour.
        assert_eq!(p("0.000001").mul(p("0.0000005")), None, "rounds to zero");
        assert_eq!(
            p("0.000003").mul(p("0.0000005")).unwrap().to_string(),
            "0.000000000002"
        );
        assert_eq!(
            p("0.000001").mul(p("0.0000015")).unwrap().to_string(),
            "0.000000000002"
        );
        assert_eq!(
            p("0.000001").mul(p("0.0000025")).unwrap().to_string(),
            "0.000000000002"
        );
    }

    #[test]
    fn moves_compare_exactly() {
        assert_eq!(p("150").moved_more_than(p("100"), 1, 2), Some(false));
        assert_eq!(p("150.01").moved_more_than(p("100"), 1, 2), Some(true));
        assert_eq!(p("50").moved_more_than(p("100"), 1, 2), Some(false));
        assert_eq!(p("49.99").moved_more_than(p("100"), 1, 2), Some(true));
        // A tenfold jump either way.
        assert_eq!(p("1000").ratio_cmp(p("100"), 10, 1), Some(Ordering::Equal));
        assert_eq!(p("9.99").ratio_cmp(p("100"), 1, 10), Some(Ordering::Less));
    }
}
