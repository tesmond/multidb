package history

import (
	"context"
	"database/sql"
	"fmt"
	"strings"
	"time"

	"multidb/backend/connections"

	_ "modernc.org/sqlite"
)

// QueryRecord is a saved query history entry.
type QueryRecord struct {
	ID          int64  `json:"id"`
	ConnID      string `json:"connId"`
	Query       string `json:"query"`
	Duration    int64  `json:"duration"` // ms
	ResultCount int    `json:"resultCount"`
	Error       string `json:"error,omitempty"`
	CreatedAt   string `json:"createdAt"`
}

// Store persists query history and saved connections in a local SQLite DB.
type Store struct {
	db *sql.DB
}

// NewStore opens (or creates) the local history database at the given path.
func NewStore(path string) (*Store, error) {
	db, err := sql.Open("sqlite", path)
	if err != nil {
		return nil, fmt.Errorf("open store: %w", err)
	}
	if err := db.Ping(); err != nil {
		return nil, fmt.Errorf("ping store: %w", err)
	}
	s := &Store{db: db}
	if err := s.migrate(); err != nil {
		return nil, err
	}
	return s, nil
}

func (s *Store) migrate() error {
	// Create tables
	_, err := s.db.Exec(`
		CREATE TABLE IF NOT EXISTS query_history (
			id          INTEGER PRIMARY KEY AUTOINCREMENT,
			conn_id     TEXT    NOT NULL,
			query       TEXT    NOT NULL,
			duration_ms INTEGER NOT NULL DEFAULT 0,
			result_count INTEGER NOT NULL DEFAULT 0,
			error       TEXT    NOT NULL DEFAULT '',
			created_at  TEXT    NOT NULL
		);
		CREATE TABLE IF NOT EXISTS saved_connections (
			id       TEXT PRIMARY KEY,
			name     TEXT NOT NULL,
			driver   TEXT NOT NULL,
			host     TEXT NOT NULL DEFAULT '',
			port     INTEGER NOT NULL DEFAULT 0,
			username TEXT NOT NULL DEFAULT '',
			password TEXT NOT NULL DEFAULT '',
			database TEXT NOT NULL DEFAULT '',
			dsn      TEXT NOT NULL DEFAULT ''
		);
	`)
	if err != nil {
		return err
	}

	// Add result_count column if it doesn't exist (for migration from older versions)
	_, err = s.db.Exec(`
		ALTER TABLE query_history 
		ADD COLUMN result_count INTEGER NOT NULL DEFAULT 0
	`)
	// Ignore error if column already exists
	if err != nil && !strings.Contains(err.Error(), "duplicate column name") && !strings.Contains(err.Error(), "already exists") {
		return err
	}

	return nil
}

// AddQueryHistory inserts a history record.
func (s *Store) AddQueryHistory(ctx context.Context, rec QueryRecord) error {
	if rec.CreatedAt == "" {
		rec.CreatedAt = time.Now().UTC().Format(time.RFC3339)
	}
	_, err := s.db.ExecContext(ctx, `
		INSERT INTO query_history (conn_id, query, duration_ms, result_count, error, created_at)
		VALUES (?, ?, ?, ?, ?, ?)`,
		rec.ConnID, rec.Query, rec.Duration, rec.ResultCount, rec.Error, rec.CreatedAt)
	return err
}

// GetQueryHistory returns the last n history records (most recent first).
func (s *Store) GetQueryHistory(ctx context.Context, limit int) ([]QueryRecord, error) {
	if limit <= 0 {
		limit = 100
	}
	rows, err := s.db.QueryContext(ctx, `
		SELECT id, conn_id, query, duration_ms, result_count, error, created_at
		FROM query_history
		ORDER BY id DESC LIMIT ?`, limit)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var records []QueryRecord
	for rows.Next() {
		var r QueryRecord
		if err := rows.Scan(&r.ID, &r.ConnID, &r.Query, &r.Duration, &r.ResultCount, &r.Error, &r.CreatedAt); err != nil {
			return nil, err
		}
		records = append(records, r)
	}
	return records, rows.Err()
}

// GetQueryHistoryByConnID returns the last n history records for a specific connection (most recent first).
func (s *Store) GetQueryHistoryByConnID(ctx context.Context, connID string, limit int) ([]QueryRecord, error) {
	if limit <= 0 {
		limit = 100
	}
	rows, err := s.db.QueryContext(ctx, `
		SELECT id, conn_id, query, duration_ms, result_count, error, created_at
		FROM query_history
		WHERE conn_id = ?
		ORDER BY id DESC LIMIT ?`, connID, limit)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var records []QueryRecord
	for rows.Next() {
		var r QueryRecord
		if err := rows.Scan(&r.ID, &r.ConnID, &r.Query, &r.Duration, &r.ResultCount, &r.Error, &r.CreatedAt); err != nil {
			return nil, err
		}
		records = append(records, r)
	}
	return records, rows.Err()
}

// ClearQueryHistory removes all query history records.
func (s *Store) ClearQueryHistory(ctx context.Context) error {
	_, err := s.db.ExecContext(ctx, "DELETE FROM query_history")
	return err
}

// ClearQueryHistoryByConnID removes all query history records for a specific connection.
func (s *Store) ClearQueryHistoryByConnID(ctx context.Context, connID string) error {
	_, err := s.db.ExecContext(ctx, "DELETE FROM query_history WHERE conn_id = ?", connID)
	return err
}

// SaveConnection persists a connection config.
func (s *Store) SaveConnection(ctx context.Context, cfg connections.ConnectionConfig) error {
	_, err := s.db.ExecContext(ctx, `
		INSERT INTO saved_connections (id, name, driver, host, port, username, password, database, dsn)
		VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
		ON CONFLICT(id) DO UPDATE SET
			name=excluded.name, driver=excluded.driver,
			host=excluded.host, port=excluded.port,
			username=excluded.username, password=excluded.password,
			database=excluded.database, dsn=excluded.dsn`,
		cfg.ID, cfg.Name, cfg.Driver, cfg.Host, cfg.Port,
		cfg.Username, cfg.Password, cfg.Database, cfg.DSN)
	return err
}

// DeleteConnection removes a saved connection by ID.
func (s *Store) DeleteConnection(ctx context.Context, id string) error {
	_, err := s.db.ExecContext(ctx, "DELETE FROM saved_connections WHERE id = ?", id)
	return err
}

// ListSavedConnections returns all saved connection configs.
func (s *Store) ListSavedConnections(ctx context.Context) ([]connections.ConnectionConfig, error) {
	rows, err := s.db.QueryContext(ctx, `
		SELECT id, name, driver, host, port, username, password, database, dsn
		FROM saved_connections ORDER BY name`)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var cfgs []connections.ConnectionConfig
	for rows.Next() {
		var c connections.ConnectionConfig
		if err := rows.Scan(&c.ID, &c.Name, &c.Driver, &c.Host, &c.Port,
			&c.Username, &c.Password, &c.Database, &c.DSN); err != nil {
			return nil, err
		}
		cfgs = append(cfgs, c)
	}
	return cfgs, rows.Err()
}

// Close closes the underlying database.
func (s *Store) Close() error {
	return s.db.Close()
}
