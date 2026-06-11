use crate::models::ExecuteResult;
use anyhow::Result;
use futures_util::StreamExt;
use serde_json::{Number, Value};
use sqlx::{
    mysql::MySqlRow,
    postgres::{PgRow, PgValueFormat},
    AnyPool, Column, MySqlPool, PgPool, Row, TypeInfo, Value as SqlxValue, ValueRef,
};
use std::time::Instant;
use tokio_util::sync::CancellationToken;

pub fn looks_like_row_returning_query(query: &str) -> bool {
    let q = query.trim();
    if q.is_empty() {
        return false;
    }
    let upper = q.to_ascii_uppercase();
    [
        "SELECT", "WITH", "SHOW", "DESCRIBE", "DESC ", "EXPLAIN", "PRAGMA", "VALUES",
    ]
    .iter()
    .any(|prefix| upper.starts_with(prefix))
}

pub async fn execute(
    pool: &AnyPool,
    query: &str,
    max_rows: i64,
    cancel: CancellationToken,
) -> ExecuteResult {
    let max_rows = if max_rows <= 0 {
        10_000_000
    } else {
        max_rows as usize
    };
    if !looks_like_row_returning_query(query) {
        return execute_non_query(pool, query).await;
    }

    let start = Instant::now();
    let mut stream = sqlx::query(query).fetch(pool);
    let mut result = ExecuteResult::default();

    while result.rows.len() < max_rows {
        tokio::select! {
            _ = cancel.cancelled() => {
                result.duration = elapsed_ms(start);
                result.error = "query cancelled".to_string();
                return result;
            }
            row = stream.next() => {
                let Some(row) = row else { break };
                let row = match row {
                    Ok(row) => row,
                    Err(err) => {
                        result.duration = elapsed_ms(start);
                        result.error = err.to_string();
                        return result;
                    }
                };

                if result.columns.is_empty() {
                    result.columns = row.columns().iter().map(|col| col.name().to_string()).collect();
                    result.column_types = row.columns().iter().map(|col| col.type_info().name().to_string()).collect();
                }

                match row_to_json_values(&row) {
                    Ok(values) => result.rows.push(values),
                    Err(err) => {
                        result.duration = elapsed_ms(start);
                        result.error = format!("scan: {err}");
                        return result;
                    }
                }
            }
        }
    }

    result.duration = elapsed_ms(start);
    result
}

pub async fn execute_non_query(pool: &AnyPool, query: &str) -> ExecuteResult {
    let start = Instant::now();
    let mut rows_affected = 0_i64;

    for statement in split_statements(query) {
        match sqlx::query(&statement).execute(pool).await {
            Ok(done) => rows_affected += done.rows_affected() as i64,
            Err(err) => {
                return ExecuteResult {
                    duration: elapsed_ms(start),
                    error: err.to_string(),
                    ..ExecuteResult::default()
                };
            }
        }
    }

    ExecuteResult {
        rows_affected,
        duration: elapsed_ms(start),
        ..ExecuteResult::default()
    }
}

pub async fn execute_postgres(
    pool: &PgPool,
    query: &str,
    max_rows: i64,
    cancel: CancellationToken,
) -> ExecuteResult {
    let max_rows = if max_rows <= 0 {
        1_000_000
    } else {
        max_rows as usize
    };
    if !looks_like_row_returning_query(query) {
        return execute_postgres_non_query(pool, query).await;
    }

    let start = Instant::now();
    let mut stream = sqlx::query(query).fetch(pool);
    let mut result = ExecuteResult::default();

    while result.rows.len() < max_rows {
        tokio::select! {
            _ = cancel.cancelled() => {
                result.duration = elapsed_ms(start);
                result.error = "query cancelled".to_string();
                return result;
            }
            row = stream.next() => {
                let Some(row) = row else { break };
                let row = match row {
                    Ok(row) => row,
                    Err(err) => {
                        result.duration = elapsed_ms(start);
                        result.error = err.to_string();
                        return result;
                    }
                };

                if result.columns.is_empty() {
                    result.columns = row.columns().iter().map(|col| col.name().to_string()).collect();
                    result.column_types = row.columns().iter().map(|col| col.type_info().name().to_string()).collect();
                }

                match pg_row_to_json_values(&row) {
                    Ok(values) => result.rows.push(values),
                    Err(err) => {
                        result.duration = elapsed_ms(start);
                        result.error = format!("scan: {err}");
                        return result;
                    }
                }
            }
        }
    }

    result.duration = elapsed_ms(start);
    result
}

pub async fn execute_postgres_non_query(pool: &PgPool, query: &str) -> ExecuteResult {
    let start = Instant::now();
    let mut rows_affected = 0_i64;

    for statement in split_statements(query) {
        match sqlx::query(&statement).execute(pool).await {
            Ok(done) => rows_affected += done.rows_affected() as i64,
            Err(err) => {
                return ExecuteResult {
                    duration: elapsed_ms(start),
                    error: err.to_string(),
                    ..ExecuteResult::default()
                };
            }
        }
    }

    ExecuteResult {
        rows_affected,
        duration: elapsed_ms(start),
        ..ExecuteResult::default()
    }
}

pub async fn execute_mysql(
    pool: &MySqlPool,
    query: &str,
    max_rows: i64,
    cancel: CancellationToken,
) -> ExecuteResult {
    let max_rows = if max_rows <= 0 {
        1_000_000
    } else {
        max_rows as usize
    };
    if !looks_like_row_returning_query(query) {
        return execute_mysql_non_query(pool, query).await;
    }

    let start = Instant::now();
    let mut stream = sqlx::query(query).fetch(pool);
    let mut result = ExecuteResult::default();

    while result.rows.len() < max_rows {
        tokio::select! {
            _ = cancel.cancelled() => {
                result.duration = elapsed_ms(start);
                result.error = "query cancelled".to_string();
                return result;
            }
            row = stream.next() => {
                let Some(row) = row else { break };
                let row = match row {
                    Ok(row) => row,
                    Err(err) => {
                        result.duration = elapsed_ms(start);
                        result.error = err.to_string();
                        return result;
                    }
                };

                if result.columns.is_empty() {
                    result.columns = row.columns().iter().map(|col| col.name().to_string()).collect();
                    result.column_types = row.columns().iter().map(|col| col.type_info().name().to_string()).collect();
                }

                match mysql_row_to_json_values(&row) {
                    Ok(values) => result.rows.push(values),
                    Err(err) => {
                        result.duration = elapsed_ms(start);
                        result.error = format!("scan: {err}");
                        return result;
                    }
                }
            }
        }
    }

    result.duration = elapsed_ms(start);
    result
}

pub async fn execute_mysql_non_query(pool: &MySqlPool, query: &str) -> ExecuteResult {
    let start = Instant::now();
    let mut rows_affected = 0_i64;

    for statement in split_statements(query) {
        match sqlx::query(&statement).execute(pool).await {
            Ok(done) => rows_affected += done.rows_affected() as i64,
            Err(err) => {
                return ExecuteResult {
                    duration: elapsed_ms(start),
                    error: err.to_string(),
                    ..ExecuteResult::default()
                };
            }
        }
    }

    ExecuteResult {
        rows_affected,
        duration: elapsed_ms(start),
        ..ExecuteResult::default()
    }
}

pub fn split_statements(query: &str) -> Vec<String> {
    let statements: Vec<_> = query
        .split(';')
        .map(str::trim)
        .filter(|statement| !statement.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    if statements.is_empty() {
        vec![query.trim().to_string()]
    } else {
        statements
    }
}

pub fn row_to_json_values(row: &sqlx::any::AnyRow) -> Result<Vec<Value>> {
    let mut out = Vec::with_capacity(row.len());
    for idx in 0..row.len() {
        out.push(value_at(row, idx)?);
    }
    Ok(out)
}

pub fn value_at(row: &sqlx::any::AnyRow, idx: usize) -> Result<Value> {
    let raw = row.try_get_raw(idx)?;
    if raw.is_null() {
        return Ok(Value::Null);
    }

    let ty = row
        .columns()
        .get(idx)
        .map(|col| col.type_info().name().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    if let Ok(value) = row.try_get::<bool, _>(idx) {
        return Ok(Value::Bool(value));
    }
    if let Ok(value) = row.try_get::<i64, _>(idx) {
        return Ok(Value::Number(Number::from(value)));
    }
    if let Ok(value) = row.try_get::<f64, _>(idx) {
        if let Some(number) = Number::from_f64(value) {
            return Ok(Value::Number(number));
        }
    }
    if let Ok(value) = row.try_get::<String, _>(idx) {
        return Ok(Value::String(format_text_value(&ty, value)));
    }
    if let Ok(value) = row.try_get::<Vec<u8>, _>(idx) {
        return Ok(Value::String(format_binary_value(&ty, &value)));
    }

    Ok(Value::String(format!("<unsupported:{ty}>")))
}

pub fn pg_row_to_json_values(row: &PgRow) -> Result<Vec<Value>> {
    let mut out = Vec::with_capacity(row.len());
    for idx in 0..row.len() {
        out.push(pg_value_at(row, idx)?);
    }
    Ok(out)
}

pub fn pg_value_at(row: &PgRow, idx: usize) -> Result<Value> {
    let raw = row.try_get_raw(idx)?;
    if raw.is_null() {
        return Ok(Value::Null);
    }

    let ty = row
        .columns()
        .get(idx)
        .map(|col| col.type_info().name().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    if let Ok(value) = row.try_get::<bool, _>(idx) {
        return Ok(Value::Bool(value));
    }
    if let Ok(value) = row.try_get::<i64, _>(idx) {
        return Ok(Value::Number(Number::from(value)));
    }
    if let Ok(value) = row.try_get::<i32, _>(idx) {
        return Ok(Value::Number(Number::from(value)));
    }
    if let Ok(value) = row.try_get::<i16, _>(idx) {
        return Ok(Value::Number(Number::from(value)));
    }
    if let Ok(value) = row.try_get::<f64, _>(idx) {
        if let Some(number) = Number::from_f64(value) {
            return Ok(Value::Number(number));
        }
    }
    if let Ok(value) = row.try_get::<f32, _>(idx) {
        if let Some(number) = Number::from_f64(value as f64) {
            return Ok(Value::Number(number));
        }
    }
    if let Ok(value) = row.try_get::<sqlx::types::BigDecimal, _>(idx) {
        return Ok(Value::String(value.to_string()));
    }
    if let Ok(value) = row.try_get::<sqlx::types::Json<Value>, _>(idx) {
        return Ok(Value::String(json_value_to_text(&value.0)));
    }
    if let Ok(value) = row.try_get::<sqlx::types::chrono::NaiveDateTime, _>(idx) {
        return Ok(Value::String(value.to_string()));
    }
    if let Ok(value) = row.try_get::<sqlx::types::chrono::NaiveDate, _>(idx) {
        return Ok(Value::String(value.to_string()));
    }
    if let Ok(value) = row.try_get::<sqlx::types::chrono::NaiveTime, _>(idx) {
        return Ok(Value::String(value.to_string()));
    }
    if let Ok(value) =
        row.try_get::<sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>, _>(idx)
    {
        return Ok(Value::String(value.to_rfc3339()));
    }
    if let Ok(value) =
        row.try_get::<sqlx::types::chrono::DateTime<sqlx::types::chrono::FixedOffset>, _>(idx)
    {
        return Ok(Value::String(value.to_rfc3339()));
    }
    if let Ok(value) =
        row.try_get::<sqlx::types::chrono::DateTime<sqlx::types::chrono::Local>, _>(idx)
    {
        return Ok(Value::String(value.to_rfc3339()));
    }
    if let Ok(value) = row.try_get::<sqlx::postgres::types::PgPoint, _>(idx) {
        return Ok(Value::String(format_point_wkt(value.x, value.y)));
    }
    if let Ok(value) = row.try_get::<sqlx::types::Uuid, _>(idx) {
        return Ok(Value::String(value.to_string()));
    }
    // Route by wire format: binary-format values must go through type-aware
    // binary decoders.  try_decode_unchecked::<String> silently succeeds for
    // binary bytes that happen to be valid UTF-8 (e.g. MACADDR, MONEY, arrays)
    // and produces garbage text; checking the format first avoids this.
    let owned_val = ValueRef::to_owned(&raw);
    match raw.format() {
        PgValueFormat::Text => {
            if let Ok(text) = SqlxValue::try_decode_unchecked::<String>(&owned_val) {
                return Ok(Value::String(format_text_value(&ty, text)));
            }
        }
        PgValueFormat::Binary => {
            if let Ok(bytes) = SqlxValue::try_decode_unchecked::<Vec<u8>>(&owned_val) {
                return Ok(Value::String(format_pg_binary_value(&ty, &bytes)));
            }
        }
    }

    Ok(Value::String(format!("<unsupported postgres:{ty}>")))
}

pub fn mysql_row_to_json_values(row: &MySqlRow) -> Result<Vec<Value>> {
    let mut out = Vec::with_capacity(row.len());
    for idx in 0..row.len() {
        out.push(mysql_value_at(row, idx)?);
    }
    Ok(out)
}

pub fn mysql_value_at(row: &MySqlRow, idx: usize) -> Result<Value> {
    let raw = row.try_get_raw(idx)?;
    if raw.is_null() {
        return Ok(Value::Null);
    }

    let ty = row
        .columns()
        .get(idx)
        .map(|col| col.type_info().name().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    if let Ok(value) = row.try_get::<bool, _>(idx) {
        return Ok(Value::Bool(value));
    }
    if let Ok(value) = row.try_get::<i64, _>(idx) {
        return Ok(Value::Number(Number::from(value)));
    }
    if let Ok(value) = row.try_get::<i32, _>(idx) {
        return Ok(Value::Number(Number::from(value)));
    }
    if let Ok(value) = row.try_get::<i16, _>(idx) {
        return Ok(Value::Number(Number::from(value)));
    }
    if let Ok(value) = row.try_get::<i8, _>(idx) {
        return Ok(Value::Number(Number::from(value)));
    }
    if let Ok(value) = row.try_get::<u64, _>(idx) {
        return Ok(Value::Number(Number::from(value)));
    }
    if let Ok(value) = row.try_get::<u32, _>(idx) {
        return Ok(Value::Number(Number::from(value)));
    }
    if let Ok(value) = row.try_get::<u16, _>(idx) {
        return Ok(Value::Number(Number::from(value)));
    }
    if let Ok(value) = row.try_get::<u8, _>(idx) {
        return Ok(Value::Number(Number::from(value)));
    }
    if let Ok(value) = row.try_get::<f64, _>(idx) {
        if let Some(number) = Number::from_f64(value) {
            return Ok(Value::Number(number));
        }
    }
    if let Ok(value) = row.try_get::<f32, _>(idx) {
        if let Some(number) = Number::from_f64(value as f64) {
            return Ok(Value::Number(number));
        }
    }
    if let Ok(value) = row.try_get::<sqlx::types::BigDecimal, _>(idx) {
        return Ok(Value::String(value.to_string()));
    }
    if let Ok(value) = row.try_get::<sqlx::types::Json<Value>, _>(idx) {
        return Ok(Value::String(json_value_to_text(&value.0)));
    }
    if let Ok(value) = row.try_get::<sqlx::types::chrono::NaiveDateTime, _>(idx) {
        return Ok(Value::String(value.to_string()));
    }
    if let Ok(value) = row.try_get::<sqlx::types::chrono::NaiveDate, _>(idx) {
        return Ok(Value::String(value.to_string()));
    }
    if let Ok(value) = row.try_get::<sqlx::types::chrono::NaiveTime, _>(idx) {
        return Ok(Value::String(value.to_string()));
    }
    if let Ok(value) =
        row.try_get::<sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>, _>(idx)
    {
        return Ok(Value::String(value.to_rfc3339()));
    }
    if let Ok(value) = row.try_get::<sqlx::mysql::types::MySqlTime, _>(idx) {
        return Ok(Value::String(value.to_string()));
    }
    if is_geometry_type(&ty) {
        let owned = ValueRef::to_owned(&raw);
        if let Ok(value) = SqlxValue::try_decode_unchecked::<Vec<u8>>(&owned) {
            return Ok(Value::String(format_binary_value(&ty, &value)));
        }
    }
    // MySQL binary/blob types must be decoded as bytes *before* the String
    // fallback — WKB and other binary payloads are valid UTF-8 control
    // characters that try_get::<String> happily returns as garbage.
    if is_mysql_binary_type(&ty) {
        if let Ok(value) = row.try_get::<Vec<u8>, _>(idx) {
            return Ok(Value::String(format_binary_value(&ty, &value)));
        }
    }
    if let Ok(value) = row.try_get::<String, _>(idx) {
        return Ok(Value::String(value));
    }
    if let Ok(value) = row.try_get::<Vec<u8>, _>(idx) {
        return Ok(Value::String(format_binary_value(&ty, &value)));
    }

    Ok(Value::String(format!("<unsupported mysql:{ty}>")))
}

fn json_value_to_text(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
}

fn format_text_value(type_name: &str, text: String) -> String {
    if is_json_type(type_name) {
        return normalize_json_text(&text);
    }
    text
}

fn normalize_json_text(text: &str) -> String {
    serde_json::from_str::<Value>(text)
        .map(|value| json_value_to_text(&value))
        .unwrap_or_else(|_| text.to_string())
}

fn format_binary_value(type_name: &str, bytes: &[u8]) -> String {
    if is_geometry_type(type_name) {
        if let Some(text) = geometry_bytes_to_text(bytes) {
            return text;
        }
        return format!("<{}:{} bytes>", type_name.to_ascii_lowercase(), bytes.len());
    }

    if is_json_type(type_name) {
        let text = String::from_utf8_lossy(bytes);
        return normalize_json_text(&text);
    }

    if is_bytea_type(type_name) {
        // Encode as Postgres hex notation so raw binary data is always human-readable.
        let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        return format!("\\x{hex}");
    }

    if is_mysql_binary_type(type_name) {
        // Try to interpret the bytes as WKB geometry (e.g. from ST_AsBinary()).
        // MySQL's native geometry columns carry a 4-byte SRID prefix which
        // geometry_bytes_to_text already handles; ST_AsBinary() produces raw WKB.
        if let Some(text) = geometry_bytes_to_text(bytes) {
            return text;
        }
        // Not parseable as geometry — hex-encode so the output is at least readable.
        let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        return format!("0x{hex}");
    }

    String::from_utf8_lossy(bytes).to_string()
}

fn is_mysql_binary_type(type_name: &str) -> bool {
    matches!(
        type_name.to_ascii_lowercase().as_str(),
        "binary" | "varbinary" | "blob" | "tinyblob" | "mediumblob" | "longblob"
    )
}

fn is_bytea_type(type_name: &str) -> bool {
    type_name.eq_ignore_ascii_case("bytea")
}

// ─── Postgres binary-format value decoders ───────────────────────────────────
// Called when sqlx reports PgValueFormat::Binary for a column whose type was
// not caught by any earlier typed decoder.  Each function receives the raw
// wire bytes for that column value.

fn format_pg_binary_value(type_name: &str, bytes: &[u8]) -> String {
    let ty_lower = type_name.to_ascii_lowercase();
    match ty_lower.as_str() {
        "macaddr" => format_pg_macaddr(bytes),
        "macaddr8" => format_pg_macaddr8(bytes),
        "line" => format_pg_line(bytes),
        "lseg" => format_pg_lseg(bytes),
        "box" => format_pg_box(bytes),
        "circle" => format_pg_circle(bytes),
        "money" => format_pg_money(bytes),
        "inet" | "cidr" => format_pg_inet(bytes),
        "interval" => format_pg_interval(bytes),
        "timetz" => format_pg_timetz(bytes),
        "time" => format_pg_time(bytes),
        "int4range" | "int8range" | "numrange" | "tsrange" | "tstzrange" | "daterange" => {
            format_pg_range(bytes, ty_lower.as_str())
        }
        "bytea" => {
            let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
            format!("\\x{hex}")
        }
        // Array types: sqlx may report either the catalog form (_int4) or the
        // display form (INT4[]).  Handle both.
        ty if ty.starts_with('_') || ty.ends_with("[]") => format_pg_array_binary(bytes)
            .unwrap_or_else(|| format!("<{type_name}:{} bytes>", bytes.len())),
        _ => {
            // Unknown binary type: pass through as UTF-8 if valid, else hex-encode.
            String::from_utf8(bytes.to_vec()).unwrap_or_else(|_| {
                let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
                format!("\\x{hex}")
            })
        }
    }
}

fn format_pg_macaddr(bytes: &[u8]) -> String {
    if bytes.len() == 6 {
        format!(
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5]
        )
    } else {
        format!("<macaddr:{} bytes>", bytes.len())
    }
}

fn format_pg_macaddr8(bytes: &[u8]) -> String {
    if bytes.len() == 8 {
        format!(
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7]
        )
    } else {
        format!("<macaddr8:{} bytes>", bytes.len())
    }
}

fn format_pg_line(bytes: &[u8]) -> String {
    // LINE binary: three f64 big-endian values (A, B, C) for Ax+By+C=0
    if bytes.len() != 24 {
        return format!("<line:{} bytes>", bytes.len());
    }
    let a = f64::from_be_bytes(bytes[0..8].try_into().unwrap());
    let b = f64::from_be_bytes(bytes[8..16].try_into().unwrap());
    let c = f64::from_be_bytes(bytes[16..24].try_into().unwrap());
    format!(
        "{{{},{},{}}}",
        format_ordinate(a),
        format_ordinate(b),
        format_ordinate(c)
    )
}

fn format_pg_lseg(bytes: &[u8]) -> String {
    format_pg_two_points(bytes, "lseg")
}

fn format_pg_box(bytes: &[u8]) -> String {
    format_pg_two_points(bytes, "box")
}

/// Shared decoder for two-point types (LSEG, BOX): four f64 big-endian values.
fn format_pg_two_points(bytes: &[u8], type_name: &str) -> String {
    if bytes.len() != 32 {
        return format!("<{type_name}:{} bytes>", bytes.len());
    }
    let x1 = f64::from_be_bytes(bytes[0..8].try_into().unwrap());
    let y1 = f64::from_be_bytes(bytes[8..16].try_into().unwrap());
    let x2 = f64::from_be_bytes(bytes[16..24].try_into().unwrap());
    let y2 = f64::from_be_bytes(bytes[24..32].try_into().unwrap());
    format!(
        "({},{}),({},{})",
        format_ordinate(x1),
        format_ordinate(y1),
        format_ordinate(x2),
        format_ordinate(y2)
    )
}

fn format_pg_circle(bytes: &[u8]) -> String {
    // CIRCLE binary: center_x, center_y, radius as three f64 big-endian values
    if bytes.len() != 24 {
        return format!("<circle:{} bytes>", bytes.len());
    }
    let x = f64::from_be_bytes(bytes[0..8].try_into().unwrap());
    let y = f64::from_be_bytes(bytes[8..16].try_into().unwrap());
    let r = f64::from_be_bytes(bytes[16..24].try_into().unwrap());
    format!(
        "<({},{}),{}>",
        format_ordinate(x),
        format_ordinate(y),
        format_ordinate(r)
    )
}

fn format_pg_money(bytes: &[u8]) -> String {
    // MONEY binary: signed int64, stored as integer cents (2 decimal places).
    if bytes.len() != 8 {
        return format!("<money:{} bytes>", bytes.len());
    }
    let raw: i64 = i64::from_be_bytes(bytes.try_into().unwrap());
    let sign = if raw < 0 { "-" } else { "" };
    let abs_val = raw.unsigned_abs();
    format!("{sign}{}.{:02}", abs_val / 100, abs_val % 100)
}

fn format_pg_inet(bytes: &[u8]) -> String {
    // Binary INET/CIDR: family(1), bits(1), is_cidr(1), nb(1), addr(nb bytes)
    if bytes.len() < 4 {
        return format!("<inet:{} bytes>", bytes.len());
    }
    let family = bytes[0];
    let bits = bytes[1];
    let nb = bytes[3] as usize;
    if bytes.len() < 4 + nb {
        return format!("<inet:{} bytes>", bytes.len());
    }
    let addr = &bytes[4..4 + nb];
    match (family, nb) {
        (2, 4) => {
            let s = format!("{}.{}.{}.{}", addr[0], addr[1], addr[2], addr[3]);
            if bits == 32 {
                s
            } else {
                format!("{s}/{bits}")
            }
        }
        (3, 16) => {
            let groups: Vec<String> = addr
                .chunks(2)
                .map(|c| format!("{:x}", u16::from_be_bytes([c[0], c[1]])))
                .collect();
            let s = groups.join(":");
            if bits == 128 {
                s
            } else {
                format!("{s}/{bits}")
            }
        }
        _ => format!("<inet:{} bytes>", bytes.len()),
    }
}

fn format_pg_interval(bytes: &[u8]) -> String {
    // Binary INTERVAL: int64 microseconds | int32 days | int32 months (16 bytes)
    if bytes.len() != 16 {
        return format!("<interval:{} bytes>", bytes.len());
    }
    let usecs: i64 = i64::from_be_bytes(bytes[0..8].try_into().unwrap());
    let days: i32 = i32::from_be_bytes(bytes[8..12].try_into().unwrap());
    let months: i32 = i32::from_be_bytes(bytes[12..16].try_into().unwrap());

    let years = months / 12;
    let rem_months = months % 12;
    let abs_us = usecs.unsigned_abs();
    let total_secs = abs_us / 1_000_000;
    let micros = abs_us % 1_000_000;
    let secs = total_secs % 60;
    let mins = (total_secs / 60) % 60;
    let hours = total_secs / 3600;
    let time_neg = usecs < 0;

    let mut parts: Vec<String> = Vec::new();
    if years != 0 {
        parts.push(format!(
            "{years} year{}",
            if years.abs() != 1 { "s" } else { "" }
        ));
    }
    if rem_months != 0 {
        parts.push(format!(
            "{rem_months} mon{}",
            if rem_months.abs() != 1 { "s" } else { "" }
        ));
    }
    if days != 0 {
        parts.push(format!(
            "{days} day{}",
            if days.abs() != 1 { "s" } else { "" }
        ));
    }
    let time_sign = if time_neg { "-" } else { "" };
    if micros > 0 {
        parts.push(format!(
            "{time_sign}{hours:02}:{mins:02}:{secs:02}.{micros:06}"
        ));
    } else if hours != 0 || mins != 0 || secs != 0 || parts.is_empty() {
        parts.push(format!("{time_sign}{hours:02}:{mins:02}:{secs:02}"));
    }
    parts.join(" ")
}

fn format_pg_timetz(bytes: &[u8]) -> String {
    // TIMETZ binary: 8 bytes microseconds-since-midnight (i64 BE)
    //              + 4 bytes timezone offset seconds-west-of-UTC (i32 BE)
    if bytes.len() != 12 {
        return format!("<timetz:{} bytes>", bytes.len());
    }
    let usecs: i64 = i64::from_be_bytes(bytes[0..8].try_into().unwrap());
    let zone_secs: i32 = i32::from_be_bytes(bytes[8..12].try_into().unwrap());

    let abs_us = usecs.unsigned_abs();
    let total_secs = abs_us / 1_000_000;
    let micros = abs_us % 1_000_000;
    let secs = total_secs % 60;
    let mins = (total_secs / 60) % 60;
    let hours = total_secs / 3600;

    // zone_secs is seconds *west* of UTC: positive = negative UTC offset.
    let zone_sign = if zone_secs >= 0 { '-' } else { '+' };
    let abs_zone = zone_secs.unsigned_abs();
    let z_hours = abs_zone / 3600;
    let z_mins = (abs_zone % 3600) / 60;

    let time_part = if micros > 0 {
        format!("{hours:02}:{mins:02}:{secs:02}.{micros:06}")
    } else {
        format!("{hours:02}:{mins:02}:{secs:02}")
    };
    let zone_part = if z_mins != 0 {
        format!("{zone_sign}{z_hours:02}:{z_mins:02}")
    } else {
        format!("{zone_sign}{z_hours:02}")
    };
    format!("{time_part}{zone_part}")
}

fn format_pg_time(bytes: &[u8]) -> String {
    // TIME binary: 8 bytes microseconds-since-midnight (i64 BE)
    if bytes.len() != 8 {
        return format!("<time:{} bytes>", bytes.len());
    }
    let usecs: i64 = i64::from_be_bytes(bytes.try_into().unwrap());
    let abs_us = usecs.unsigned_abs();
    let total_secs = abs_us / 1_000_000;
    let micros = abs_us % 1_000_000;
    let secs = total_secs % 60;
    let mins = (total_secs / 60) % 60;
    let hours = total_secs / 3600;
    if micros > 0 {
        format!("{hours:02}:{mins:02}:{secs:02}.{micros:06}")
    } else {
        format!("{hours:02}:{mins:02}:{secs:02}")
    }
}

fn format_pg_range(bytes: &[u8], range_type: &str) -> String {
    // PostgreSQL binary range layout:
    //   1 byte  flags  (RANGE_EMPTY=0x01, RANGE_LB_INC=0x02, RANGE_UB_INC=0x04,
    //                   RANGE_LB_INF=0x08, RANGE_UB_INF=0x10)
    //   [4-byte len + elem bytes]  lower bound  (absent when RANGE_LB_INF)
    //   [4-byte len + elem bytes]  upper bound  (absent when RANGE_UB_INF)
    if bytes.is_empty() {
        return format!("<{range_type}: 0 bytes>");
    }
    let flags = bytes[0];
    const RANGE_EMPTY: u8 = 0x01;
    const RANGE_LB_INC: u8 = 0x02;
    const RANGE_UB_INC: u8 = 0x04;
    const RANGE_LB_INF: u8 = 0x08;
    const RANGE_UB_INF: u8 = 0x10;

    if flags & RANGE_EMPTY != 0 {
        return "empty".to_string();
    }

    let elem_oid: u32 = match range_type {
        "int4range" => 23,
        "int8range" => 20,
        "numrange" => 1700,
        "daterange" => 1082,
        "tsrange" => 1114,
        "tstzrange" => 1184,
        _ => 25,
    };

    let lb = if flags & RANGE_LB_INC != 0 { '[' } else { '(' };
    let ub = if flags & RANGE_UB_INC != 0 { ']' } else { ')' };
    let mut cursor = 1usize;

    let lower = if flags & RANGE_LB_INF != 0 {
        String::new()
    } else {
        match read_range_bound(bytes, &mut cursor, elem_oid) {
            Some(s) => s,
            None => return format!("<{range_type}:{} bytes>", bytes.len()),
        }
    };
    let upper = if flags & RANGE_UB_INF != 0 {
        String::new()
    } else {
        match read_range_bound(bytes, &mut cursor, elem_oid) {
            Some(s) => s,
            None => return format!("<{range_type}:{} bytes>", bytes.len()),
        }
    };

    format!("{lb}{lower},{upper}{ub}")
}

fn read_range_bound(bytes: &[u8], cursor: &mut usize, elem_oid: u32) -> Option<String> {
    if *cursor + 4 > bytes.len() {
        return None;
    }
    let len = i32::from_be_bytes(bytes[*cursor..*cursor + 4].try_into().ok()?);
    *cursor += 4;
    if len < 0 {
        return Some("NULL".to_string());
    }
    let len = len as usize;
    if *cursor + len > bytes.len() {
        return None;
    }
    let s = decode_pg_range_element(elem_oid, &bytes[*cursor..*cursor + len]);
    *cursor += len;
    Some(s)
}

fn decode_pg_range_element(elem_oid: u32, bytes: &[u8]) -> String {
    match elem_oid {
        // Delegate simple scalar types to the array element decoder.
        16 | 20 | 21 | 23 | 26 | 700 | 701 | 2950 => decode_pg_array_element(elem_oid, bytes),
        // DATE: i32 BE days since 2000-01-01
        1082 if bytes.len() == 4 => {
            let days = i32::from_be_bytes(bytes.try_into().unwrap());
            // 2000-01-01 is Unix day 10957.  Add days offset.
            let unix_day = 10957i64 + days as i64;
            pg_unix_day_to_date(unix_day)
        }
        // TIMESTAMP / TIMESTAMPTZ: i64 BE microseconds since 2000-01-01 00:00:00
        1114 | 1184 if bytes.len() == 8 => {
            let usecs = i64::from_be_bytes(bytes.try_into().unwrap());
            // 2000-01-01 00:00:00 UTC is Unix timestamp 946684800 s.
            let unix_us = 946_684_800i64 * 1_000_000 + usecs;
            pg_unix_us_to_datetime(unix_us)
        }
        // NUMERIC: decode PostgreSQL base-10000 binary format
        1700 => decode_pg_numeric(bytes),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

fn pg_unix_day_to_date(unix_day: i64) -> String {
    // Convert a Unix day number (days since 1970-01-01) to a YYYY-MM-DD string.
    sqlx::types::chrono::NaiveDate::from_num_days_from_ce_opt(
        (unix_day + 719_163) as i32, // days since year 1 CE: Unix epoch = day 719163 CE
    )
    .map(|d| d.to_string())
    .unwrap_or_else(|| format!("<date:day={unix_day}>"))
}

fn pg_unix_us_to_datetime(unix_us: i64) -> String {
    let secs = unix_us.div_euclid(1_000_000);
    let nsecs = (unix_us.rem_euclid(1_000_000) * 1_000) as u32;
    sqlx::types::chrono::DateTime::<sqlx::types::chrono::Utc>::from_timestamp(secs, nsecs)
        .map(|dt| dt.naive_utc().to_string())
        .unwrap_or_else(|| format!("<ts:{unix_us}us>"))
}

fn decode_pg_numeric(bytes: &[u8]) -> String {
    // PostgreSQL NUMERIC binary: ndigits(u16) weight(i16) sign(u16) dscale(u16)
    //   followed by ndigits × u16 base-10000 digit groups.
    if bytes.len() < 8 {
        return format!("<numeric:{} bytes>", bytes.len());
    }
    let ndigits = u16::from_be_bytes([bytes[0], bytes[1]]) as i32;
    let weight = i16::from_be_bytes([bytes[2], bytes[3]]) as i32;
    let sign = u16::from_be_bytes([bytes[4], bytes[5]]);
    let dscale = u16::from_be_bytes([bytes[6], bytes[7]]) as usize;
    match sign {
        0xC000 => return "NaN".to_string(),
        0xD000 => return "Infinity".to_string(),
        0xF000 => return "-Infinity".to_string(),
        _ => {}
    }
    if bytes.len() < 8 + ndigits as usize * 2 {
        return format!("<numeric:{} bytes>", bytes.len());
    }
    let groups: Vec<u16> = (0..ndigits as usize)
        .map(|i| u16::from_be_bytes([bytes[8 + i * 2], bytes[9 + i * 2]]))
        .collect();
    let mut result = String::new();
    if sign == 0x4000 {
        result.push('-');
    }
    // Zero
    if ndigits == 0 {
        result.push('0');
        if dscale > 0 {
            result.push('.');
            for _ in 0..dscale {
                result.push('0');
            }
        }
        return result;
    }
    // Integer part: groups[0..int_groups]
    let int_groups = (weight + 1).max(0) as usize;
    if int_groups == 0 {
        result.push('0');
    } else {
        for g in 0..int_groups {
            if g < groups.len() {
                if g == 0 {
                    result.push_str(&groups[g].to_string());
                } else {
                    result.push_str(&format!("{:04}", groups[g]));
                }
            } else {
                result.push_str("0000");
            }
        }
    }
    // Fractional part
    if dscale > 0 {
        result.push('.');
        let mut written = 0usize;
        // Groups before the first stored fractional group (weight < -1)
        let frac_leading = if weight < -1 {
            (-(weight + 1)) as usize
        } else {
            0
        };
        for _ in 0..frac_leading {
            if written >= dscale {
                break;
            }
            let take = (dscale - written).min(4);
            result.push_str(&"0000"[..take]);
            written += take;
        }
        let frac_start = int_groups;
        for g in frac_start..groups.len() {
            if written >= dscale {
                break;
            }
            let s = format!("{:04}", groups[g]);
            let take = (dscale - written).min(4);
            result.push_str(&s[..take]);
            written += take;
        }
        while written < dscale {
            result.push('0');
            written += 1;
        }
    }
    result
}

fn format_pg_array_binary(bytes: &[u8]) -> Option<String> {
    // PostgreSQL binary array layout:
    //   int32  ndim          — number of dimensions
    //   int32  flags         — 0 = no nulls; bit 1 = has nulls
    //   int32  elem_oid      — OID of the element type
    //   For each dimension (ndim × 8 bytes):
    //     int32  dim_length
    //     int32  lower_bound (always 1 for non-slice arrays)
    //   For each element:
    //     int32  elem_len    (-1 = NULL)
    //     bytes  elem_data   (only present when elem_len >= 0)
    if bytes.len() < 12 {
        return None;
    }
    let ndim = i32::from_be_bytes(bytes[0..4].try_into().ok()?) as usize;
    let elem_oid = u32::from_be_bytes(bytes[8..12].try_into().ok()?);

    if ndim == 0 {
        return Some("{}".to_string());
    }

    let header_end = 12 + ndim * 8;
    if bytes.len() < header_end {
        return None;
    }

    let mut dims: Vec<usize> = Vec::with_capacity(ndim);
    for d in 0..ndim {
        let off = 12 + d * 8;
        let dim_len = i32::from_be_bytes(bytes[off..off + 4].try_into().ok()?) as usize;
        dims.push(dim_len);
    }

    let total: usize = dims.iter().product();
    let mut offset = header_end;
    let mut elements: Vec<Option<String>> = Vec::with_capacity(total);

    for _ in 0..total {
        if offset + 4 > bytes.len() {
            return None;
        }
        let elem_len = i32::from_be_bytes(bytes[offset..offset + 4].try_into().ok()?);
        offset += 4;
        if elem_len < 0 {
            elements.push(None);
        } else {
            let len = elem_len as usize;
            if offset + len > bytes.len() {
                return None;
            }
            elements.push(Some(decode_pg_array_element(
                elem_oid,
                &bytes[offset..offset + len],
            )));
            offset += len;
        }
    }

    let mut idx = 0usize;
    Some(pg_array_to_text(&elements, &dims, 0, &mut idx))
}

fn decode_pg_array_element(elem_oid: u32, bytes: &[u8]) -> String {
    match elem_oid {
        16 => bytes.first().map_or("f".into(), |&b| if b != 0 { "t".into() } else { "f".into() }),
        21 => <[u8; 2]>::try_from(bytes).ok()
            .map(i16::from_be_bytes).map(|v| v.to_string())
            .unwrap_or_else(|| format!("<int2:{}>", bytes.len())),
        23 | 26 => <[u8; 4]>::try_from(bytes).ok()
            .map(i32::from_be_bytes).map(|v| v.to_string())
            .unwrap_or_else(|| format!("<int4:{}>", bytes.len())),
        20 => <[u8; 8]>::try_from(bytes).ok()
            .map(i64::from_be_bytes).map(|v| v.to_string())
            .unwrap_or_else(|| format!("<int8:{}>", bytes.len())),
        700 => <[u8; 4]>::try_from(bytes).ok()
            .map(f32::from_be_bytes).map(|v| format_ordinate(v as f64))
            .unwrap_or_else(|| format!("<float4:{}>", bytes.len())),
        701 => <[u8; 8]>::try_from(bytes).ok()
            .map(f64::from_be_bytes).map(|v| format_ordinate(v))
            .unwrap_or_else(|| format!("<float8:{}>", bytes.len())),
        2950 if bytes.len() == 16 => format!(
            "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            bytes[0], bytes[1], bytes[2], bytes[3],
            bytes[4], bytes[5], bytes[6], bytes[7],
            bytes[8], bytes[9],
            bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
        ),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

fn pg_array_to_text(
    elements: &[Option<String>],
    dims: &[usize],
    dim: usize,
    idx: &mut usize,
) -> String {
    if dim == dims.len() - 1 {
        let mut parts = Vec::with_capacity(dims[dim]);
        for _ in 0..dims[dim] {
            let s = match &elements[*idx] {
                None => "NULL".to_string(),
                Some(v) if pg_array_elem_needs_quoting(v) => {
                    format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\""))
                }
                Some(v) => v.clone(),
            };
            parts.push(s);
            *idx += 1;
        }
        format!("{{{}}}", parts.join(","))
    } else {
        let mut parts = Vec::with_capacity(dims[dim]);
        for _ in 0..dims[dim] {
            parts.push(pg_array_to_text(elements, dims, dim + 1, idx));
        }
        format!("{{{}}}", parts.join(","))
    }
}

fn pg_array_elem_needs_quoting(s: &str) -> bool {
    s.is_empty()
        || s.eq_ignore_ascii_case("null")
        || s.contains(|c: char| {
            matches!(c, '"' | '\\' | '{' | '}' | ',') || c.is_ascii_whitespace()
        })
}

fn is_json_type(type_name: &str) -> bool {
    matches!(type_name.to_ascii_lowercase().as_str(), "json" | "jsonb")
}

fn is_geometry_type(type_name: &str) -> bool {
    matches!(
        type_name.to_ascii_lowercase().as_str(),
        "geometry"
            | "point"
            | "linestring"
            | "polygon"
            | "multipoint"
            | "multilinestring"
            | "multipolygon"
            | "geometrycollection"
            | "geography"
    )
}

fn geometry_bytes_to_text(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        return Some(String::new());
    }

    if let Some(text) = parse_geometry_payload(bytes, None) {
        return Some(text);
    }

    if bytes.len() >= 5 && matches!(bytes[4], 0 | 1) {
        let srid = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        if let Some(text) = parse_geometry_payload(&bytes[4..], Some(srid)) {
            return Some(text);
        }
    }

    None
}

fn parse_geometry_payload(wkb: &[u8], srid: Option<u32>) -> Option<String> {
    let mut cursor = WkbCursor::new(wkb);
    let text = parse_wkb_geometry(&mut cursor)?;
    if cursor.remaining() != 0 {
        return None;
    }

    match srid {
        Some(srid) if srid != 0 => Some(format!("SRID={srid};{text}")),
        _ => Some(text),
    }
}

fn parse_wkb_geometry(cursor: &mut WkbCursor<'_>) -> Option<String> {
    let endian = cursor.read_u8()?;
    let little_endian = match endian {
        0 => false,
        1 => true,
        _ => return None,
    };

    let type_word = cursor.read_u32(little_endian)?;
    let has_z = type_word >= 1000 && type_word < 4000 || (type_word & 0x8000_0000) != 0;
    let has_m = type_word >= 2000 && type_word < 4000 || (type_word & 0x4000_0000) != 0;
    let has_srid = (type_word & 0x2000_0000) != 0;
    if has_srid {
        cursor.read_u32(little_endian)?;
    }

    let base_type = if (1..=7).contains(&type_word) {
        type_word
    } else if (1000..4000).contains(&type_word) {
        type_word % 1000
    } else {
        type_word & 0x0000_00ff
    };
    let dimensions = 2 + usize::from(has_z) + usize::from(has_m);

    match base_type {
        1 => parse_wkb_point(cursor, little_endian, dimensions)
            .map(|coords| format!("POINT({coords})")),
        2 => parse_wkb_linestring(cursor, little_endian, dimensions)
            .map(|coords| format!("LINESTRING({coords})")),
        3 => parse_wkb_polygon(cursor, little_endian, dimensions)
            .map(|rings| format!("POLYGON({rings})")),
        4 => parse_wkb_multi(cursor, little_endian, "MULTIPOINT"),
        5 => parse_wkb_multi(cursor, little_endian, "MULTILINESTRING"),
        6 => parse_wkb_multi(cursor, little_endian, "MULTIPOLYGON"),
        7 => parse_wkb_multi(cursor, little_endian, "GEOMETRYCOLLECTION"),
        _ => None,
    }
}

fn parse_wkb_point(
    cursor: &mut WkbCursor<'_>,
    little_endian: bool,
    dimensions: usize,
) -> Option<String> {
    Some(read_wkb_coordinate(cursor, little_endian, dimensions))
}

fn parse_wkb_linestring(
    cursor: &mut WkbCursor<'_>,
    little_endian: bool,
    dimensions: usize,
) -> Option<String> {
    let count = usize::try_from(cursor.read_u32(little_endian)?).ok()?;
    let mut coords = Vec::with_capacity(count);
    for _ in 0..count {
        coords.push(read_wkb_coordinate(cursor, little_endian, dimensions));
    }
    Some(coords.join(", "))
}

fn parse_wkb_polygon(
    cursor: &mut WkbCursor<'_>,
    little_endian: bool,
    dimensions: usize,
) -> Option<String> {
    let ring_count = usize::try_from(cursor.read_u32(little_endian)?).ok()?;
    let mut rings = Vec::with_capacity(ring_count);
    for _ in 0..ring_count {
        let point_count = usize::try_from(cursor.read_u32(little_endian)?).ok()?;
        let mut points = Vec::with_capacity(point_count);
        for _ in 0..point_count {
            points.push(read_wkb_coordinate(cursor, little_endian, dimensions));
        }
        rings.push(format!("({})", points.join(", ")));
    }
    Some(rings.join(", "))
}

fn parse_wkb_multi(cursor: &mut WkbCursor<'_>, little_endian: bool, label: &str) -> Option<String> {
    let count = usize::try_from(cursor.read_u32(little_endian)?).ok()?;
    let mut items = Vec::with_capacity(count);
    for _ in 0..count {
        items.push(parse_wkb_geometry(cursor)?);
    }
    Some(format!("{}({})", label, items.join(", ")))
}

fn read_wkb_coordinate(
    cursor: &mut WkbCursor<'_>,
    little_endian: bool,
    dimensions: usize,
) -> String {
    let mut ordinates = Vec::with_capacity(dimensions.min(3));
    let x = cursor.read_f64(little_endian).unwrap_or_default();
    let y = cursor.read_f64(little_endian).unwrap_or_default();
    ordinates.push(format_ordinate(x));
    ordinates.push(format_ordinate(y));

    if dimensions >= 3 {
        let z = cursor.read_f64(little_endian).unwrap_or_default();
        ordinates.push(format_ordinate(z));
    }
    for _ in ordinates.len()..dimensions {
        let _ = cursor.read_f64(little_endian);
    }

    ordinates.join(" ")
}

fn format_point_wkt(x: f64, y: f64) -> String {
    format!("POINT({} {})", format_ordinate(x), format_ordinate(y))
}

fn format_ordinate(value: f64) -> String {
    let mut text = format!("{value}");
    if text.contains('.') {
        while text.ends_with('0') {
            text.pop();
        }
        if text.ends_with('.') {
            text.pop();
        }
    }
    if text == "-0" {
        "0".to_string()
    } else {
        text
    }
}

struct WkbCursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> WkbCursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.offset)
    }

    fn read_u8(&mut self) -> Option<u8> {
        let value = *self.bytes.get(self.offset)?;
        self.offset += 1;
        Some(value)
    }

    fn read_u32(&mut self, little_endian: bool) -> Option<u32> {
        let slice = self.read_exact::<4>()?;
        Some(if little_endian {
            u32::from_le_bytes(slice)
        } else {
            u32::from_be_bytes(slice)
        })
    }

    fn read_f64(&mut self, little_endian: bool) -> Option<f64> {
        let slice = self.read_exact::<8>()?;
        Some(if little_endian {
            f64::from_le_bytes(slice)
        } else {
            f64::from_be_bytes(slice)
        })
    }

    fn read_exact<const N: usize>(&mut self) -> Option<[u8; N]> {
        let end = self.offset.checked_add(N)?;
        let slice = self.bytes.get(self.offset..end)?;
        self.offset = end;
        slice.try_into().ok()
    }
}

pub fn elapsed_ms(start: Instant) -> i64 {
    start.elapsed().as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::{
        decode_pg_numeric, format_binary_value, format_pg_array_binary, format_pg_binary_value,
        format_text_value, geometry_bytes_to_text, json_value_to_text,
    };
    use serde_json::json;

    #[test]
    fn json_values_render_as_text() {
        assert_eq!(
            json_value_to_text(&json!({"name":"demo","ok":true})),
            r#"{"name":"demo","ok":true}"#
        );
    }

    #[test]
    fn mysql_point_bytes_render_as_wkt() {
        let bytes = [
            1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 240, 63, 0, 0, 0, 0, 0, 0, 0, 64,
        ];

        assert_eq!(
            geometry_bytes_to_text(&bytes),
            Some("POINT(1 2)".to_string())
        );
        assert_eq!(format_binary_value("POINT", &bytes), "POINT(1 2)");
    }

    #[test]
    fn mysql_srid_zero_point_bytes_render_as_wkt() {
        let bytes = [
            0, 0, 0, 0, 1, 1, 0, 0, 0, 197, 254, 178, 123, 242, 192, 73, 64, 235, 226, 54, 26, 192,
            91, 192, 191,
        ];

        assert_eq!(
            geometry_bytes_to_text(&bytes),
            Some("POINT(51.5074 -0.1278)".to_string())
        );
        assert_eq!(
            format_binary_value("GEOMETRY", &bytes),
            "POINT(51.5074 -0.1278)"
        );
    }

    #[test]
    fn mysql_srid_zero_linestring_bytes_render_as_wkt() {
        let bytes = [
            0, 0, 0, 0, 1, 2, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 240, 63, 0, 0, 0, 0, 0, 0, 240, 63, 0, 0, 0, 0, 0, 0, 0, 64, 0, 0, 0,
            0, 0, 0, 0, 64,
        ];

        assert_eq!(
            geometry_bytes_to_text(&bytes),
            Some("LINESTRING(0 0, 1 1, 2 2)".to_string())
        );
        assert_eq!(
            format_binary_value("GEOMETRY", &bytes),
            "LINESTRING(0 0, 1 1, 2 2)"
        );
    }

    #[test]
    fn mysql_geometry_with_srid_renders_readably() {
        let bytes = [
            230, 16, 0, 0, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 34, 64, 0, 0, 0, 0, 0, 0, 36, 64,
        ];

        assert_eq!(
            geometry_bytes_to_text(&bytes),
            Some("SRID=4326;POINT(9 10)".to_string())
        );
    }

    // ── Postgres type display ──────────────────────────────────────────────────

    #[test]
    fn bytea_binary_renders_as_hex() {
        assert_eq!(
            format_binary_value("bytea", &[0xde, 0xad, 0xbe, 0xef]),
            r"\xdeadbeef"
        );
        assert_eq!(format_binary_value("BYTEA", &[0x00, 0xff]), r"\x00ff");
        assert_eq!(format_binary_value("bytea", &[]), r"\x");
    }

    #[test]
    fn bytea_binary_non_utf8_does_not_corrupt() {
        // Bytes that are not valid UTF-8 must be hex-encoded, not replaced with U+FFFD.
        let result = format_binary_value("bytea", &[0x80, 0x81, 0xff]);
        assert_eq!(result, r"\x8081ff");
    }

    #[test]
    fn format_text_value_passes_through_postgres_type_text() {
        // These postgres text-format values must be returned unchanged by
        // format_text_value (they are already human-readable as-is).
        let cases = [
            ("uuid", "550e8400-e29b-41d4-a716-446655440000"),
            ("inet", "192.168.1.0/24"),
            ("macaddr", "08:00:2b:01:02:03"),
            ("bit", "10101010"),
            ("varbit", "101"),
            ("interval", "1 year 2 mons 3 days 04:05:06"),
            ("timetz", "14:30:00+05:30"),
            ("int4range", "[1,10)"),
            ("_int4", "{1,2,3}"),
            ("_text", "{hello,world}"),
            ("xml", "<root><child/></root>"),
            ("money", "$1,234.56"),
            ("line", "{1,2,-3}"),
            ("box", "(2,2),(0,0)"),
        ];
        for (ty, val) in cases {
            let result = format_text_value(ty, val.to_string());
            assert_eq!(result, val, "type '{ty}' should pass through unchanged");
        }
    }

    // ── MySQL binary/varbinary/blob ───────────────────────────────────────────────

    #[test]
    fn mysql_varbinary_wkb_point_renders_as_wkt() {
        // WKB for POINT(1 1) as produced by ST_AsBinary() — no SRID prefix.
        let bytes: &[u8] = &[
            0x01, // little-endian
            0x01, 0x00, 0x00, 0x00, // type: POINT
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xf0, 0x3f, // x = 1.0
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xf0, 0x3f, // y = 1.0
        ];
        assert_eq!(format_binary_value("varbinary", bytes), "POINT(1 1)");
        assert_eq!(format_binary_value("VARBINARY", bytes), "POINT(1 1)");
        assert_eq!(format_binary_value("blob", bytes), "POINT(1 1)");
    }

    #[test]
    fn mysql_varbinary_non_geometry_hex_encodes() {
        // Non-WKB binary data must hex-encode rather than show as garbage.
        let bytes: &[u8] = &[0xde, 0xad, 0xbe, 0xef];
        assert_eq!(format_binary_value("varbinary", bytes), "0xdeadbeef");
    }

    #[test]
    fn json_type_is_still_normalised_by_format_text_value() {
        // JSON/JSONB must still be normalised (whitespace stripped) through
        // format_text_value so the existing clipboard behaviour is unaffected.
        let compact = format_text_value("json", "{ \"a\" : 1 }".to_string());
        assert_eq!(compact, r#"{"a":1}"#);
        let compact_b = format_text_value("jsonb", "{ \"x\" : true }".to_string());
        assert_eq!(compact_b, r#"{"x":true}"#);
    }

    // ── Postgres binary-format decoders ───────────────────────────────────────────

    #[test]
    fn pg_macaddr_binary_renders_correctly() {
        let bytes = [0x08u8, 0x00, 0x2b, 0x01, 0x02, 0x03];
        assert_eq!(
            format_pg_binary_value("macaddr", &bytes),
            "08:00:2b:01:02:03"
        );
    }

    #[test]
    fn pg_line_binary_renders_correctly() {
        // LINE {1,2,-3}: A=1.0, B=2.0, C=-3.0 as big-endian f64
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&1.0f64.to_be_bytes());
        bytes.extend_from_slice(&2.0f64.to_be_bytes());
        bytes.extend_from_slice(&(-3.0f64).to_be_bytes());
        assert_eq!(format_pg_binary_value("line", &bytes), "{1,2,-3}");
    }

    #[test]
    fn pg_box_binary_renders_correctly() {
        // BOX (5,5),(0,0): upper-right then lower-left
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&5.0f64.to_be_bytes());
        bytes.extend_from_slice(&5.0f64.to_be_bytes());
        bytes.extend_from_slice(&0.0f64.to_be_bytes());
        bytes.extend_from_slice(&0.0f64.to_be_bytes());
        assert_eq!(format_pg_binary_value("box", &bytes), "(5,5),(0,0)");
    }

    #[test]
    fn pg_money_binary_renders_as_decimal() {
        // $99.99 = 9999 cents
        assert_eq!(
            format_pg_binary_value("money", &9999i64.to_be_bytes()),
            "99.99"
        );
        // -$1.50 = -150 cents
        assert_eq!(
            format_pg_binary_value("money", &(-150i64).to_be_bytes()),
            "-1.50"
        );
    }

    #[test]
    fn pg_int_array_binary_renders_as_postgres_array() {
        // INT4 array {1,2,3,4}
        let mut bytes: Vec<u8> = Vec::new();
        bytes.extend_from_slice(&1i32.to_be_bytes()); // ndim=1
        bytes.extend_from_slice(&0i32.to_be_bytes()); // flags=0
        bytes.extend_from_slice(&23i32.to_be_bytes()); // elem_oid=INT4=23
        bytes.extend_from_slice(&4i32.to_be_bytes()); // dim_len=4
        bytes.extend_from_slice(&1i32.to_be_bytes()); // lower_bound=1
        for v in [1i32, 2, 3, 4] {
            bytes.extend_from_slice(&4i32.to_be_bytes()); // elem_len=4
            bytes.extend_from_slice(&v.to_be_bytes());
        }
        assert_eq!(
            format_pg_array_binary(&bytes),
            Some("{1,2,3,4}".to_string())
        );
    }

    #[test]
    fn pg_text_2d_array_binary_renders_as_nested_postgres_array() {
        // TEXT[][] {{top-left,top-right},{bottom-left,bottom-right}}
        let texts = ["top-left", "top-right", "bottom-left", "bottom-right"];
        let mut bytes: Vec<u8> = Vec::new();
        bytes.extend_from_slice(&2i32.to_be_bytes()); // ndim=2
        bytes.extend_from_slice(&0i32.to_be_bytes()); // flags=0
        bytes.extend_from_slice(&25i32.to_be_bytes()); // elem_oid=TEXT=25
        bytes.extend_from_slice(&2i32.to_be_bytes()); // dim1=2
        bytes.extend_from_slice(&1i32.to_be_bytes()); // lb1=1
        bytes.extend_from_slice(&2i32.to_be_bytes()); // dim2=2
        bytes.extend_from_slice(&1i32.to_be_bytes()); // lb2=1
        for t in texts {
            let tb = t.as_bytes();
            bytes.extend_from_slice(&(tb.len() as i32).to_be_bytes());
            bytes.extend_from_slice(tb);
        }
        assert_eq!(
            format_pg_array_binary(&bytes),
            Some("{{top-left,top-right},{bottom-left,bottom-right}}".to_string())
        );
    }

    #[test]
    fn pg_timetz_binary_renders_correctly() {
        // 18:30:00-05  →  usecs = 18*3600+30*60 = 66600s = 66_600_000_000 µs
        //                 zone  = +18000 s west of UTC  → "-05"
        let usecs: i64 = 66_600 * 1_000_000;
        let mut bytes: Vec<u8> = Vec::new();
        bytes.extend_from_slice(&usecs.to_be_bytes());
        bytes.extend_from_slice(&18000i32.to_be_bytes()); // 18000 s west = UTC-5
        assert_eq!(format_pg_binary_value("timetz", &bytes), "18:30:00-05");
    }

    #[test]
    fn pg_timetz_binary_with_minutes_in_offset_renders_correctly() {
        // 14:30:00+05:30  (IST)  →  zone = -19800 s west = UTC+5:30
        let usecs: i64 = (14 * 3600 + 30 * 60) * 1_000_000;
        let mut bytes: Vec<u8> = Vec::new();
        bytes.extend_from_slice(&usecs.to_be_bytes());
        bytes.extend_from_slice(&(-19800i32).to_be_bytes()); // negative = east
        assert_eq!(format_pg_binary_value("timetz", &bytes), "14:30:00+05:30");
    }

    #[test]
    fn pg_array_type_name_bracket_suffix_is_handled() {
        // sqlx reports INT4[] (bracket form), not _int4 (catalog form).
        // format_pg_binary_value must route both to format_pg_array_binary.
        let mut bytes: Vec<u8> = Vec::new();
        bytes.extend_from_slice(&1i32.to_be_bytes()); // ndim=1
        bytes.extend_from_slice(&0i32.to_be_bytes()); // flags=0
        bytes.extend_from_slice(&23i32.to_be_bytes()); // INT4 OID=23
        bytes.extend_from_slice(&3i32.to_be_bytes()); // dim=3
        bytes.extend_from_slice(&1i32.to_be_bytes()); // lower_bound=1
        for v in [1i32, 2, 3] {
            bytes.extend_from_slice(&4i32.to_be_bytes());
            bytes.extend_from_slice(&v.to_be_bytes());
        }
        // Both name variants must produce the same output.
        assert_eq!(format_pg_binary_value("INT4[]", &bytes), "{1,2,3}");
        assert_eq!(format_pg_binary_value("_int4", &bytes), "{1,2,3}");
    }

    // ── Range types ───────────────────────────────────────────────────────────────

    fn make_int4range_bytes(
        lb_inc: bool,
        lower: Option<i32>,
        ub_inc: bool,
        upper: Option<i32>,
    ) -> Vec<u8> {
        let mut flags: u8 = 0;
        if lb_inc {
            flags |= 0x02;
        }
        if ub_inc {
            flags |= 0x04;
        }
        if lower.is_none() {
            flags |= 0x08;
        }
        if upper.is_none() {
            flags |= 0x10;
        }
        let mut bytes = vec![flags];
        if let Some(v) = lower {
            bytes.extend_from_slice(&4i32.to_be_bytes());
            bytes.extend_from_slice(&v.to_be_bytes());
        }
        if let Some(v) = upper {
            bytes.extend_from_slice(&4i32.to_be_bytes());
            bytes.extend_from_slice(&v.to_be_bytes());
        }
        bytes
    }

    #[test]
    fn pg_int4range_half_open_renders_correctly() {
        let bytes = make_int4range_bytes(true, Some(10), false, Some(25));
        assert_eq!(format_pg_binary_value("int4range", &bytes), "[10,25)");
    }

    #[test]
    fn pg_int4range_closed_renders_correctly() {
        let bytes = make_int4range_bytes(true, Some(1), true, Some(10));
        assert_eq!(format_pg_binary_value("int4range", &bytes), "[1,10]");
    }

    #[test]
    fn pg_int4range_empty_renders_correctly() {
        let bytes = [0x01u8]; // RANGE_EMPTY flag
        assert_eq!(format_pg_binary_value("int4range", &bytes), "empty");
    }

    #[test]
    fn pg_int4range_infinite_upper_renders_correctly() {
        let bytes = make_int4range_bytes(true, Some(5), false, None);
        assert_eq!(format_pg_binary_value("int4range", &bytes), "[5,)");
    }

    #[test]
    fn pg_numeric_decoder_basic() {
        // 1234.5678: ndigits=2, weight=0, sign=0, dscale=4, groups=[1234,5678]
        let mut b: Vec<u8> = Vec::new();
        b.extend_from_slice(&2u16.to_be_bytes()); // ndigits
        b.extend_from_slice(&0i16.to_be_bytes()); // weight
        b.extend_from_slice(&0u16.to_be_bytes()); // sign
        b.extend_from_slice(&4u16.to_be_bytes()); // dscale
        b.extend_from_slice(&1234u16.to_be_bytes());
        b.extend_from_slice(&5678u16.to_be_bytes());
        assert_eq!(decode_pg_numeric(&b), "1234.5678");
    }

    #[test]
    fn pg_numeric_decoder_zero() {
        let mut b: Vec<u8> = Vec::new();
        b.extend_from_slice(&0u16.to_be_bytes()); // ndigits=0
        b.extend_from_slice(&0i16.to_be_bytes()); // weight
        b.extend_from_slice(&0u16.to_be_bytes()); // sign
        b.extend_from_slice(&0u16.to_be_bytes()); // dscale
        assert_eq!(decode_pg_numeric(&b), "0");
    }
}
