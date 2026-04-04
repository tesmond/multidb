package schema_test

import (
	"context"
	"database/sql"
	"testing"

	"multidb/backend/schema"

	_ "modernc.org/sqlite"
)

func openSQLiteWithSchema(t *testing.T) *sql.DB {
	t.Helper()
	db, err := sql.Open("sqlite", ":memory:")
	if err != nil {
		t.Fatalf("open sqlite: %v", err)
	}
	_, err = db.Exec(`
		CREATE TABLE users (
			id   INTEGER PRIMARY KEY,
			name TEXT    NOT NULL,
			age  INTEGER
		);
		CREATE TABLE orders (
			id      INTEGER PRIMARY KEY,
			user_id INTEGER NOT NULL,
			total   REAL
		);
		CREATE VIEW active_users AS SELECT * FROM users WHERE age > 18;
		CREATE INDEX idx_users_name ON users(name);
	`)
	if err != nil {
		t.Fatalf("schema setup: %v", err)
	}
	return db
}

func TestGetSchema_SQLite(t *testing.T) {
	db := openSQLiteWithSchema(t)
	defer db.Close()

	ins := schema.NewInspector()
	tree, err := ins.GetSchema(context.Background(), db, "sqlite")
	if err != nil {
		t.Fatalf("GetSchema: %v", err)
	}

	if len(tree.Tables) != 2 {
		t.Fatalf("expected 2 tables, got %d", len(tree.Tables))
	}
	if len(tree.Views) != 1 {
		t.Fatalf("expected 1 view, got %d", len(tree.Views))
	}
	if len(tree.Indexes) == 0 {
		t.Fatal("expected at least 1 index")
	}

	// Check users table columns
	var usersTable *schema.Table
	for i := range tree.Tables {
		if tree.Tables[i].Name == "users" {
			usersTable = &tree.Tables[i]
			break
		}
	}
	if usersTable == nil {
		t.Fatal("users table not found")
	}
	if len(usersTable.Columns) != 3 {
		t.Fatalf("expected 3 columns in users, got %d", len(usersTable.Columns))
	}

	// Verify PK
	if usersTable.Columns[0].Key != "PRI" {
		t.Errorf("expected id to be PK, got key=%q", usersTable.Columns[0].Key)
	}
}

func TestGetSchema_UnsupportedDriver(t *testing.T) {
	db, _ := sql.Open("sqlite", ":memory:")
	defer db.Close()

	ins := schema.NewInspector()
	_, err := ins.GetSchema(context.Background(), db, "oracle")
	if err == nil {
		t.Fatal("expected error for unsupported driver")
	}
}
