package schema

import (
	"context"
	"database/sql"
	"fmt"
	"os"
)

// Column represents a column in a table.
type Column struct {
	Name     string `json:"name"`
	Type     string `json:"type"`
	Nullable bool   `json:"nullable"`
	Default  string `json:"default"`
	Key      string `json:"key"`
}

// Table represents a table or view.
type Table struct {
	Name      string   `json:"name"`
	Type      string   `json:"type"` // TABLE | VIEW
	SizeBytes int64    `json:"sizeBytes,omitempty"`
	Columns   []Column `json:"columns,omitempty"`
}

// Schema represents a named schema containing tables and views (used by Postgres).
type Schema struct {
	Name      string   `json:"name"`
	SizeBytes int64    `json:"sizeBytes,omitempty"`
	Tables    []Table  `json:"tables"`
	Views     []Table  `json:"views"`
	Indexes   []string `json:"indexes"`
}

// SchemaTree is the full schema metadata for a database.
type SchemaTree struct {
	SizeBytes int64    `json:"sizeBytes,omitempty"`
	Tables    []Table  `json:"tables"`
	Views     []Table  `json:"views"`
	Indexes   []string `json:"indexes"`
	Schemas   []Schema `json:"schemas,omitempty"`
}

// Inspector fetches schema metadata from a database.
type Inspector struct{}

// NewInspector creates a new Inspector.
func NewInspector() *Inspector {
	return &Inspector{}
}

// GetSchema fetches the full schema for the given driver.
func (i *Inspector) GetSchema(ctx context.Context, db *sql.DB, driver string) (SchemaTree, error) {
	switch driver {
	case "mysql":
		return i.mysqlSchema(ctx, db)
	case "postgres":
		return i.postgresSchema(ctx, db)
	case "sqlite":
		return i.sqliteSchema(ctx, db)
	default:
		return SchemaTree{}, fmt.Errorf("unsupported driver: %s", driver)
	}
}

func (i *Inspector) mysqlSchema(ctx context.Context, db *sql.DB) (SchemaTree, error) {
	tree := SchemaTree{Tables: []Table{}, Views: []Table{}, Indexes: []string{}}
	sizes, schemaSizes, err := i.mysqlTableSizes(ctx, db)
	if err != nil {
		return tree, err
	}

	dbRows, err := db.QueryContext(ctx, `
		SELECT SCHEMA_NAME FROM information_schema.SCHEMATA
		WHERE SCHEMA_NAME NOT IN ('information_schema','performance_schema','mysql')
		ORDER BY SCHEMA_NAME`)
	if err != nil {
		return tree, fmt.Errorf("databases query: %w", err)
	}
	defer dbRows.Close()

	var dbNames []string
	for dbRows.Next() {
		var name string
		if err := dbRows.Scan(&name); err != nil {
			return tree, err
		}
		dbNames = append(dbNames, name)
	}
	if err := dbRows.Err(); err != nil {
		return tree, err
	}

	for _, dbName := range dbNames {
		s := Schema{Name: dbName, Tables: []Table{}, Views: []Table{}, Indexes: []string{}}

		rows, err := db.QueryContext(ctx, `
			SELECT TABLE_NAME, TABLE_TYPE
			FROM information_schema.TABLES
			WHERE TABLE_SCHEMA = ?
			ORDER BY TABLE_NAME`, dbName)
		if err != nil {
			return tree, fmt.Errorf("tables query: %w", err)
		}

		for rows.Next() {
			var name, tableType string
			if err := rows.Scan(&name, &tableType); err != nil {
				rows.Close()
				return tree, err
			}
			t := Table{Name: name, SizeBytes: sizes[dbName+"."+name]}
			if tableType == "VIEW" {
				t.Type = "VIEW"
				s.Views = append(s.Views, t)
			} else {
				t.Type = "TABLE"
				s.Tables = append(s.Tables, t)
			}
		}
		rows.Close()
		if err := rows.Err(); err != nil {
			return tree, err
		}

		for idx := range s.Tables {
			cols, err := i.mysqlColumns(ctx, db, dbName, s.Tables[idx].Name)
			if err != nil {
				return tree, err
			}
			s.Tables[idx].Columns = cols
		}

		idxRows, err := db.QueryContext(ctx, `
			SELECT DISTINCT INDEX_NAME
			FROM information_schema.STATISTICS
			WHERE TABLE_SCHEMA = ?
			ORDER BY INDEX_NAME`, dbName)
		if err == nil {
			for idxRows.Next() {
				var name string
				if err := idxRows.Scan(&name); err == nil {
					s.Indexes = append(s.Indexes, name)
				}
			}
			idxRows.Close()
		}

		s.SizeBytes = schemaSizes[dbName]
		tree.SizeBytes += s.SizeBytes
		tree.Schemas = append(tree.Schemas, s)
	}

	return tree, nil
}

func (i *Inspector) mysqlTableSizes(ctx context.Context, db *sql.DB) (map[string]int64, map[string]int64, error) {
	rows, err := db.QueryContext(ctx, `
		SELECT TABLE_SCHEMA, TABLE_NAME, COALESCE(DATA_LENGTH, 0) + COALESCE(INDEX_LENGTH, 0)
		FROM information_schema.TABLES
		WHERE TABLE_SCHEMA NOT IN ('information_schema','performance_schema','mysql')
		  AND TABLE_TYPE = 'BASE TABLE'`)
	if err != nil {
		return nil, nil, fmt.Errorf("table sizes query: %w", err)
	}
	defer rows.Close()

	tableSizes := make(map[string]int64)
	schemaSizes := make(map[string]int64)
	for rows.Next() {
		var schemaName, tableName string
		var size int64
		if err := rows.Scan(&schemaName, &tableName, &size); err != nil {
			return nil, nil, err
		}
		tableSizes[schemaName+"."+tableName] = size
		schemaSizes[schemaName] += size
	}
	return tableSizes, schemaSizes, rows.Err()
}

func (i *Inspector) mysqlColumns(ctx context.Context, db *sql.DB, dbName, table string) ([]Column, error) {
	rows, err := db.QueryContext(ctx, `
		SELECT COLUMN_NAME, COLUMN_TYPE, IS_NULLABLE, IFNULL(COLUMN_DEFAULT,''), COLUMN_KEY
		FROM information_schema.COLUMNS
		WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?
		ORDER BY ORDINAL_POSITION`, dbName, table)
	if err != nil {
		return nil, fmt.Errorf("columns query: %w", err)
	}
	defer rows.Close()

	var cols []Column
	for rows.Next() {
		var col Column
		var nullable string
		if err := rows.Scan(&col.Name, &col.Type, &nullable, &col.Default, &col.Key); err != nil {
			return nil, err
		}
		col.Nullable = nullable == "YES"
		cols = append(cols, col)
	}
	return cols, rows.Err()
}

func (i *Inspector) postgresSchema(ctx context.Context, db *sql.DB) (SchemaTree, error) {
	tree := SchemaTree{Tables: []Table{}, Views: []Table{}, Indexes: []string{}}
	sizes, schemaSizes, err := i.postgresTableSizes(ctx, db)
	if err != nil {
		return tree, err
	}

	schemaRows, err := db.QueryContext(ctx, `
		SELECT schema_name FROM information_schema.schemata
		WHERE schema_name NOT IN ('pg_catalog', 'information_schema')
		  AND schema_name NOT LIKE 'pg_%'
		ORDER BY schema_name`)
	if err != nil {
		return tree, fmt.Errorf("schemas query: %w", err)
	}
	defer schemaRows.Close()

	var schemaNames []string
	for schemaRows.Next() {
		var name string
		if err := schemaRows.Scan(&name); err != nil {
			return tree, err
		}
		schemaNames = append(schemaNames, name)
	}
	if err := schemaRows.Err(); err != nil {
		return tree, err
	}

	for _, schemaName := range schemaNames {
		s := Schema{Name: schemaName, Tables: []Table{}, Views: []Table{}, Indexes: []string{}}

		rows, err := db.QueryContext(ctx, `
			SELECT table_name, table_type
			FROM information_schema.tables
			WHERE table_schema = $1
			ORDER BY table_name`, schemaName)
		if err != nil {
			return tree, fmt.Errorf("tables query: %w", err)
		}

		for rows.Next() {
			var name, tableType string
			if err := rows.Scan(&name, &tableType); err != nil {
				rows.Close()
				return tree, err
			}
			t := Table{Name: name, SizeBytes: sizes[schemaName+"."+name]}
			if tableType == "VIEW" {
				t.Type = "VIEW"
				s.Views = append(s.Views, t)
			} else {
				t.Type = "TABLE"
				s.Tables = append(s.Tables, t)
			}
		}
		rows.Close()
		if err := rows.Err(); err != nil {
			return tree, err
		}

		for idx := range s.Tables {
			cols, err := i.postgresColumns(ctx, db, schemaName, s.Tables[idx].Name)
			if err != nil {
				return tree, err
			}
			s.Tables[idx].Columns = cols
		}

		idxRows, err := db.QueryContext(ctx, `
			SELECT indexname FROM pg_indexes
			WHERE schemaname = $1
			ORDER BY indexname`, schemaName)
		if err == nil {
			for idxRows.Next() {
				var name string
				if err := idxRows.Scan(&name); err == nil {
					s.Indexes = append(s.Indexes, name)
				}
			}
			idxRows.Close()
		}

		s.SizeBytes = schemaSizes[schemaName]
		tree.SizeBytes += s.SizeBytes
		tree.Schemas = append(tree.Schemas, s)
	}

	return tree, nil
}

func (i *Inspector) postgresTableSizes(ctx context.Context, db *sql.DB) (map[string]int64, map[string]int64, error) {
	rows, err := db.QueryContext(ctx, `
		SELECT n.nspname, c.relname, pg_total_relation_size(c.oid)
		FROM pg_class c
		JOIN pg_namespace n ON n.oid = c.relnamespace
		WHERE c.relkind IN ('r', 'p')
		  AND n.nspname NOT IN ('pg_catalog', 'information_schema')
		  AND n.nspname NOT LIKE 'pg_%'`)
	if err != nil {
		return nil, nil, fmt.Errorf("table sizes query: %w", err)
	}
	defer rows.Close()

	tableSizes := make(map[string]int64)
	schemaSizes := make(map[string]int64)
	for rows.Next() {
		var schemaName, tableName string
		var size int64
		if err := rows.Scan(&schemaName, &tableName, &size); err != nil {
			return nil, nil, err
		}
		tableSizes[schemaName+"."+tableName] = size
		schemaSizes[schemaName] += size
	}
	return tableSizes, schemaSizes, rows.Err()
}

func (i *Inspector) postgresColumns(ctx context.Context, db *sql.DB, schemaName, table string) ([]Column, error) {
	rows, err := db.QueryContext(ctx, `
		SELECT column_name, data_type, is_nullable, COALESCE(column_default, '')
		FROM information_schema.columns
		WHERE table_schema = $1 AND table_name = $2
		ORDER BY ordinal_position`, schemaName, table)
	if err != nil {
		return nil, fmt.Errorf("columns query: %w", err)
	}
	defer rows.Close()

	var cols []Column
	for rows.Next() {
		var col Column
		var nullable string
		if err := rows.Scan(&col.Name, &col.Type, &nullable, &col.Default); err != nil {
			return nil, err
		}
		col.Nullable = nullable == "YES"
		cols = append(cols, col)
	}
	return cols, rows.Err()
}

func (i *Inspector) sqliteSchema(ctx context.Context, db *sql.DB) (SchemaTree, error) {
	tree := SchemaTree{Tables: []Table{}, Views: []Table{}, Indexes: []string{}}
	tableSizes := i.sqliteTableSizes(ctx, db)
	tree.SizeBytes = i.sqliteDatabaseSize(ctx, db)

	rows, err := db.QueryContext(ctx, `
		SELECT name, type FROM sqlite_master
		WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%'
		ORDER BY name`)
	if err != nil {
		return tree, fmt.Errorf("tables query: %w", err)
	}
	defer rows.Close()

	for rows.Next() {
		var name, objType string
		if err := rows.Scan(&name, &objType); err != nil {
			return tree, err
		}
		t := Table{Name: name, SizeBytes: tableSizes[name]}
		if objType == "view" {
			t.Type = "VIEW"
			tree.Views = append(tree.Views, t)
		} else {
			t.Type = "TABLE"
			tree.Tables = append(tree.Tables, t)
		}
	}
	if err := rows.Err(); err != nil {
		return tree, err
	}

	for idx := range tree.Tables {
		cols, err := i.sqliteColumns(ctx, db, tree.Tables[idx].Name)
		if err != nil {
			return tree, err
		}
		tree.Tables[idx].Columns = cols
	}

	// Fetch index names
	idxRows, err := db.QueryContext(ctx, `
		SELECT name FROM sqlite_master WHERE type = 'index' ORDER BY name`)
	if err == nil {
		defer idxRows.Close()
		for idxRows.Next() {
			var name string
			if err := idxRows.Scan(&name); err == nil {
				tree.Indexes = append(tree.Indexes, name)
			}
		}
	}

	return tree, nil
}

func (i *Inspector) sqliteTableSizes(ctx context.Context, db *sql.DB) map[string]int64 {
	rows, err := db.QueryContext(ctx, `
		SELECT COALESCE(m.tbl_name, d.name), SUM(d.pgsize)
		FROM dbstat d
		LEFT JOIN sqlite_master m
		  ON m.type = 'index' AND m.name = d.name
		WHERE d.name NOT LIKE 'sqlite_%'
		GROUP BY COALESCE(m.tbl_name, d.name)`)
	if err != nil {
		return nil
	}
	defer rows.Close()

	tableSizes := make(map[string]int64)
	for rows.Next() {
		var name string
		var size sql.NullInt64
		if err := rows.Scan(&name, &size); err == nil && size.Valid {
			tableSizes[name] = size.Int64
		}
	}
	return tableSizes
}

func (i *Inspector) sqliteDatabaseSize(ctx context.Context, db *sql.DB) int64 {
	var pageCount, pageSize int64
	if err := db.QueryRowContext(ctx, "PRAGMA page_count").Scan(&pageCount); err == nil {
		if err := db.QueryRowContext(ctx, "PRAGMA page_size").Scan(&pageSize); err == nil {
			return pageCount * pageSize
		}
	}

	var path string
	if err := db.QueryRowContext(ctx, "PRAGMA database_list").Scan(new(int), new(string), &path); err == nil && path != "" {
		if info, err := os.Stat(path); err == nil {
			return info.Size()
		}
	}
	return 0
}

func (i *Inspector) sqliteColumns(ctx context.Context, db *sql.DB, table string) ([]Column, error) {
	// SQLite doesn't support parameterized PRAGMA, table name is validated from sqlite_master
	rows, err := db.QueryContext(ctx, fmt.Sprintf("PRAGMA table_info(%q)", table))
	if err != nil {
		return nil, fmt.Errorf("pragma: %w", err)
	}
	defer rows.Close()

	var cols []Column
	for rows.Next() {
		var cid int
		var name, colType string
		var notNull int
		var dfltValue sql.NullString
		var pk int
		if err := rows.Scan(&cid, &name, &colType, &notNull, &dfltValue, &pk); err != nil {
			return nil, err
		}
		col := Column{
			Name:     name,
			Type:     colType,
			Nullable: notNull == 0,
		}
		if dfltValue.Valid {
			col.Default = dfltValue.String
		}
		if pk > 0 {
			col.Key = "PRI"
		}
		cols = append(cols, col)
	}
	return cols, rows.Err()
}

// GetPrimaryKeys returns the primary key column names for a given table.
func (i *Inspector) GetPrimaryKeys(ctx context.Context, db *sql.DB, driver, schemaName, tableName string) ([]string, error) {
	switch driver {
	case "mysql":
		return i.mysqlPrimaryKeys(ctx, db, schemaName, tableName)
	case "postgres":
		return i.postgresPrimaryKeys(ctx, db, schemaName, tableName)
	case "sqlite":
		return i.sqlitePrimaryKeys(ctx, db, tableName)
	default:
		return nil, fmt.Errorf("unsupported driver: %s", driver)
	}
}

func (i *Inspector) mysqlPrimaryKeys(ctx context.Context, db *sql.DB, dbName, tableName string) ([]string, error) {
	rows, err := db.QueryContext(ctx, `
		SELECT COLUMN_NAME FROM information_schema.COLUMNS
		WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? AND COLUMN_KEY = 'PRI'
		ORDER BY ORDINAL_POSITION`, dbName, tableName)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var pks []string
	for rows.Next() {
		var col string
		if err := rows.Scan(&col); err != nil {
			return nil, err
		}
		pks = append(pks, col)
	}
	return pks, rows.Err()
}

func (i *Inspector) postgresPrimaryKeys(ctx context.Context, db *sql.DB, schemaName, tableName string) ([]string, error) {
	if schemaName == "" {
		schemaName = "public"
	}
	rows, err := db.QueryContext(ctx, `
		SELECT kcu.column_name
		FROM information_schema.table_constraints tc
		JOIN information_schema.key_column_usage kcu
		  ON tc.constraint_name = kcu.constraint_name
		 AND tc.table_schema = kcu.table_schema
		 AND tc.table_name = kcu.table_name
		WHERE tc.constraint_type = 'PRIMARY KEY'
		  AND tc.table_schema = $1
		  AND tc.table_name = $2
		ORDER BY kcu.ordinal_position`, schemaName, tableName)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var pks []string
	for rows.Next() {
		var col string
		if err := rows.Scan(&col); err != nil {
			return nil, err
		}
		pks = append(pks, col)
	}
	return pks, rows.Err()
}

func (i *Inspector) sqlitePrimaryKeys(ctx context.Context, db *sql.DB, tableName string) ([]string, error) {
	rows, err := db.QueryContext(ctx, fmt.Sprintf("PRAGMA table_info(%q)", tableName))
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	type pkEntry struct {
		name string
		pkn  int
	}
	var entries []pkEntry
	for rows.Next() {
		var cid int
		var name, colType string
		var notNull int
		var dfltValue sql.NullString
		var pk int
		if err := rows.Scan(&cid, &name, &colType, &notNull, &dfltValue, &pk); err != nil {
			return nil, err
		}
		if pk > 0 {
			entries = append(entries, pkEntry{name, pk})
		}
	}
	if err := rows.Err(); err != nil {
		return nil, err
	}
	pks := make([]string, len(entries))
	for idx, e := range entries {
		pks[idx] = e.name
	}
	return pks, nil
}
