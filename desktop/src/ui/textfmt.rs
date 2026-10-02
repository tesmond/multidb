//! Text conversions shared by the results grid, status bar export and
//! clipboard handling (ports of `lib/csv.ts` and `lib/resultClipboard.ts`).

use super::js;
use serde_json::Value;

// ─── Clipboard ──────────────────────────────────────────────────────────────

pub fn is_json_column_type(column_type: Option<&str>) -> bool {
    let text = column_type.unwrap_or("").to_ascii_lowercase();
    // /\bjsonb?\b/i
    let bytes = text.as_bytes();
    let mut i = 0;
    while let Some(pos) = text[i..].find("json") {
        let start = i + pos;
        let mut end = start + 4;
        if end < bytes.len() && bytes[end] == b'b' {
            end += 1;
        }
        let before_ok = start == 0 || !is_word(bytes[start - 1]);
        let after_ok = end >= bytes.len() || !is_word(bytes[end]);
        if before_ok && after_ok {
            return true;
        }
        // "json" followed by 'b' then word char: try the shorter match.
        if end == start + 5 {
            let after_short = !is_word(bytes[start + 4]);
            if before_ok && after_short {
                return true;
            }
        }
        i = start + 1;
    }
    false
}

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

pub fn pretty_print_json_text(text: &str) -> String {
    match serde_json::from_str::<Value>(text) {
        Ok(value) => js_pretty_json(&value, 0),
        Err(_) => text.to_string(),
    }
}

/// `JSON.stringify(value, null, 2)`.
fn js_pretty_json(value: &Value, indent: usize) -> String {
    let pad = "  ".repeat(indent + 1);
    let close = "  ".repeat(indent);
    match value {
        Value::Array(items) if !items.is_empty() => {
            let inner: Vec<String> = items
                .iter()
                .map(|item| format!("{pad}{}", js_pretty_json(item, indent + 1)))
                .collect();
            format!("[\n{}\n{close}]", inner.join(",\n"))
        }
        Value::Object(map) if !map.is_empty() => {
            let inner: Vec<String> = map
                .iter()
                .map(|(k, v)| {
                    format!(
                        "{pad}{}: {}",
                        serde_json::to_string(k).unwrap_or_default(),
                        js_pretty_json(v, indent + 1)
                    )
                })
                .collect();
            format!("{{\n{}\n{close}}}", inner.join(",\n"))
        }
        Value::Array(_) => "[]".into(),
        Value::Object(_) => "{}".into(),
        Value::Number(_) => js::value_to_string(value),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

pub fn format_value_for_clipboard(value: &Value, column_type: Option<&str>, null_text: &str) -> String {
    if value.is_null() {
        return null_text.to_string();
    }
    let text = js::value_to_string(value);
    if is_json_column_type(column_type) {
        return pretty_print_json_text(&text);
    }
    text
}

pub fn escape_tsv_cell(text: &str) -> String {
    if !text.contains(['\t', '\n', '\r', '"']) {
        return text.to_string();
    }
    format!("\"{}\"", text.replace('"', "\"\""))
}

// ─── CSV ────────────────────────────────────────────────────────────────────

fn csv_safe_string(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn escape_csv(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Array(_) | Value::Object(_) => {
            csv_safe_string(&serde_json::to_string(value).unwrap_or_default())
        }
        other => csv_safe_string(&js::value_to_string(other)),
    }
}

pub fn build_csv(columns: &[String], rows: &[Vec<Value>], sort_index: Option<&[usize]>) -> String {
    if columns.is_empty() {
        return String::new();
    }
    let mut lines = Vec::with_capacity(rows.len() + 1);
    lines.push(
        columns
            .iter()
            .map(|c| csv_safe_string(c))
            .collect::<Vec<_>>()
            .join(","),
    );
    let row_to_csv = |row: &Vec<Value>| -> String {
        (0..columns.len())
            .map(|c| escape_csv(row.get(c).unwrap_or(&Value::Null)))
            .collect::<Vec<_>>()
            .join(",")
    };
    match sort_index {
        Some(index) if !index.is_empty() => {
            for &idx in index {
                if let Some(row) = rows.get(idx) {
                    lines.push(row_to_csv(row));
                }
            }
        }
        _ => {
            for row in rows {
                lines.push(row_to_csv(row));
            }
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn clipboard_formats_json_columns() {
        assert_eq!(
            format_value_for_clipboard(&json!("{\"a\":1,\"b\":[1,2]}"), Some("jsonb"), ""),
            "{\n  \"a\": 1,\n  \"b\": [\n    1,\n    2\n  ]\n}"
        );
        assert_eq!(format_value_for_clipboard(&json!("x"), Some("text"), ""), "x");
        assert_eq!(format_value_for_clipboard(&Value::Null, Some("text"), "NULL"), "NULL");
        assert!(is_json_column_type(Some("JSON")));
        assert!(!is_json_column_type(Some("jsonpath")));
    }

    #[test]
    fn tsv_and_csv_escape() {
        assert_eq!(escape_tsv_cell("a\tb"), "\"a\tb\"");
        assert_eq!(escape_tsv_cell("plain"), "plain");
        let csv = build_csv(
            &["a".into(), "b,c".into()],
            &[vec![json!(1), json!("x\"y")], vec![Value::Null, json!(2.5)]],
            None,
        );
        assert_eq!(csv, "a,\"b,c\"\n1,\"x\"\"y\"\n,2.5");
    }
}
