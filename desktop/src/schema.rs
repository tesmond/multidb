use crate::models::{
    Column as DbColumn, Relationship, RelationshipColumnPair, RelationshipTableRef, Schema,
    SchemaTree, Table,
};
use anyhow::{anyhow, Result};
use sqlx::{any::AnyRow, mysql::MySqlRow, AnyPool, ColumnIndex, MySqlPool, Row, ValueRef};
use std::collections::BTreeMap;
use std::collections::HashMap;

pub async fn get_schema(pool: &AnyPool, driver: &str) -> Result<SchemaTree> {
    match driver {
        "mysql" => mysql_schema(pool).await,
        "postgres" => postgres_schema(pool).await,
        "sqlite" => sqlite_schema(pool).await,
        other => Err(anyhow!("unsupported driver: {other}")),
    }
}

pub async fn postgres_database_catalog(pool: &AnyPool) -> Result<SchemaTree> {
    let databases = string_column(
        pool,
        r#"
        SELECT datname::text
        FROM pg_database
        WHERE datistemplate = false
          AND datallowconn = true
        ORDER BY datname
        "#,
        &[],
    )
    .await?;

    let mut tree = SchemaTree::default();
    tree.schemas = databases
        .into_iter()
        .map(|database_name| Schema {
            name: database_name,
            ..Schema::default()
        })
        .collect();
    Ok(tree)
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
                    let name = decode_any_text(&row, "name").ok()?;
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

pub async fn get_mysql_primary_keys(
    pool: &MySqlPool,
    schema_name: &str,
    table_name: &str,
) -> Result<Vec<String>> {
    string_column_mysql(
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

async fn mysql_schema(pool: &AnyPool) -> Result<SchemaTree> {
    let sizes = mysql_table_sizes(pool).await.unwrap_or_default();
    let relationships = mysql_relationships(pool).await?;
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
        let db_name = decode_any_text(&row, 0)?;
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
            let name = decode_any_text(&row, 0)?;
            let table_type = decode_any_text(&row, 1)?;
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
    tree.relationships = relationships;
    Ok(tree)
}

pub async fn get_mysql_schema(pool: &MySqlPool) -> Result<SchemaTree> {
    let sizes = mysql_table_sizes_typed(pool).await.unwrap_or_default();
    let relationships = mysql_relationships_typed(pool).await?;
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
        let db_name = decode_mysql_text(&row, 0)?;
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
            let name = decode_mysql_text(&row, 0)?;
            let table_type = decode_mysql_text(&row, 1)?;
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
                table.columns = mysql_columns_typed(pool, &db_name, &table.name).await?;
                schema.tables.push(table);
            } else {
                schema.views.push(table);
            }
        }

        schema.indexes = string_column_mysql(
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
    tree.relationships = relationships;
    Ok(tree)
}

async fn mysql_table_sizes(pool: &AnyPool) -> Result<HashMap<String, i64>> {
    let rows = sqlx::query(
        r#"
        SELECT
            TABLE_SCHEMA,
            TABLE_NAME,
            CAST(COALESCE(DATA_LENGTH, 0) + COALESCE(INDEX_LENGTH, 0) AS CHAR)
        FROM information_schema.TABLES
        WHERE TABLE_SCHEMA NOT IN ('information_schema','performance_schema','mysql')
          AND TABLE_TYPE = 'BASE TABLE'
        "#,
    )
    .fetch_all(pool)
    .await?;
    let mut out = HashMap::new();
    for row in rows {
        let schema = decode_any_text(&row, 0)?;
        let table = decode_any_text(&row, 1)?;
        let size = decode_any_i64(&row, 2)?;
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
                name: decode_any_text(&row, 0)?,
                column_type: decode_any_text(&row, 1)?,
                nullable: decode_any_text(&row, 2)? == "YES",
                default: decode_any_text(&row, 3)?,
                key: decode_any_text(&row, 4)?,
            })
        })
        .collect()
}

async fn mysql_table_sizes_typed(pool: &MySqlPool) -> Result<HashMap<String, i64>> {
    let rows = sqlx::query(
        r#"
        SELECT
            TABLE_SCHEMA,
            TABLE_NAME,
            CAST(COALESCE(DATA_LENGTH, 0) + COALESCE(INDEX_LENGTH, 0) AS CHAR)
        FROM information_schema.TABLES
        WHERE TABLE_SCHEMA NOT IN ('information_schema','performance_schema','mysql')
          AND TABLE_TYPE = 'BASE TABLE'
        "#,
    )
    .fetch_all(pool)
    .await?;
    let mut out = HashMap::new();
    for row in rows {
        let schema = decode_mysql_text(&row, 0)?;
        let table = decode_mysql_text(&row, 1)?;
        let size = decode_mysql_i64(&row, 2)?;
        out.insert(format!("{schema}.{table}"), size);
    }
    Ok(out)
}

async fn mysql_columns_typed(
    pool: &MySqlPool,
    db_name: &str,
    table: &str,
) -> Result<Vec<DbColumn>> {
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
                name: decode_mysql_text(&row, 0)?,
                column_type: decode_mysql_text(&row, 1)?,
                nullable: decode_mysql_text(&row, 2)? == "YES",
                default: decode_mysql_text(&row, 3)?,
                key: decode_mysql_text(&row, 4)?,
            })
        })
        .collect()
}

async fn postgres_schema(pool: &AnyPool) -> Result<SchemaTree> {
    let sizes = postgres_table_sizes(pool).await.unwrap_or_default();
    let relationships = postgres_relationships(pool).await?;
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
    tree.relationships = relationships;
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
        let name = decode_any_text(&row, 0)?;
        let obj_type = decode_any_text(&row, 1)?;
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

    tree.relationships = sqlite_relationships(
        pool,
        &tree
            .tables
            .iter()
            .map(|table| table.name.clone())
            .collect::<Vec<_>>(),
    )
    .await?;

    tree.indexes = string_column(
        pool,
        "SELECT name FROM sqlite_master WHERE type = 'index' ORDER BY name",
        &[],
    )
    .await
    .unwrap_or_default();

    Ok(tree)
}

#[derive(Debug, Clone)]
struct ForeignKeyRow {
    constraint_name: String,
    source_schema: String,
    source_table: String,
    source_column: String,
    target_schema: String,
    target_table: String,
    target_column: String,
    on_update: String,
    on_delete: String,
    position: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ForeignKeyGroupKey {
    constraint_name: String,
    source_schema: String,
    source_table: String,
    target_schema: String,
    target_table: String,
    on_update: String,
    on_delete: String,
}

async fn mysql_relationships(pool: &AnyPool) -> Result<Vec<Relationship>> {
    let rows = sqlx::query(
        r#"
        SELECT
            kcu.CONSTRAINT_NAME,
            kcu.TABLE_SCHEMA,
            kcu.TABLE_NAME,
            kcu.COLUMN_NAME,
            kcu.REFERENCED_TABLE_SCHEMA,
            kcu.REFERENCED_TABLE_NAME,
            kcu.REFERENCED_COLUMN_NAME,
            COALESCE(rc.UPDATE_RULE, ''),
            COALESCE(rc.DELETE_RULE, ''),
            kcu.ORDINAL_POSITION
        FROM information_schema.KEY_COLUMN_USAGE kcu
        JOIN information_schema.REFERENTIAL_CONSTRAINTS rc
          ON rc.CONSTRAINT_SCHEMA = kcu.CONSTRAINT_SCHEMA
         AND rc.CONSTRAINT_NAME = kcu.CONSTRAINT_NAME
        WHERE kcu.REFERENCED_TABLE_NAME IS NOT NULL
          AND kcu.TABLE_SCHEMA NOT IN ('information_schema','performance_schema','mysql')
        ORDER BY kcu.TABLE_SCHEMA, kcu.TABLE_NAME, kcu.CONSTRAINT_NAME, kcu.ORDINAL_POSITION
        "#,
    )
    .fetch_all(pool)
    .await?;

    let fk_rows = rows
        .into_iter()
        .map(|row| {
            Ok(ForeignKeyRow {
                constraint_name: decode_any_text(&row, 0)?,
                source_schema: decode_any_text(&row, 1)?,
                source_table: decode_any_text(&row, 2)?,
                source_column: decode_any_text(&row, 3)?,
                target_schema: decode_any_text(&row, 4)?,
                target_table: decode_any_text(&row, 5)?,
                target_column: decode_any_text(&row, 6)?,
                on_update: decode_any_text(&row, 7)?,
                on_delete: decode_any_text(&row, 8)?,
                position: row.try_get(9)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(group_foreign_key_rows(fk_rows))
}

async fn mysql_relationships_typed(pool: &MySqlPool) -> Result<Vec<Relationship>> {
    let rows = sqlx::query(
        r#"
        SELECT
            kcu.CONSTRAINT_NAME,
            kcu.TABLE_SCHEMA,
            kcu.TABLE_NAME,
            kcu.COLUMN_NAME,
            kcu.REFERENCED_TABLE_SCHEMA,
            kcu.REFERENCED_TABLE_NAME,
            kcu.REFERENCED_COLUMN_NAME,
            COALESCE(rc.UPDATE_RULE, ''),
            COALESCE(rc.DELETE_RULE, ''),
            kcu.ORDINAL_POSITION
        FROM information_schema.KEY_COLUMN_USAGE kcu
        JOIN information_schema.REFERENTIAL_CONSTRAINTS rc
          ON rc.CONSTRAINT_SCHEMA = kcu.CONSTRAINT_SCHEMA
         AND rc.CONSTRAINT_NAME = kcu.CONSTRAINT_NAME
        WHERE kcu.REFERENCED_TABLE_NAME IS NOT NULL
          AND kcu.TABLE_SCHEMA NOT IN ('information_schema','performance_schema','mysql')
        ORDER BY kcu.TABLE_SCHEMA, kcu.TABLE_NAME, kcu.CONSTRAINT_NAME, kcu.ORDINAL_POSITION
        "#,
    )
    .fetch_all(pool)
    .await?;

    let fk_rows = rows
        .into_iter()
        .map(|row| {
            Ok(ForeignKeyRow {
                constraint_name: decode_mysql_text(&row, 0)?,
                source_schema: decode_mysql_text(&row, 1)?,
                source_table: decode_mysql_text(&row, 2)?,
                source_column: decode_mysql_text(&row, 3)?,
                target_schema: decode_mysql_text(&row, 4)?,
                target_table: decode_mysql_text(&row, 5)?,
                target_column: decode_mysql_text(&row, 6)?,
                on_update: decode_mysql_text(&row, 7)?,
                on_delete: decode_mysql_text(&row, 8)?,
                position: row.try_get(9)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(group_foreign_key_rows(fk_rows))
}

async fn postgres_relationships(pool: &AnyPool) -> Result<Vec<Relationship>> {
    let rows = sqlx::query(
        r#"
        SELECT
            tc.constraint_name::text,
            kcu.table_schema::text,
            kcu.table_name::text,
            kcu.column_name::text,
            target_kcu.table_schema::text,
            target_kcu.table_name::text,
            target_kcu.column_name::text,
            COALESCE(rc.update_rule::text, ''),
            COALESCE(rc.delete_rule::text, ''),
            kcu.ordinal_position
        FROM information_schema.table_constraints tc
        JOIN information_schema.key_column_usage kcu
          ON tc.constraint_name = kcu.constraint_name
         AND tc.constraint_schema = kcu.constraint_schema
        JOIN information_schema.referential_constraints rc
          ON rc.constraint_name = tc.constraint_name
         AND rc.constraint_schema = tc.constraint_schema
        JOIN information_schema.key_column_usage target_kcu
          ON target_kcu.constraint_name = rc.unique_constraint_name
         AND target_kcu.constraint_schema = rc.unique_constraint_schema
         AND target_kcu.ordinal_position = kcu.position_in_unique_constraint
        WHERE tc.constraint_type = 'FOREIGN KEY'
          AND kcu.table_schema NOT IN ('pg_catalog', 'information_schema')
          AND kcu.table_schema NOT LIKE 'pg_%'
        ORDER BY kcu.table_schema, kcu.table_name, tc.constraint_name, kcu.ordinal_position
        "#,
    )
    .fetch_all(pool)
    .await?;

    let fk_rows = rows
        .into_iter()
        .map(|row| {
            Ok(ForeignKeyRow {
                constraint_name: decode_any_text(&row, 0)?,
                source_schema: decode_any_text(&row, 1)?,
                source_table: decode_any_text(&row, 2)?,
                source_column: decode_any_text(&row, 3)?,
                target_schema: decode_any_text(&row, 4)?,
                target_table: decode_any_text(&row, 5)?,
                target_column: decode_any_text(&row, 6)?,
                on_update: decode_any_text(&row, 7)?,
                on_delete: decode_any_text(&row, 8)?,
                position: row.try_get(9)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(group_foreign_key_rows(fk_rows))
}

async fn sqlite_relationships(pool: &AnyPool, table_names: &[String]) -> Result<Vec<Relationship>> {
    let mut fk_rows = Vec::new();

    for table_name in table_names {
        let rows = sqlx::query(&format!(
            "PRAGMA foreign_key_list({})",
            quote_sqlite_pragma(table_name)
        ))
        .fetch_all(pool)
        .await?;

        for row in rows {
            let fk_id: i64 = row.try_get("id")?;
            let sequence: i64 = row.try_get("seq")?;
            fk_rows.push(ForeignKeyRow {
                constraint_name: format!("{table_name}_fk_{fk_id}"),
                source_schema: String::new(),
                source_table: table_name.clone(),
                source_column: decode_any_text(&row, "from")?,
                target_schema: String::new(),
                target_table: decode_any_text(&row, "table")?,
                target_column: decode_any_text(&row, "to")?,
                on_update: decode_any_text(&row, "on_update")?,
                on_delete: decode_any_text(&row, "on_delete")?,
                position: sequence,
            });
        }
    }

    Ok(group_foreign_key_rows(fk_rows))
}

fn group_foreign_key_rows(rows: Vec<ForeignKeyRow>) -> Vec<Relationship> {
    let mut grouped = BTreeMap::<ForeignKeyGroupKey, Vec<(i64, RelationshipColumnPair)>>::new();

    for row in rows {
        let key = ForeignKeyGroupKey {
            constraint_name: row.constraint_name,
            source_schema: row.source_schema,
            source_table: row.source_table,
            target_schema: row.target_schema,
            target_table: row.target_table,
            on_update: row.on_update,
            on_delete: row.on_delete,
        };
        grouped.entry(key).or_default().push((
            row.position,
            RelationshipColumnPair {
                source_column: row.source_column,
                target_column: row.target_column,
            },
        ));
    }

    grouped
        .into_iter()
        .map(|(key, mut pairs)| {
            pairs.sort_by_key(|(position, _)| *position);
            Relationship {
                constraint_name: key.constraint_name,
                source_table: RelationshipTableRef {
                    schema_name: key.source_schema,
                    table_name: key.source_table,
                },
                target_table: RelationshipTableRef {
                    schema_name: key.target_schema,
                    table_name: key.target_table,
                },
                column_pairs: pairs.into_iter().map(|(_, pair)| pair).collect(),
                on_update: key.on_update,
                on_delete: key.on_delete,
            }
        })
        .collect()
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
                name: decode_any_text(&row, "name")?,
                column_type: decode_any_text(&row, "type")?,
                nullable: not_null == 0,
                default: decode_optional_any_text(&row, "dflt_value")?.unwrap_or_default(),
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
        let name = decode_any_text(&row, 0)?;
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
        .map(|row| decode_any_text(&row, 0))
        .collect()
}

async fn string_column_mysql(pool: &MySqlPool, sql: &str, args: &[&str]) -> Result<Vec<String>> {
    let mut query = sqlx::query(sql);
    for arg in args {
        query = query.bind(*arg);
    }
    let rows = query.fetch_all(pool).await?;
    rows.into_iter()
        .map(|row| decode_mysql_text(&row, 0))
        .collect()
}

fn decode_any_text<I>(row: &AnyRow, index: I) -> Result<String>
where
    I: ColumnIndex<AnyRow> + Copy,
{
    if let Ok(value) = row.try_get::<String, _>(index) {
        return Ok(value);
    }
    if let Ok(value) = row.try_get::<Vec<u8>, _>(index) {
        return Ok(String::from_utf8_lossy(&value).into_owned());
    }
    if let Ok(value) = row.try_get::<i64, _>(index) {
        return Ok(value.to_string());
    }
    if let Ok(value) = row.try_get::<f64, _>(index) {
        return Ok(value.to_string());
    }
    if let Ok(value) = row.try_get::<bool, _>(index) {
        return Ok(if value { "1" } else { "0" }.to_string());
    }
    if let Ok(value) = row.try_get::<Option<String>, _>(index) {
        return Ok(value.unwrap_or_default());
    }
    if let Ok(value) = row.try_get::<Option<Vec<u8>>, _>(index) {
        return Ok(value
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default());
    }

    Err(anyhow!("failed to decode text value"))
}

fn decode_mysql_text<I>(row: &MySqlRow, index: I) -> Result<String>
where
    I: ColumnIndex<MySqlRow> + Copy,
{
    if let Ok(value) = row.try_get::<String, _>(index) {
        return Ok(value);
    }
    if let Ok(value) = row.try_get::<Vec<u8>, _>(index) {
        return Ok(String::from_utf8_lossy(&value).into_owned());
    }
    if let Ok(value) = row.try_get::<i64, _>(index) {
        return Ok(value.to_string());
    }
    if let Ok(value) = row.try_get::<f64, _>(index) {
        return Ok(value.to_string());
    }
    if let Ok(value) = row.try_get::<bool, _>(index) {
        return Ok(if value { "1" } else { "0" }.to_string());
    }
    if let Ok(value) = row.try_get::<Option<String>, _>(index) {
        return Ok(value.unwrap_or_default());
    }
    if let Ok(value) = row.try_get::<Option<Vec<u8>>, _>(index) {
        return Ok(value
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default());
    }

    Err(anyhow!("failed to decode text value"))
}

fn decode_any_i64<I>(row: &AnyRow, index: I) -> Result<i64>
where
    I: ColumnIndex<AnyRow> + Copy,
{
    if let Ok(value) = row.try_get::<i64, _>(index) {
        return Ok(value);
    }
    if let Ok(value) = row.try_get::<i32, _>(index) {
        return Ok(i64::from(value));
    }
    if let Some(value) = decode_optional_any_text(row, index)? {
        return parse_integral_i64(&value);
    }

    Err(anyhow!("failed to decode integer value"))
}

fn decode_mysql_i64<I>(row: &MySqlRow, index: I) -> Result<i64>
where
    I: ColumnIndex<MySqlRow> + Copy,
{
    if let Ok(value) = row.try_get::<i64, _>(index) {
        return Ok(value);
    }
    if let Ok(value) = row.try_get::<i32, _>(index) {
        return Ok(i64::from(value));
    }
    if let Some(value) = decode_optional_mysql_text(row, index)? {
        return parse_integral_i64(&value);
    }

    Err(anyhow!("failed to decode integer value"))
}

fn parse_integral_i64(value: &str) -> Result<i64> {
    let trimmed = value.trim();
    let Some((integer, fractional)) = trimmed.split_once('.') else {
        return trimmed.parse::<i64>().map_err(Into::into);
    };

    if fractional.chars().all(|ch| ch == '0') {
        return integer.parse::<i64>().map_err(Into::into);
    }

    Err(anyhow!("expected integral value, got {trimmed:?}"))
}

fn decode_optional_any_text<I>(row: &AnyRow, index: I) -> Result<Option<String>>
where
    I: ColumnIndex<AnyRow> + Copy,
{
    let raw = row.try_get_raw(index)?;
    if raw.is_null() {
        return Ok(None);
    }
    Ok(Some(decode_any_text(row, index)?))
}

fn decode_optional_mysql_text<I>(row: &MySqlRow, index: I) -> Result<Option<String>>
where
    I: ColumnIndex<MySqlRow> + Copy,
{
    if let Ok(value) = row.try_get::<Option<String>, _>(index) {
        return Ok(value);
    }
    if let Ok(value) = row.try_get::<Option<Vec<u8>>, _>(index) {
        return Ok(value.map(|bytes| String::from_utf8_lossy(&bytes).into_owned()));
    }
    Ok(Some(decode_mysql_text(row, index)?))
}

fn quote_sqlite_pragma(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::{decode_any_i64, decode_any_text, get_schema};
    use sqlx::any::AnyPoolOptions;

    #[tokio::test]
    async fn decode_any_text_handles_sqlite_blob_values() {
        sqlx::any::install_default_drivers();

        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("create in-memory sqlite pool");

        let row = sqlx::query("SELECT x'68656c6c6f'")
            .fetch_one(&pool)
            .await
            .expect("fetch blob row");

        let decoded = decode_any_text(&row, 0).expect("decode blob text");
        assert_eq!(decoded, "hello");

        pool.close().await;
    }

    #[tokio::test]
    async fn decode_any_i64_handles_text_values() {
        sqlx::any::install_default_drivers();

        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("create in-memory sqlite pool");

        let row = sqlx::query("SELECT '4096.0000'")
            .fetch_one(&pool)
            .await
            .expect("fetch text row");

        let decoded = decode_any_i64(&row, 0).expect("decode text integer");
        assert_eq!(decoded, 4096);

        pool.close().await;
    }

    #[tokio::test]
    async fn sqlite_schema_handles_blob_result_values() {
        sqlx::any::install_default_drivers();

        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("create in-memory sqlite pool");

        sqlx::query("CREATE TABLE sample (id INTEGER PRIMARY KEY, payload BLOB)")
            .execute(&pool)
            .await
            .expect("create table");

        let row = sqlx::query("PRAGMA table_info(\"sample\")")
            .fetch_one(&pool)
            .await
            .expect("fetch pragma row");

        let name = decode_any_text(&row, "name").expect("decode pragma column name");
        assert_eq!(name, "id");

        pool.close().await;
    }

    #[tokio::test]
    async fn sqlite_schema_includes_single_column_foreign_keys() {
        sqlx::any::install_default_drivers();

        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("create in-memory sqlite pool");

        sqlx::query(
            r#"
            CREATE TABLE parents (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL
            )
            "#,
        )
        .execute(&pool)
        .await
        .expect("create parents table");

        sqlx::query(
            r#"
            CREATE TABLE children (
                id INTEGER PRIMARY KEY,
                parent_id INTEGER NOT NULL,
                FOREIGN KEY(parent_id) REFERENCES parents(id)
            )
            "#,
        )
        .execute(&pool)
        .await
        .expect("create children table");

        let schema = get_schema(&pool, "sqlite").await.expect("load schema");

        assert_eq!(schema.relationships.len(), 1);
        let relationship = &schema.relationships[0];
        assert_eq!(relationship.constraint_name, "children_fk_0");
        assert_eq!(relationship.source_table.schema_name, "");
        assert_eq!(relationship.source_table.table_name, "children");
        assert_eq!(relationship.target_table.schema_name, "");
        assert_eq!(relationship.target_table.table_name, "parents");
        assert_eq!(relationship.column_pairs.len(), 1);
        assert_eq!(relationship.column_pairs[0].source_column, "parent_id");
        assert_eq!(relationship.column_pairs[0].target_column, "id");

        pool.close().await;
    }

    #[tokio::test]
    async fn sqlite_schema_groups_composite_foreign_keys_into_one_relationship() {
        sqlx::any::install_default_drivers();

        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("create in-memory sqlite pool");

        sqlx::query(
            r#"
            CREATE TABLE parents (
                first_id INTEGER NOT NULL,
                second_id INTEGER NOT NULL,
                PRIMARY KEY(first_id, second_id)
            )
            "#,
        )
        .execute(&pool)
        .await
        .expect("create parents table");

        sqlx::query(
            r#"
            CREATE TABLE children (
                id INTEGER PRIMARY KEY,
                parent_first_id INTEGER NOT NULL,
                parent_second_id INTEGER NOT NULL,
                FOREIGN KEY(parent_first_id, parent_second_id)
                    REFERENCES parents(first_id, second_id)
                    ON DELETE CASCADE
                    ON UPDATE RESTRICT
            )
            "#,
        )
        .execute(&pool)
        .await
        .expect("create children table");

        let schema = get_schema(&pool, "sqlite").await.expect("load schema");

        assert_eq!(schema.relationships.len(), 1);
        let relationship = &schema.relationships[0];
        assert_eq!(relationship.column_pairs.len(), 2);
        assert_eq!(
            relationship.column_pairs[0].source_column,
            "parent_first_id"
        );
        assert_eq!(relationship.column_pairs[0].target_column, "first_id");
        assert_eq!(
            relationship.column_pairs[1].source_column,
            "parent_second_id"
        );
        assert_eq!(relationship.column_pairs[1].target_column, "second_id");
        assert_eq!(relationship.on_delete, "CASCADE");
        assert_eq!(relationship.on_update, "RESTRICT");

        pool.close().await;
    }

    #[tokio::test]
    async fn sqlite_schema_includes_self_referential_foreign_keys() {
        sqlx::any::install_default_drivers();

        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("create in-memory sqlite pool");

        sqlx::query(
            r#"
            CREATE TABLE employees (
                id INTEGER PRIMARY KEY,
                manager_id INTEGER,
                FOREIGN KEY(manager_id) REFERENCES employees(id)
            )
            "#,
        )
        .execute(&pool)
        .await
        .expect("create employees table");

        let schema = get_schema(&pool, "sqlite").await.expect("load schema");

        assert_eq!(schema.relationships.len(), 1);
        let relationship = &schema.relationships[0];
        assert_eq!(relationship.source_table.table_name, "employees");
        assert_eq!(relationship.target_table.table_name, "employees");
        assert_eq!(relationship.column_pairs.len(), 1);
        assert_eq!(relationship.column_pairs[0].source_column, "manager_id");
        assert_eq!(relationship.column_pairs[0].target_column, "id");

        pool.close().await;
    }

    #[tokio::test]
    async fn sqlite_schema_has_no_relationships_when_none_exist() {
        sqlx::any::install_default_drivers();

        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("create in-memory sqlite pool");

        sqlx::query(
            r#"
            CREATE TABLE notes (
                id INTEGER PRIMARY KEY,
                title TEXT NOT NULL
            )
            "#,
        )
        .execute(&pool)
        .await
        .expect("create notes table");

        let schema = get_schema(&pool, "sqlite").await.expect("load schema");

        assert!(schema.relationships.is_empty());

        pool.close().await;
    }
}
