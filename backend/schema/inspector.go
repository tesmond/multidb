package schema

import (
	"context"
	"database/sql"
	"fmt"
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
	Name    string   `json:"name"`
	Type    string   `json:"type"` // TABLE | VIEW
	Columns []Column `json:"columns,omitempty"`
}

// Schema represents a named schema containing tables and views (used by Postgres).
type Schema struct {
	Name    string   `json:"name"`
	Tables  []Table  `json:"tables"`
	Views   []Table  `json:"views"`
	Indexes []string `json:"indexes"`
}

// SchemaTree is the full schema metadata for a database.
type SchemaTree struct {
	Tables  []Table  `json:"tables"`
	Views   []Table  `json:"views"`
	Indexes []string `json:"indexes"`
	Schemas []Schema `json:"schemas,omitempty"`
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
			t := Table{Name: name}
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

		tree.Schemas = append(tree.Schemas, s)
	}

	return tree, nil
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
			t := Table{Name: name}
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

		tree.Schemas = append(tree.Schemas, s)
	}

	return tree, nil
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
		t := Table{Name: name}
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
