//! Port of Dash Core's `bech32::LocateErrors` (`src/bech32.cpp`, from
//! Bitcoin Core): the error message for an invalid bech32/bech32m string
//! and the positions of the characters that are probably wrong. dash-qt
//! uses the positions to highlight typos in the address field.
//!
//! Up to two substitution errors are located by solving for them from the
//! BCH syndromes in GF(1024); the arithmetic tables are generated the same
//! way Core generates them.

use std::sync::OnceLock;

const CHARSET_REV: [i8; 128] = [
    -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
    -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
    15, -1, 10, 17, 21, 20, 26, 30, 7, 5, -1, -1, -1, -1, -1, -1, -1, 29, -1, 24, 13, 25, 9, 8, 23,
    -1, 18, 22, 31, 27, 19, -1, 1, 0, 3, 16, 11, 28, 12, 14, 6, 4, 2, -1, -1, -1, -1, -1, -1, 29,
    -1, 24, 13, 25, 9, 8, 23, -1, 18, 22, 31, 27, 19, -1, 1, 0, 3, 16, 11, 28, 12, 14, 6, 4, 2, -1,
    -1, -1, -1, -1,
];

const CHECKSUM_SIZE: usize = 6;
/// BIP173/350 character limit for addresses.
const CHAR_LIMIT: usize = 90;
const BECH32_CONST: u32 = 1;
const BECH32M_CONST: u32 = 0x2bc8_30a3;

struct Tables {
    exp: [i32; 1023],
    log: [i32; 1024],
    syndrome: [u32; 25],
}

/// `GenerateGFTables` and `GenerateSyndromeConstants`. The index loops fill
/// the EXP and LOG tables together, as in Core.
#[allow(clippy::needless_range_loop)]
fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let mut gf32_exp = [0i32; 31];
        let mut gf32_log = [0i32; 32];
        gf32_exp[0] = 1;
        gf32_log[0] = -1;
        gf32_log[1] = 0;
        let mut v = 1i32;
        for i in 1..31 {
            v <<= 1;
            if v & 32 != 0 {
                v ^= 41; // x^5 + x^3 + 1
            }
            gf32_exp[i] = v;
            gf32_log[v as usize] = i as i32;
        }
        let mul = |a: i32, log_b: i32| {
            if a == 0 {
                0
            } else {
                gf32_exp[((gf32_log[a as usize] + log_b) % 31) as usize]
            }
        };
        let mut exp = [0i32; 1023];
        let mut log = [0i32; 1024];
        exp[0] = 1;
        log[0] = -1;
        log[1] = 0;
        let mut v = 1i32;
        for i in 1..1023 {
            let (v0, v1) = (v & 31, v >> 5);
            let v0n = mul(v1, gf32_log[23]);
            let v1n = mul(v1, gf32_log[9]) ^ v0;
            v = (v1n << 5) | v0n;
            exp[i] = v;
            log[v as usize] = i as i32;
        }
        let mut syndrome = [0u32; 25];
        for k in 1..6 {
            for shift in 0..5 {
                let b = log[1 << shift];
                let c0 = exp[((997 * k + b) % 1023) as usize] as u32;
                let c1 = exp[((998 * k + b) % 1023) as usize] as u32;
                let c2 = exp[((999 * k + b) % 1023) as usize] as u32;
                syndrome[(5 * (k - 1) + shift) as usize] = (c2 << 20) | (c1 << 10) | c0;
            }
        }
        Tables { exp, log, syndrome }
    })
}

fn polymod(values: &[u8]) -> u32 {
    let mut c: u32 = 1;
    for &v in values {
        let c0 = (c >> 25) as u8;
        c = ((c & 0x1ff_ffff) << 5) ^ u32::from(v);
        if c0 & 1 != 0 {
            c ^= 0x3b6a_57b2;
        }
        if c0 & 2 != 0 {
            c ^= 0x2650_8e6d;
        }
        if c0 & 4 != 0 {
            c ^= 0x1ea1_19fa;
        }
        if c0 & 8 != 0 {
            c ^= 0x3d42_33dd;
        }
        if c0 & 16 != 0 {
            c ^= 0x2a14_62b3;
        }
    }
    c
}

fn syndrome(residue: u32) -> u32 {
    let t = tables();
    let low = residue & 0x1f;
    let mut result = low ^ (low << 10) ^ (low << 20);
    for i in 0..25 {
        if (residue >> (5 + i)) & 1 != 0 {
            result ^= t.syndrome[i];
        }
    }
    result
}

/// `bech32::LocateErrors(str)`: `("", [])` when the string is a valid
/// bech32 or bech32m string, else Core's message and error positions.
pub(crate) fn locate_errors(s: &str) -> (&'static str, Vec<usize>) {
    let b = s.as_bytes();
    if b.len() > CHAR_LIMIT {
        return ("Bech32 string too long", (CHAR_LIMIT..b.len()).collect());
    }
    // CheckCharacters
    let mut errors = Vec::new();
    let (mut lower, mut upper) = (false, false);
    for (i, &c) in b.iter().enumerate() {
        if c.is_ascii_lowercase() {
            if upper {
                errors.push(i);
            } else {
                lower = true;
            }
        } else if c.is_ascii_uppercase() {
            if lower {
                errors.push(i);
            } else {
                upper = true;
            }
        } else if !(33..=126).contains(&c) {
            errors.push(i);
        }
    }
    if !errors.is_empty() {
        return ("Invalid character or mixed case", errors);
    }
    let Some(pos) = s.rfind('1') else {
        return ("Missing separator", Vec::new());
    };
    if pos == 0 || pos + CHECKSUM_SIZE >= b.len() {
        return ("Invalid separator position", vec![pos]);
    }
    let hrp: Vec<u8> = b[..pos].iter().map(u8::to_ascii_lowercase).collect();
    let length = b.len() - 1 - pos;
    let mut values = Vec::with_capacity(length);
    for (i, &c) in b.iter().enumerate().skip(pos + 1) {
        let rev = CHARSET_REV[usize::from(c)];
        if rev == -1 {
            return ("Invalid Base 32 character", vec![i]);
        }
        values.push(rev as u8);
    }

    let t = tables();
    let mut enc: Vec<u8> = hrp.iter().map(|c| c >> 5).collect();
    enc.push(0);
    enc.extend(hrp.iter().map(|c| c & 0x1f));
    enc.extend_from_slice(&values);
    let pm = polymod(&enc);

    let mut error_locations: Vec<usize> = Vec::new();
    let mut error_encoding: Option<&'static str> = None;
    for (constant, name) in [
        (BECH32_CONST, "Invalid Bech32 checksum"),
        (BECH32M_CONST, "Invalid Bech32m checksum"),
    ] {
        let residue = pm ^ constant;
        if residue == 0 {
            return ("", Vec::new());
        }
        let mut possible: Vec<usize> = Vec::new();
        let syn = syndrome(residue);
        let s0 = (syn & 0x3ff) as i32;
        let s1 = ((syn >> 10) & 0x3ff) as i32;
        let s2 = (syn >> 20) as i32;
        let (l_s0, l_s1, l_s2) = (t.log[s0 as usize], t.log[s1 as usize], t.log[s2 as usize]);
        if l_s0 != -1 && l_s1 != -1 && l_s2 != -1 && (2 * l_s1 - l_s2 - l_s0 + 2046) % 1023 == 0 {
            let p1 = ((l_s1 - l_s0 + 1023) % 1023) as usize;
            let l_e1 = l_s0 as i64 + (1023 - 997) * p1 as i64;
            if p1 < length && l_e1 % 33 == 0 {
                possible.push(b.len() - p1 - 1);
            }
        } else {
            for p1 in 0..length {
                let s2_s1p1 = s2
                    ^ if s1 == 0 {
                        0
                    } else {
                        t.exp[((l_s1 as usize) + p1) % 1023]
                    };
                if s2_s1p1 == 0 {
                    continue;
                }
                let l_s2_s1p1 = t.log[s2_s1p1 as usize];
                let s1_s0p1 = s1
                    ^ if s0 == 0 {
                        0
                    } else {
                        t.exp[((l_s0 as usize) + p1) % 1023]
                    };
                if s1_s0p1 == 0 {
                    continue;
                }
                let l_s1_s0p1 = t.log[s1_s0p1 as usize];
                let p2 = ((l_s2_s1p1 - l_s1_s0p1 + 1023) % 1023) as usize;
                if p2 >= length || p1 == p2 {
                    continue;
                }
                let s1_s0p2 = s1
                    ^ if s0 == 0 {
                        0
                    } else {
                        t.exp[((l_s0 as usize) + p2) % 1023]
                    };
                if s1_s0p2 == 0 {
                    continue;
                }
                let l_s1_s0p2 = t.log[s1_s0p2 as usize];
                let inv_p1_p2 = 1023 - t.log[(t.exp[p1] ^ t.exp[p2]) as usize];
                let l_e2 = l_s1_s0p1 as i64 + inv_p1_p2 as i64 + (1023 - 997) * p2 as i64;
                if l_e2 % 33 != 0 {
                    continue;
                }
                let l_e1 = l_s1_s0p2 as i64 + inv_p1_p2 as i64 + (1023 - 997) * p1 as i64;
                if l_e1 % 33 != 0 {
                    continue;
                }
                if p1 > p2 {
                    possible.push(b.len() - p1 - 1);
                    possible.push(b.len() - p2 - 1);
                } else {
                    possible.push(b.len() - p2 - 1);
                    possible.push(b.len() - p1 - 1);
                }
                break;
            }
        }
        if error_locations.is_empty()
            || (!possible.is_empty() && possible.len() < error_locations.len())
        {
            error_locations = possible;
            if !error_locations.is_empty() {
                error_encoding = Some(name);
            }
        }
    }
    (
        error_encoding.unwrap_or("Invalid checksum"),
        error_locations,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_match_core_comment() {
        let t = tables();
        for k in 0..1023 {
            assert_eq!(t.log[t.exp[k] as usize], k as i32);
        }
    }

    #[test]
    fn valid_strings_have_no_errors() {
        // BIP-350 test vector.
        assert_eq!(locate_errors("a1lqfn3a"), ("", vec![]));
        assert_eq!(locate_errors("A1LQFN3A"), ("", vec![]));
    }

    #[test]
    fn substitutions_are_located() {
        // BIP-350 valid bech32m vector with one and two characters changed.
        let valid = "abcdef1l7aum6echk45nj3s0wdvt2fg8x9yrzpqzd3ryx";
        let swap = |s: &str, i: usize, c: char| format!("{}{c}{}", &s[..i], &s[i + 1..]);
        assert_eq!(locate_errors(valid), ("", vec![]));
        assert_eq!(
            locate_errors(&swap(valid, 10, 'q')),
            ("Invalid Bech32m checksum", vec![10])
        );
        let two = swap(&swap(valid, 10, 'q'), 20, 'q');
        assert_eq!(
            locate_errors(&two),
            ("Invalid Bech32m checksum", vec![10, 20])
        );
    }
}
