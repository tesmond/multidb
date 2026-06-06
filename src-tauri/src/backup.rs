use crate::{
    connections::build_dsn,
    models::{ConnectionConfig, TableBackupArchive, TableBackupPayload},
    queries::{row_to_json_values, split_statements},
};
use anyhow::{anyhow, Context, Result};
use flate2::read::GzDecoder;
use futures_util::StreamExt;
use serde_json::{Map, Value};
use sqlx::{AnyPool, Column, Row};
use std::{
    fs,
    io::{Cursor, Read, Write},
    path::Path,
};
use zip::{write::FileOptions, ZipArchive, ZipWriter};

pub async fn backup_table(
    pool: &AnyPool,
    cfg: &ConnectionConfig,
    table_name: &str,
    schema_name: &str,
    target_path: &Path,
) -> Result<()> {
    let payload = build_table_backup_payload(pool, cfg, table_name, schema_name).await?;
    let archive = TableBackupArchive {
        version: 1,
        table: payload,
    };
    let data = serde_json::to_vec_pretty(&archive)?;

    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer.start_file("table-backup.json", FileOptions::<()>::default())?;
    writer.write_all(&data)?;
    let buffer = writer.finish()?.into_inner();
    fs::write(target_path, buffer).with_context(|| format!("write {}", target_path.display()))?;
    Ok(())
}

pub async fn import_table(
    pool: &AnyPool,
    cfg: &ConnectionConfig,
    import_type: &str,
    source_path: &Path,
) -> Result<()> {
    match import_type {
        "zipped-sql" => import_backup_archive(pool, cfg, source_path).await,
        "pgdump" => import_pg_dump(pool, cfg, source_path).await,
        other => Err(anyhow!("unknown import type: {other}")),
    }
}

pub async fn drop_table(
    pool: &AnyPool,
    cfg: &ConnectionConfig,
    table_name: &str,
    schema_name: &str,
) -> Result<()> {
    if table_name.trim().is_empty() {
        return Err(anyhow!("table name is required"));
    }

    let qualified = qualified_table_name(&cfg.driver, table_name, schema_name);
    let mut statement = format!("DROP TABLE IF EXISTS {qualified}");
    if cfg.driver == "postgres" {
        statement.push_str(" CASCADE");
    }
    sqlx::query(&statement).execute(pool).await?;
    Ok(())
}

async fn build_table_backup_payload(
    pool: &AnyPool,
    cfg: &ConnectionConfig,
    table_name: &str,
    schema_name: &str,
) -> Result<TableBackupPayload> {
    Ok(TableBackupPayload {
        driver: cfg.driver.clone(),
        schema_name: schema_name.to_string(),
        table_name: table_name.to_string(),
        create_sql: get_create_table_sql(pool, cfg, table_name, schema_name).await?,
        indexes_sql: get_create_indexes_sql(pool, cfg, table_name, schema_name).await?,
        columns: get_table_columns(pool, cfg, table_name, schema_name).await?,
        rows: get_table_rows(pool, cfg, table_name, schema_name).await?,
        created_at: unix_now_string(),
    })
}

async fn get_create_table_sql(
    pool: &AnyPool,
    cfg: &ConnectionConfig,
    table_name: &str,
    schema_name: &str,
) -> Result<String> {
    match cfg.driver.as_str() {
        "sqlite" => {
            let row =
                sqlx::query("SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?")
                    .bind(table_name)
                    .fetch_one(pool)
                    .await?;
            Ok(row.try_get::<String, _>(0)?.trim().to_string())
        }
        "postgres" => {
            let qualified = qualified_table_name("postgres", table_name, schema_name);
            let row = sqlx::query(
                r#"
                SELECT 'CREATE TABLE ' || $1 || E' (\n' ||
                       string_agg(
                           '  ' || quote_ident(a.attname) || ' ' ||
                           pg_catalog.format_type(a.atttypid, a.atttypmod) ||
                           CASE WHEN a.attnotnull THEN ' NOT NULL' ELSE '' END ||
                           CASE WHEN d.adbin IS NOT NULL THEN ' DEFAULT ' || pg_get_expr(d.adbin, d.adrelid) ELSE '' END,
                           E',\n' ORDER BY a.attnum
                       ) || E'\n);'
                FROM pg_attribute a
                JOIN pg_class c ON c.oid = a.attrelid
                JOIN pg_namespace n ON n.oid = c.relnamespace
                LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum
                WHERE c.relkind = 'r'
                  AND c.relname = $2
                  AND n.nspname = $3
                  AND a.attnum > 0
                  AND NOT a.attisdropped
                GROUP BY c.oid
                "#,
            )
            .bind(qualified)
            .bind(table_name)
            .bind(schema_name)
            .fetch_one(pool)
            .await?;
            Ok(row.try_get::<String, _>(0)?.trim().to_string())
        }
        "mysql" => {
            let qualified = qualified_table_name("mysql", table_name, schema_name);
            let row = sqlx::query(&format!("SHOW CREATE TABLE {qualified}"))
                .fetch_one(pool)
                .await?;
            Ok(row.try_get::<String, _>(1)?.trim().to_string())
        }
        other => Err(anyhow!("backup not supported for driver: {other}")),
    }
}

async fn get_create_indexes_sql(
    pool: &AnyPool,
    cfg: &ConnectionConfig,
    table_name: &str,
    schema_name: &str,
) -> Result<Vec<String>> {
    match cfg.driver.as_str() {
        "sqlite" => string_column(
            pool,
            r#"
            SELECT sql
            FROM sqlite_master
            WHERE type = 'index' AND tbl_name = ? AND sql IS NOT NULL
            ORDER BY name
            "#,
            &[table_name],
        )
        .await,
        "postgres" => string_column(
            pool,
            "SELECT indexdef::text FROM pg_indexes WHERE schemaname = $1 AND tablename = $2 ORDER BY indexname",
            &[schema_name, table_name],
        )
        .await,
        "mysql" => {
            let rows = sqlx::query(
                r#"
                SELECT INDEX_NAME, NON_UNIQUE, GROUP_CONCAT(COLUMN_NAME ORDER BY SEQ_IN_INDEX SEPARATOR ',')
                FROM information_schema.STATISTICS
                WHERE TABLE_SCHEMA = DATABASE()
                  AND TABLE_NAME = ?
                  AND INDEX_NAME <> 'PRIMARY'
                GROUP BY INDEX_NAME, NON_UNIQUE
                ORDER BY INDEX_NAME
                "#,
            )
            .bind(table_name)
            .fetch_all(pool)
            .await?;
            let mut out = Vec::new();
            for row in rows {
                let name: String = row.try_get(0)?;
                let non_unique: i64 = row.try_get(1)?;
                let cols: String = row.try_get(2)?;
                let mut prefix = "CREATE ".to_string();
                if non_unique == 0 {
                    prefix.push_str("UNIQUE ");
                }
                out.push(format!(
                    "{}INDEX {} ON {} ({})",
                    prefix,
                    quote_identifier("mysql", &name),
                    qualified_table_name("mysql", table_name, schema_name),
                    join_quoted_columns("mysql", &cols.split(',').collect::<Vec<_>>())
                ));
            }
            Ok(out)
        }
        other => Err(anyhow!("backup not supported for driver: {other}")),
    }
}

async fn get_table_columns(
    pool: &AnyPool,
    cfg: &ConnectionConfig,
    table_name: &str,
    schema_name: &str,
) -> Result<Vec<String>> {
    let query = format!(
        "SELECT * FROM {} LIMIT 0",
        qualified_table_name(&cfg.driver, table_name, schema_name)
    );
    let mut stream = sqlx::query(&query).fetch(pool);
    if let Some(row) = stream.next().await {
        let row = row?;
        return Ok(row
            .columns()
            .iter()
            .map(|col| col.name().to_string())
            .collect());
    }

    match cfg.driver.as_str() {
        "sqlite" => {
            let rows = sqlx::query(&format!("PRAGMA table_info({})", quote_identifier("sqlite", table_name)))
                .fetch_all(pool)
                .await?;
            rows.into_iter().map(|row| row.try_get(1).map_err(Into::into)).collect()
        }
        "postgres" => string_column(
            pool,
            "SELECT column_name::text FROM information_schema.columns WHERE table_schema = $1 AND table_name = $2 ORDER BY ordinal_position",
            &[schema_name, table_name],
        )
        .await,
        "mysql" => string_column(
            pool,
            "SELECT COLUMN_NAME FROM information_schema.COLUMNS WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = ? ORDER BY ORDINAL_POSITION",
            &[table_name],
        )
        .await,
        other => Err(anyhow!("backup not supported for driver: {other}")),
    }
}

async fn get_table_rows(
    pool: &AnyPool,
    cfg: &ConnectionConfig,
    table_name: &str,
    schema_name: &str,
) -> Result<Vec<Map<String, Value>>> {
    let query = format!(
        "SELECT * FROM {}",
        qualified_table_name(&cfg.driver, table_name, schema_name)
    );
    let mut stream = sqlx::query(&query).fetch(pool);
    let mut out = Vec::new();

    while let Some(row) = stream.next().await {
        let row = row?;
        let columns: Vec<_> = row
            .columns()
            .iter()
            .map(|col| col.name().to_string())
            .collect();
        let values = row_to_json_values(&row)?;
        let mut row_map = Map::new();
        for (column, value) in columns.into_iter().zip(values.into_iter()) {
            row_map.insert(column, value);
        }
        out.push(row_map);
    }
    Ok(out)
}

async fn import_backup_archive(
    pool: &AnyPool,
    cfg: &ConnectionConfig,
    source_path: &Path,
) -> Result<()> {
    let archive: TableBackupArchive = {
        let data =
            fs::read(source_path).with_context(|| format!("read {}", source_path.display()))?;
        let mut archive = ZipArchive::new(Cursor::new(data))?;
        let mut file = archive
            .by_name("table-backup.json")
            .context("backup archive does not contain table-backup.json")?;
        let mut payload = String::new();
        file.read_to_string(&mut payload)?;
        serde_json::from_str(&payload)?
    };

    if archive.version != 1 {
        return Err(anyhow!("unsupported backup version: {}", archive.version));
    }
    if archive.table.driver != cfg.driver {
        return Err(anyhow!(
            "backup archive is for {}, but connection uses {}",
            archive.table.driver,
            cfg.driver
        ));
    }
    if table_exists(
        pool,
        cfg,
        &archive.table.table_name,
        &archive.table.schema_name,
    )
    .await?
    {
        return Err(anyhow!(
            "table {} already exists",
            qualified_table_name(
                &cfg.driver,
                &archive.table.table_name,
                &archive.table.schema_name
            )
        ));
    }

    sqlx::query("BEGIN").execute(pool).await?;
    let import_result = async {
        sqlx::query(&archive.table.create_sql).execute(pool).await?;
        for statement in &archive.table.indexes_sql {
            if !statement.trim().is_empty() {
                sqlx::query(statement).execute(pool).await?;
            }
        }
        insert_rows(pool, cfg, &archive.table).await
    }
    .await;

    if import_result.is_ok() {
        sqlx::query("COMMIT").execute(pool).await?;
    } else {
        let _ = sqlx::query("ROLLBACK").execute(pool).await;
    }
    import_result
}

async fn insert_rows(
    pool: &AnyPool,
    cfg: &ConnectionConfig,
    payload: &TableBackupPayload,
) -> Result<()> {
    if payload.columns.is_empty() {
        return Ok(());
    }

    let placeholders = (1..=payload.columns.len())
        .map(|idx| placeholder(&cfg.driver, idx))
        .collect::<Vec<_>>()
        .join(", ");
    let statement = format!(
        "INSERT INTO {} ({}) VALUES ({})",
        qualified_table_name(&cfg.driver, &payload.table_name, &payload.schema_name),
        join_quoted_columns(
            &cfg.driver,
            &payload
                .columns
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
        ),
        placeholders
    );

    for row in &payload.rows {
        let mut query = sqlx::query(&statement);
        for column in &payload.columns {
            query = bind_json_value(query, row.get(column).unwrap_or(&Value::Null));
        }
        query.execute(pool).await?;
    }
    Ok(())
}

async fn import_pg_dump(pool: &AnyPool, cfg: &ConnectionConfig, source_path: &Path) -> Result<()> {
    if cfg.driver != "postgres" {
        return Err(anyhow!("pg_dump import requires a PostgreSQL connection"));
    }
    let mut data = fs::read(source_path)?;
    if source_path.extension().is_some_and(|ext| ext == "gz") {
        let mut decoder = GzDecoder::new(Cursor::new(data));
        let mut decoded = Vec::new();
        decoder.read_to_end(&mut decoded)?;
        data = decoded;
    }
    let text = String::from_utf8(data).context("pg_dump is not valid UTF-8")?;
    for segment in parse_pg_dump_segments(&text)? {
        match segment {
            PgDumpSegment::Sql(sql) => {
                for statement in split_statements(&sql) {
                    if let Err(err) = sqlx::query(&statement).execute(pool).await {
                        if !should_ignore_pg_dump_error(&err) {
                            return Err(err.into());
                        }
                    }
                }
            }
            PgDumpSegment::Copy {
                table,
                columns,
                rows,
            } => {
                let placeholders = (1..=columns.len())
                    .map(|idx| placeholder("postgres", idx))
                    .collect::<Vec<_>>()
                    .join(", ");
                let statement = format!(
                    "INSERT INTO {} ({}) VALUES ({})",
                    table,
                    join_quoted_columns(
                        "postgres",
                        &columns.iter().map(String::as_str).collect::<Vec<_>>()
                    ),
                    placeholders
                );
                for row in rows {
                    let mut query = sqlx::query(&statement);
                    for value in row {
                        query = match value {
                            Some(value) => query.bind(value),
                            None => query.bind(Option::<String>::None),
                        };
                    }
                    query.execute(pool).await?;
                }
            }
        }
    }
    let _ = build_dsn(cfg)?;
    Ok(())
}

enum PgDumpSegment {
    Sql(String),
    Copy {
        table: String,
        columns: Vec<String>,
        rows: Vec<Vec<Option<String>>>,
    },
}

fn parse_pg_dump_segments(data: &str) -> Result<Vec<PgDumpSegment>> {
    let mut segments = Vec::new();
    let mut sql = String::new();
    let mut in_copy = false;
    let mut copy_table = String::new();
    let mut copy_columns = Vec::new();
    let mut copy_rows = Vec::new();

    for raw in data.lines() {
        let line = raw.trim_end_matches('\r');
        let trimmed = line.trim();

        if in_copy {
            if trimmed == r"\." {
                segments.push(PgDumpSegment::Copy {
                    table: copy_table.clone(),
                    columns: copy_columns.clone(),
                    rows: std::mem::take(&mut copy_rows),
                });
                in_copy = false;
                copy_table.clear();
                copy_columns.clear();
            } else if !trimmed.is_empty() {
                copy_rows.push(
                    line.split('\t')
                        .map(|part| (part != r"\N").then(|| part.to_string()))
                        .collect(),
                );
            }
            continue;
        }

        if trimmed.is_empty() || trimmed.starts_with("--") {
            continue;
        }

        if !sql.is_empty() {
            sql.push('\n');
        }
        sql.push_str(line);

        if sql.trim_start().to_ascii_uppercase().starts_with("COPY ")
            && sql.to_ascii_uppercase().contains("FROM STDIN;")
        {
            let (table, columns) = parse_copy_statement(&sql)?;
            copy_table = table;
            copy_columns = columns;
            sql.clear();
            in_copy = true;
            continue;
        }

        if trimmed.ends_with(';') {
            segments.push(PgDumpSegment::Sql(std::mem::take(&mut sql)));
        }
    }

    if in_copy {
        return Err(anyhow!("unterminated COPY section"));
    }
    if !sql.trim().is_empty() {
        segments.push(PgDumpSegment::Sql(sql));
    }
    Ok(segments)
}

fn parse_copy_statement(statement: &str) -> Result<(String, Vec<String>)> {
    let upper = statement.to_ascii_uppercase();
    let after_copy = statement
        .get(5..)
        .ok_or_else(|| anyhow!("invalid COPY statement"))?;
    let paren_start = after_copy
        .find('(')
        .ok_or_else(|| anyhow!("unsupported COPY statement: {statement}"))?;
    let paren_end = after_copy
        .rfind(')')
        .ok_or_else(|| anyhow!("unsupported COPY statement: {statement}"))?;
    if !upper.contains("FROM STDIN") {
        return Err(anyhow!("unsupported COPY statement: {statement}"));
    }
    let table = after_copy[..paren_start].trim().to_string();
    let columns = after_copy[paren_start + 1..paren_end]
        .split(',')
        .map(|part| unquote_identifier(part.trim()).to_string())
        .collect();
    Ok((table, columns))
}

fn should_ignore_pg_dump_error(err: &sqlx::Error) -> bool {
    let message = err.to_string().to_ascii_lowercase();
    message.contains("already exists")
        || message.contains("duplicate")
        || message.contains("duplicate_table")
        || message.contains("duplicate_object")
        || message.contains("duplicate_schema")
}

async fn table_exists(
    pool: &AnyPool,
    cfg: &ConnectionConfig,
    table_name: &str,
    schema_name: &str,
) -> Result<bool> {
    let count: i64 = match cfg.driver.as_str() {
        "sqlite" => sqlx::query("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?")
            .bind(table_name)
            .fetch_one(pool)
            .await?
            .try_get(0)?,
        "postgres" => sqlx::query(
            "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = $1 AND table_name = $2",
        )
        .bind(schema_name)
        .bind(table_name)
        .fetch_one(pool)
        .await?
        .try_get(0)?,
        "mysql" => sqlx::query(
            "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name = ?",
        )
        .bind(table_name)
        .fetch_one(pool)
        .await?
        .try_get(0)?,
        other => return Err(anyhow!("import not supported for driver: {other}")),
    };
    Ok(count > 0)
}

fn bind_json_value<'q>(
    query: sqlx::query::Query<'q, sqlx::Any, sqlx::any::AnyArguments<'q>>,
    value: &'q Value,
) -> sqlx::query::Query<'q, sqlx::Any, sqlx::any::AnyArguments<'q>> {
    match value {
        Value::Null => query.bind(Option::<String>::None),
        Value::Bool(value) => query.bind(*value),
        Value::Number(value) if value.is_i64() => query.bind(value.as_i64().unwrap_or_default()),
        Value::Number(value) if value.is_u64() => {
            query.bind(value.as_u64().unwrap_or_default() as i64)
        }
        Value::Number(value) => query.bind(value.as_f64().unwrap_or_default()),
        Value::String(value) => query.bind(value),
        other => query.bind(other.to_string()),
    }
}

async fn string_column(pool: &AnyPool, sql: &str, args: &[&str]) -> Result<Vec<String>> {
    let mut query = sqlx::query(sql);
    for arg in args {
        query = query.bind(*arg);
    }
    let rows = query.fetch_all(pool).await?;
    rows.into_iter()
        .map(|row| row.try_get(0).map_err(Into::into))
        .collect()
}

pub fn qualified_table_name(driver: &str, table_name: &str, schema_name: &str) -> String {
    if schema_name.is_empty() {
        quote_identifier(driver, table_name)
    } else {
        format!(
            "{}.{}",
            quote_identifier(driver, schema_name),
            quote_identifier(driver, table_name)
        )
    }
}

pub fn quote_identifier(driver: &str, name: &str) -> String {
    if driver == "mysql" {
        format!("`{}`", name.replace('`', "``"))
    } else {
        format!("\"{}\"", name.replace('"', "\"\""))
    }
}

fn join_quoted_columns(driver: &str, columns: &[&str]) -> String {
    columns
        .iter()
        .map(|column| quote_identifier(driver, column.trim()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn placeholder(driver: &str, index: usize) -> String {
    if driver == "postgres" {
        format!("${index}")
    } else {
        "?".to_string()
    }
}

fn unquote_identifier(name: &str) -> &str {
    name.trim().trim_matches('"')
}

fn unix_now_string() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_default()
}
