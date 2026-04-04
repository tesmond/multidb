package queries_test

import (
	"context"
	"database/sql"
	"testing"

	"multidb/backend/queries"

	_ "modernc.org/sqlite"
)

func openSQLite(t *testing.T) *sql.DB {
	t.Helper()
	db, err := sql.Open("sqlite", ":memory:")
	if err != nil {
		t.Fatalf("open sqlite: %v", err)
	}
	if _, err := db.Exec(`CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL)`); err != nil {
		t.Fatalf("create table: %v", err)
	}
	if _, err := db.Exec(`INSERT INTO users (name) VALUES ('Alice'), ('Bob'), ('Charlie')`); err != nil {
		t.Fatalf("insert: %v", err)
	}
	return db
}

func TestExecuteQuery_Select(t *testing.T) {
	db := openSQLite(t)
	defer db.Close()

	exec := queries.NewExecutor()
	result := exec.Execute(context.Background(), db, "SELECT * FROM users", 0)

	if result.Error != "" {
		t.Fatalf("unexpected error: %v", result.Error)
	}
	if len(result.Columns) != 2 {
		t.Fatalf("expected 2 columns, got %d", len(result.Columns))
	}
	if len(result.Rows) != 3 {
		t.Fatalf("expected 3 rows, got %d", len(result.Rows))
	}
}

func TestExecuteQuery_MaxRows(t *testing.T) {
	db := openSQLite(t)
	defer db.Close()

	exec := queries.NewExecutor()
	result := exec.Execute(context.Background(), db, "SELECT * FROM users", 2)

	if result.Error != "" {
		t.Fatalf("unexpected error: %v", result.Error)
	}
	if len(result.Rows) != 2 {
		t.Fatalf("expected 2 rows (limited), got %d", len(result.Rows))
	}
}

func TestExecuteQuery_Error(t *testing.T) {
	db := openSQLite(t)
	defer db.Close()

	exec := queries.NewExecutor()
	result := exec.Execute(context.Background(), db, "SELECT * FROM nonexistent", 0)

	if result.Error == "" {
		t.Fatal("expected an error for nonexistent table")
	}
}

func TestExecuteQuery_Cancellation(t *testing.T) {
	db := openSQLite(t)
	defer db.Close()

	exec := queries.NewExecutor()
	ctx, cancel := context.WithCancel(context.Background())
	cancel() // cancel immediately

	result := exec.Execute(ctx, db, "SELECT * FROM users", 0)
	// May succeed or be cancelled; must not panic
	_ = result
}

func TestExecuteNonQuery(t *testing.T) {
	db := openSQLite(t)
	defer db.Close()

	exec := queries.NewExecutor()
	result := exec.ExecuteNonQuery(context.Background(), db, "INSERT INTO users (name) VALUES ('Dave')")

	if result.Error != "" {
		t.Fatalf("unexpected error: %v", result.Error)
	}
	if result.RowsAffected != 1 {
		t.Fatalf("expected 1 row affected, got %d", result.RowsAffected)
	}
}
