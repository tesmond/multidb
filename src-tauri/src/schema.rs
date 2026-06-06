use crate::models::{Column as DbColumn, Schema, SchemaTree, Table};
use anyhow::{anyhow, Result};
use sqlx::{AnyPool, Row};
use std::collections::HashMap;

pub async fn get_schema(pool: &AnyPool, driver: &str) -> Result<SchemaTree> {
    match driver {
        "mysql" => mysql_schema(pool).await,
        "postgres" => postgres_schema(pool).await,
        "sqlite" => sqlite_schema(pool).await,
        other => Err(anyhow!("unsupported driver: {other}")),
    }
}

pub async fn get_primary_keys(
    pool: &AnyPool,
    driver: &str,
    schema_name: &str,
    table_name: &str,
) -> Result<Vec<String>> {
    match driver {
        "mysql" => {
            string_column(
                pool,
                r#"
            SELECT COLUMN_NAME FROM information_schema.COLUMNS
            WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? AND COLUMN_KEY = 'PRI'
            ORDER BY ORDINAL_POSITION
            "#,
                &[schema_name, table_name],
            )
            .await
        }
        "postgres" => {
            let schema_name = if schema_name.is_empty() {
                "public"
            } else {
                schema_name
            };
            string_column(
                pool,
                r#"
                SELECT kcu.column_name::text
                FROM information_schema.table_constraints tc
                JOIN information_schema.key_column_usage kcu
                  ON tc.constraint_name = kcu.constraint_name
                 AND tc.table_schema = kcu.table_schema
                 AND tc.table_name = kcu.table_name
                WHERE tc.constraint_type = 'PRIMARY KEY'
                  AND tc.table_schema = $1
                  AND tc.table_name = $2
                ORDER BY kcu.ordinal_position
                "#,
                &[schema_name, table_name],
            )
            .await
        }
        "sqlite" => {
            let rows = sqlx::query(&format!(
                "PRAGMA table_info({})",
                quote_sqlite_pragma(table_name)
            ))
            .fetch_all(pool)
            .await?;
            let mut entries: Vec<(i64, String)> = rows
                .into_iter()
                .filter_map(|row| {
                    let name: String = row.try_get("name").ok()?;
                    let pk: i64 = row.try_get("pk").ok()?;
                    (pk > 0).then_some((pk, name))
                })
                .collect();
            entries.sort_by_key(|(pk, _)| *pk);
            Ok(entries.into_iter().map(|(_, name)| name).collect())
        }
        other => Err(anyhow!("unsupported driver: {other}")),
    }
}

async fn mysql_schema(pool: &AnyPool) -> Result<SchemaTree> {
    let sizes = mysql_table_sizes(pool).await.unwrap_or_default();
    let db_rows = sqlx::query(
        r#"
        SELECT SCHEMA_NAME FROM information_schema.SCHEMATA
        WHERE SCHEMA_NAME NOT IN ('information_schema','performance_schema','mysql')
        ORDER BY SCHEMA_NAME
        "#,
    )
    .fetch_all(pool)
    .await?;

    let mut tree = SchemaTree::default();
    let mut total_size = 0_i64;
    for row in db_rows {
        let db_name: String = row.try_get(0)?;
        let mut schema = Schema {
            name: db_name.clone(),
            ..Schema::default()
        };

        let tables = sqlx::query(
            r#"
            SELECT TABLE_NAME, TABLE_TYPE
            FROM information_schema.TABLES
            WHERE TABLE_SCHEMA = ?
            ORDER BY TABLE_NAME
            "#,
        )
        .bind(&db_name)
        .fetch_all(pool)
        .await?;

        for row in tables {
            let name: String = row.try_get(0)?;
            let table_type: String = row.try_get(1)?;
            let mut table = Table {
                name: name.clone(),
                table_type: if table_type == "VIEW" {
                    "VIEW"
                } else {
                    "TABLE"
                }
                .to_string(),
                size_bytes: sizes.get(&format!("{db_name}.{name}")).copied(),
                ..Table::default()
            };
            if table.table_type == "TABLE" {
                table.columns = mysql_columns(pool, &db_name, &table.name).await?;
                schema.tables.push(table);
            } else {
                schema.views.push(table);
            }
        }

        schema.indexes = string_column(
            pool,
            r#"
            SELECT DISTINCT INDEX_NAME
            FROM information_schema.STATISTICS
            WHERE TABLE_SCHEMA = ?
            ORDER BY INDEX_NAME
            "#,
            &[&db_name],
        )
        .await
        .unwrap_or_default();
        let schema_size: i64 = schema
            .tables
            .iter()
            .filter_map(|table| table.size_bytes)
            .sum();
        schema.size_bytes = Some(schema_size);
        total_size += schema_size;
        tree.schemas.push(schema);
    }
    tree.size_bytes = Some(total_size);
    Ok(tree)
}

async fn mysql_table_sizes(pool: &AnyPool) -> Result<HashMap<String, i64>> {
    let rows = sqlx::query(
        r#"
        SELECT TABLE_SCHEMA, TABLE_NAME, COALESCE(DATA_LENGTH, 0) + COALESCE(INDEX_LENGTH, 0)
        FROM information_schema.TABLES
        WHERE TABLE_SCHEMA NOT IN ('information_schema','performance_schema','mysql')
          AND TABLE_TYPE = 'BASE TABLE'
        "#,
    )
    .fetch_all(pool)
    .await?;
    let mut out = HashMap::new();
    for row in rows {
        let schema: String = row.try_get(0)?;
        let table: String = row.try_get(1)?;
        let size: i64 = row.try_get(2)?;
        out.insert(format!("{schema}.{table}"), size);
    }
    Ok(out)
}

async fn mysql_columns(pool: &AnyPool, db_name: &str, table: &str) -> Result<Vec<DbColumn>> {
    let rows = sqlx::query(
        r#"
        SELECT COLUMN_NAME, COLUMN_TYPE, IS_NULLABLE, IFNULL(COLUMN_DEFAULT,''), COLUMN_KEY
        FROM information_schema.COLUMNS
        WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?
        ORDER BY ORDINAL_POSITION
        "#,
    )
    .bind(db_name)
    .bind(table)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(DbColumn {
                name: row.try_get(0)?,
                column_type: row.try_get(1)?,
                nullable: row.try_get::<String, _>(2)? == "YES",
                default: row.try_get(3)?,
                key: row.try_get(4)?,
            })
        })
        .collect()
}

async fn postgres_schema(pool: &AnyPool) -> Result<SchemaTree> {
    let sizes = postgres_table_sizes(pool).await.unwrap_or_default();
    let schema_names = string_column(
        pool,
        r#"
        SELECT schema_name::text FROM information_schema.schemata
        WHERE schema_name NOT IN ('pg_catalog', 'information_schema')
          AND schema_name NOT LIKE 'pg_%'
        ORDER BY schema_name
        "#,
        &[],
    )
    .await?;

    let mut tree = SchemaTree::default();
    let mut total_size = 0_i64;
    for schema_name in schema_names {
        let mut schema = Schema {
            name: schema_name.clone(),
            ..Schema::default()
        };

        let rows = sqlx::query(
            r#"
            SELECT table_name::text, table_type::text
            FROM information_schema.tables
            WHERE table_schema = $1
            ORDER BY table_name
            "#,
        )
        .bind(&schema_name)
        .fetch_all(pool)
        .await?;

        for row in rows {
            let name: String = row.try_get(0)?;
            let table_type: String = row.try_get(1)?;
            let mut table = Table {
                name: name.clone(),
                table_type: if table_type == "VIEW" {
                    "VIEW"
                } else {
                    "TABLE"
                }
                .to_string(),
                size_bytes: sizes.get(&format!("{schema_name}.{name}")).copied(),
                ..Table::default()
            };
            if table.table_type == "TABLE" {
                table.columns = postgres_columns(pool, &schema_name, &table.name).await?;
                schema.tables.push(table);
            } else {
                schema.views.push(table);
            }
        }

        schema.indexes = string_column(
            pool,
            "SELECT indexname::text FROM pg_indexes WHERE schemaname = $1 ORDER BY indexname",
            &[&schema_name],
        )
        .await
        .unwrap_or_default();
        let schema_size: i64 = schema
            .tables
            .iter()
            .filter_map(|table| table.size_bytes)
            .sum();
        schema.size_bytes = Some(schema_size);
        total_size += schema_size;
        tree.schemas.push(schema);
    }
    tree.size_bytes = Some(total_size);
    Ok(tree)
}

async fn postgres_table_sizes(pool: &AnyPool) -> Result<HashMap<String, i64>> {
    let rows = sqlx::query(
        r#"
        SELECT n.nspname::text, c.relname::text, pg_total_relation_size(c.oid)
        FROM pg_class c
        JOIN pg_namespace n ON n.oid = c.relnamespace
        WHERE c.relkind IN ('r', 'p')
          AND n.nspname NOT IN ('pg_catalog', 'information_schema')
          AND n.nspname NOT LIKE 'pg_%'
        "#,
    )
    .fetch_all(pool)
    .await?;
    let mut out = HashMap::new();
    for row in rows {
        let schema: String = row.try_get(0)?;
        let table: String = row.try_get(1)?;
        let size: i64 = row.try_get(2)?;
        out.insert(format!("{schema}.{table}"), size);
    }
    Ok(out)
}

async fn postgres_columns(pool: &AnyPool, schema_name: &str, table: &str) -> Result<Vec<DbColumn>> {
    let rows = sqlx::query(
        r#"
        SELECT column_name::text, data_type::text, is_nullable::text, COALESCE(column_default, '')
        FROM information_schema.columns
        WHERE table_schema = $1 AND table_name = $2
        ORDER BY ordinal_position
        "#,
    )
    .bind(schema_name)
    .bind(table)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(DbColumn {
                name: row.try_get(0)?,
                column_type: row.try_get(1)?,
                nullable: row.try_get::<String, _>(2)? == "YES",
                default: row.try_get(3)?,
                key: String::new(),
            })
        })
        .collect()
}

async fn sqlite_schema(pool: &AnyPool) -> Result<SchemaTree> {
    let table_sizes = sqlite_table_sizes(pool).await.unwrap_or_default();
    let mut tree = SchemaTree {
        size_bytes: sqlite_database_size(pool).await.ok(),
        ..SchemaTree::default()
    };

    let rows = sqlx::query(
        r#"
        SELECT name, type FROM sqlite_master
        WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%'
        ORDER BY name
        "#,
    )
    .fetch_all(pool)
    .await?;

    for row in rows {
        let name: String = row.try_get(0)?;
        let obj_type: String = row.try_get(1)?;
        let mut table = Table {
            name: name.clone(),
            table_type: if obj_type == "view" { "VIEW" } else { "TABLE" }.to_string(),
            size_bytes: table_sizes.get(&name).copied(),
            ..Table::default()
        };
        if table.table_type == "TABLE" {
            table.columns = sqlite_columns(pool, &table.name).await?;
            tree.tables.push(table);
        } else {
            tree.views.push(table);
        }
    }

    tree.indexes = string_column(
        pool,
        "SELECT name FROM sqlite_master WHERE type = 'index' ORDER BY name",
        &[],
    )
    .await
    .unwrap_or_default();

    Ok(tree)
}

async fn sqlite_columns(pool: &AnyPool, table: &str) -> Result<Vec<DbColumn>> {
    let rows = sqlx::query(&format!(
        "PRAGMA table_info({})",
        quote_sqlite_pragma(table)
    ))
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            let not_null: i64 = row.try_get("notnull")?;
            let pk: i64 = row.try_get("pk")?;
            Ok(DbColumn {
                name: row.try_get("name")?,
                column_type: row.try_get("type")?,
                nullable: not_null == 0,
                default: row
                    .try_get::<Option<String>, _>("dflt_value")?
                    .unwrap_or_default(),
                key: if pk > 0 {
                    "PRI".to_string()
                } else {
                    String::new()
                },
            })
        })
        .collect()
}

async fn sqlite_table_sizes(pool: &AnyPool) -> Result<HashMap<String, i64>> {
    let rows = sqlx::query(
        r#"
        SELECT COALESCE(m.tbl_name, d.name), SUM(d.pgsize)
        FROM dbstat d
        LEFT JOIN sqlite_master m ON m.type = 'index' AND m.name = d.name
        WHERE d.name NOT LIKE 'sqlite_%'
        GROUP BY COALESCE(m.tbl_name, d.name)
        "#,
    )
    .fetch_all(pool)
    .await?;
    let mut out = HashMap::new();
    for row in rows {
        let name: String = row.try_get(0)?;
        let size: Option<i64> = row.try_get(1)?;
        if let Some(size) = size {
            out.insert(name, size);
        }
    }
    Ok(out)
}

async fn sqlite_database_size(pool: &AnyPool) -> Result<i64> {
    let page_count: i64 = sqlx::query("PRAGMA page_count")
        .fetch_one(pool)
        .await?
        .try_get(0)?;
    let page_size: i64 = sqlx::query("PRAGMA page_size")
        .fetch_one(pool)
        .await?
        .try_get(0)?;
    Ok(page_count * page_size)
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

fn quote_sqlite_pragma(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}
