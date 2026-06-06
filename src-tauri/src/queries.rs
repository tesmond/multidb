use crate::models::ExecuteResult;
use anyhow::{anyhow, Result};
use futures_util::StreamExt;
use serde_json::{Number, Value};
use sqlx::{mysql::MySqlRow, postgres::PgRow, AnyPool, Column, MySqlPool, PgPool, Row, TypeInfo, ValueRef};
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
        1_000_000
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
        return Ok(Value::String(value));
    }
    if let Ok(value) = row.try_get::<Vec<u8>, _>(idx) {
        return Ok(Value::String(String::from_utf8_lossy(&value).to_string()));
    }

    Err(anyhow!("unsupported value in column {idx}"))
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
    if let Ok(value) = row.try_get::<sqlx::types::chrono::NaiveDateTime, _>(idx) {
        return Ok(Value::String(value.to_string()));
    }
    if let Ok(value) = row.try_get::<sqlx::types::chrono::NaiveDate, _>(idx) {
        return Ok(Value::String(value.to_string()));
    }
    if let Ok(value) = row.try_get::<sqlx::types::chrono::NaiveTime, _>(idx) {
        return Ok(Value::String(value.to_string()));
    }
    if let Ok(value) = row.try_get::<String, _>(idx) {
        return Ok(Value::String(value));
    }
    if let Ok(value) = row.try_get::<Vec<u8>, _>(idx) {
        return Ok(Value::String(String::from_utf8_lossy(&value).to_string()));
    }

    Err(anyhow!("unsupported postgres value in column {idx}"))
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
        return Ok(value.0);
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
    if let Ok(value) = row.try_get::<sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>, _>(idx) {
        return Ok(Value::String(value.to_rfc3339()));
    }
    if let Ok(value) = row.try_get::<sqlx::mysql::types::MySqlTime, _>(idx) {
        return Ok(Value::String(value.to_string()));
    }
    if let Ok(value) = row.try_get::<String, _>(idx) {
        return Ok(Value::String(value));
    }
    if let Ok(value) = row.try_get::<Vec<u8>, _>(idx) {
        return Ok(Value::String(String::from_utf8_lossy(&value).to_string()));
    }

    let ty = row
        .columns()
        .get(idx)
        .map(|col| col.type_info().name().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    Ok(Value::String(format!("<unsupported mysql:{ty}>")))
}

pub fn elapsed_ms(start: Instant) -> i64 {
    start.elapsed().as_millis() as i64
}
