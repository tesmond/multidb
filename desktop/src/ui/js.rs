//! Small helpers that reproduce JavaScript semantics the old web UI relied on
//! (number formatting, `String(value)`, `localeCompare`), so values, sorting and
//! exported text stay byte-for-byte identical after the port.

use serde_json::Value;
use std::cmp::Ordering;

/// `Number.prototype.toString()` for finite and non-finite doubles.
pub fn number_to_string(value: f64) -> String {
    if value.is_nan() {
        return "NaN".into();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    if value == 0.0 {
        return "0".into();
    }
    let negative = value < 0.0;
    // Rust's `{:e}` prints the shortest round-trip representation.
    let sci = format!("{:e}", value.abs());
    let (mantissa, exponent) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let digits: String = mantissa.chars().filter(|c| c.is_ascii_digit()).collect();
    let digits = digits.trim_end_matches('0');
    let digits = if digits.is_empty() { "0" } else { digits };
    let k = digits.len() as i32;
    let n = exponent + 1;
    let mut out = String::new();
    if negative {
        out.push('-');
    }
    if k <= n && n <= 21 {
        out.push_str(digits);
        for _ in 0..(n - k) {
            out.push('0');
        }
    } else if 0 < n && n <= 21 {
        out.push_str(&digits[..n as usize]);
        out.push('.');
        out.push_str(&digits[n as usize..]);
    } else if -6 < n && n <= 0 {
        out.push_str("0.");
        for _ in 0..(-n) {
            out.push('0');
        }
        out.push_str(digits);
    } else {
        out.push_str(&digits[..1]);
        if k > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        let e = n - 1;
        out.push(if e < 0 { '-' } else { '+' });
        out.push_str(&e.abs().to_string());
    }
    out
}

/// `String(value)` for the JSON values the backend produces.
pub fn value_to_string(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                if (i as f64).abs() < 1e21 && i.unsigned_abs() <= (1u64 << 53) {
                    return i.to_string();
                }
                return number_to_string(i as f64);
            }
            if let Some(u) = n.as_u64() {
                if u <= (1u64 << 53) {
                    return u.to_string();
                }
                return number_to_string(u as f64);
            }
            number_to_string(n.as_f64().unwrap_or(f64::NAN))
        }
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .map(|v| match v {
                Value::Null => String::new(),
                other => value_to_string(other),
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".into(),
    }
}

/// Text shown for a cell: JSON null renders as `None` so callers can style it.
pub fn cell_text(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        other => Some(value_to_string(other)),
    }
}

// ─── localeCompare ──────────────────────────────────────────────────────────

/// Approximates ICU root collation (what `String.prototype.localeCompare` uses
/// in WebKit) for the characters found in identifiers and query results:
/// whitespace < punctuation/symbols < digits < letters, case-insensitive at the
/// primary level with lowercase sorting first on ties.
pub fn locale_compare(a: &str, b: &str) -> Ordering {
    collate(a, b, false)
}

/// `localeCompare(b, undefined, { numeric: true })`.
pub fn locale_compare_numeric(a: &str, b: &str) -> Ordering {
    collate(a, b, true)
}

const PUNCT_ORDER: &str = "_-,;:!?.'\"()[]{}@*/\\&#%`^+<=>|~$";

#[derive(PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Primary {
    Space(u32),
    Punct(u32),
    Digits(String),
    Letter(u32),
}

fn primary_elements(s: &str, numeric: bool) -> Vec<Primary> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_ascii_digit() {
            if numeric {
                let start = i;
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
                let run: String = chars[start..i].iter().collect();
                let trimmed = run.trim_start_matches('0');
                let trimmed = if trimmed.is_empty() { "0" } else { trimmed };
                // Compare by length first then lexicographically: pad to fixed width.
                out.push(Primary::Digits(format!("{:0>40}", trimmed)));
                continue;
            }
            out.push(Primary::Digits(c.to_string()));
        } else if c.is_whitespace() || c.is_control() {
            out.push(Primary::Space(c as u32));
        } else if let Some(pos) = PUNCT_ORDER.find(c) {
            out.push(Primary::Punct(pos as u32));
        } else if c.is_alphabetic() {
            let lower = c.to_lowercase().next().unwrap_or(c);
            out.push(Primary::Letter(fold_accent(lower) as u32));
        } else {
            out.push(Primary::Punct(1000 + c as u32));
        }
        i += 1;
    }
    out
}

fn fold_accent(c: char) -> char {
    match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
        'ç' => 'c',
        'è' | 'é' | 'ê' | 'ë' => 'e',
        'ì' | 'í' | 'î' | 'ï' => 'i',
        'ñ' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' => 'o',
        'ù' | 'ú' | 'û' | 'ü' => 'u',
        'ý' | 'ÿ' => 'y',
        other => other,
    }
}

fn collate(a: &str, b: &str, numeric: bool) -> Ordering {
    let pa = primary_elements(a, numeric);
    let pb = primary_elements(b, numeric);
    match pa.cmp(&pb) {
        Ordering::Equal => {}
        other => return other,
    }
    // Secondary: accents (unaccented first).
    for (ca, cb) in a.chars().zip(b.chars()) {
        let la = ca.to_lowercase().next().unwrap_or(ca);
        let lb = cb.to_lowercase().next().unwrap_or(cb);
        if la != lb {
            return la.cmp(&lb);
        }
    }
    // Tertiary: lowercase before uppercase.
    for (ca, cb) in a.chars().zip(b.chars()) {
        if ca != cb {
            let a_lower = ca.is_lowercase();
            let b_lower = cb.is_lowercase();
            if a_lower != b_lower {
                return if a_lower { Ordering::Less } else { Ordering::Greater };
            }
            return ca.cmp(&cb);
        }
    }
    a.len().cmp(&b.len())
}

/// Truncate to at most `max` UTF-16 code units the way `String.slice(0, max)`
/// does for the BMP text we display.
pub fn slice_chars(s: &str, max: usize) -> &str {
    match s.char_indices().nth(max) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

pub fn char_len(s: &str) -> usize {
    s.chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_numbers_like_javascript() {
        assert_eq!(number_to_string(3.0), "3");
        assert_eq!(number_to_string(3.5), "3.5");
        assert_eq!(number_to_string(0.1 + 0.2), "0.30000000000000004");
        assert_eq!(number_to_string(1e21), "1e+21");
        assert_eq!(number_to_string(123456789012.0), "123456789012");
        assert_eq!(number_to_string(0.000001), "0.000001");
        assert_eq!(number_to_string(0.0000001), "1e-7");
        assert_eq!(number_to_string(-42.25), "-42.25");
    }

    #[test]
    fn locale_compare_orders_like_icu() {
        assert_eq!(locale_compare("apple", "Banana"), Ordering::Less);
        assert_eq!(locale_compare("a", "A"), Ordering::Less);
        assert_eq!(locale_compare("order_items", "orders"), Ordering::Less);
        assert_eq!(locale_compare_numeric("item2", "item10"), Ordering::Less);
        assert_eq!(locale_compare("item2", "item10"), Ordering::Greater);
        assert_eq!(locale_compare("10", "9"), Ordering::Less);
        assert_eq!(locale_compare_numeric("10", "9"), Ordering::Greater);
    }
}
