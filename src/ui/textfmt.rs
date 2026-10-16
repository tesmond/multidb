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

/// The whole CSV as one string; [`write_csv`] is what exports use.
#[cfg(test)]
pub fn build_csv(columns: &[String], rows: &[Vec<Value>], sort_index: Option<&[usize]>) -> String {
    let mut out = Vec::new();
    let ok = match sort_index {
        Some(index) if !index.is_empty() => write_csv(&mut out, columns, index.iter().filter_map(|&i| rows.get(i))),
        _ => write_csv(&mut out, columns, rows.iter()),
    };
    debug_assert!(ok.is_ok(), "writing to a Vec cannot fail");
    String::from_utf8(out).unwrap_or_default()
}

/// Write `rows` as CSV to `out` one row at a time, so exporting a large
/// result needs no more memory than the writer's buffer. Lines are separated
/// by `\n` with no trailing newline; nothing is written when there are no
/// columns.
pub fn write_csv<'a, W: std::io::Write>(
    out: &mut W,
    columns: &[String],
    rows: impl Iterator<Item = &'a Vec<Value>>,
) -> std::io::Result<()> {
    if columns.is_empty() {
        return Ok(());
    }
    for (i, c) in columns.iter().enumerate() {
        if i > 0 {
            out.write_all(b",")?;
        }
        out.write_all(csv_safe_string(c).as_bytes())?;
    }
    for row in rows {
        out.write_all(b"\n")?;
        for c in 0..columns.len() {
            if c > 0 {
                out.write_all(b",")?;
            }
            out.write_all(escape_csv(row.get(c).unwrap_or(&Value::Null)).as_bytes())?;
        }
    }
    Ok(())
}

/// Suggested filename for the tab being exported.
pub fn csv_export_filename(tab_title: &str) -> String {
    let name: String = tab_title
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_control() || matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                ch
            }
        })
        .collect();
    if name.is_empty() {
        "query_results.csv".into()
    } else if name.to_ascii_lowercase().ends_with(".csv") {
        name
    } else {
        format!("{name}.csv")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn csv_export_filename_uses_current_tab_name() {
        assert_eq!(csv_export_filename("Query 1"), "Query 1.csv");
        assert_eq!(csv_export_filename("public.customers"), "public.customers.csv");
        let mut title = "Query 2".to_string();
        assert_eq!(csv_export_filename(&title), "Query 2.csv");
        title = "Monthly sales".into();
        assert_eq!(csv_export_filename(&title), "Monthly sales.csv");
        assert_eq!(csv_export_filename("売上.csv"), "売上.csv");
    }

    #[test]
    fn csv_export_filename_handles_unsafe_or_empty_names() {
        assert_eq!(csv_export_filename("  sales/2026\\Q4: totals?  "), "sales_2026_Q4_ totals_.csv");
        assert_eq!(csv_export_filename("sales\nreport"), "sales_report.csv");
        assert_eq!(csv_export_filename("  "), "query_results.csv");
    }

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
