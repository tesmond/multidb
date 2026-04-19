package main

import (
	"archive/zip"
	"bytes"
	"context"
	"database/sql"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"time"

	"multidb/backend/connections"
	"multidb/backend/history"
	"multidb/backend/queries"
	"multidb/backend/schema"

	"github.com/wailsapp/wails/v2/pkg/runtime"
)

// App is the main application struct exposed to the Wails frontend.
type App struct {
	ctx       context.Context
	connMgr   *connections.Manager
	executor  *queries.Executor
	inspector *schema.Inspector
	store     *history.Store

	// cancel functions for in-flight queries keyed by a client-supplied query ID
	queryMu      sync.Mutex
	queryCancels map[string]context.CancelFunc
}

type tableBackupPayload struct {
	Driver     string           `json:"driver"`
	SchemaName string           `json:"schemaName,omitempty"`
	TableName  string           `json:"tableName"`
	CreateSQL  string           `json:"createSql"`
	IndexesSQL []string         `json:"indexesSql"`
	Columns    []string         `json:"columns"`
	Rows       []map[string]any `json:"rows"`
	CreatedAt  string           `json:"createdAt"`
}

type tableBackupArchive struct {
	Version int                `json:"version"`
	Table   tableBackupPayload `json:"table"`
}

// SchemaCacheEntry is returned by LoadSchema so the frontend receives a
// single structured value rather than a bare tuple.
type SchemaCacheEntry struct {
	SchemaJson      string `json:"schemaJson"`
	LastRefreshedAt string `json:"lastRefreshedAt"`
	Hash            string `json:"hash"`
}

// NewApp constructs the App.
func NewApp() *App {
	return &App{
		connMgr:      connections.NewManager(),
		executor:     queries.NewExecutor(),
		inspector:    schema.NewInspector(),
		queryCancels: make(map[string]context.CancelFunc),
	}
}

// startup is called when the app starts.
func (a *App) startup(ctx context.Context) {
	a.ctx = ctx

	dataDir, err := os.UserConfigDir()
	if err != nil {
		dataDir = os.TempDir()
	}
	appDir := filepath.Join(dataDir, "multidb")
	_ = os.MkdirAll(appDir, 0700)

	store, err := history.NewStore(filepath.Join(appDir, "history.db"))
	if err != nil {
		fmt.Fprintf(os.Stderr, "history store: %v\n", err)
		return
	}
	a.store = store

	savedConns, err := store.ListSavedConnections(ctx)
	if err == nil {
		for _, cfg := range savedConns {
			_ = a.connMgr.Connect(cfg)
		}
	}
}

// shutdown is called when the app is shutting down.
func (a *App) shutdown(_ context.Context) {
	a.connMgr.CloseAll()
	if a.store != nil {
		_ = a.store.Close()
	}
}

// -----------------------------------------------------------------------
// Connection API
// -----------------------------------------------------------------------

// SaveAndConnect persists a connection config and opens the connection.
func (a *App) SaveAndConnect(cfg connections.ConnectionConfig) error {
	if err := a.connMgr.Connect(cfg); err != nil {
		return err
	}
	if a.store != nil {
		_ = a.store.SaveConnection(a.ctx, cfg)
	}
	return nil
}

// TestConnection tests a connection without persisting it.
func (a *App) TestConnection(cfg connections.ConnectionConfig) error {
	return a.connMgr.TestConnection(cfg)
}

// Disconnect closes a connection and removes it from persistence.
func (a *App) Disconnect(id string) error {
	err := a.connMgr.Disconnect(id)
	if a.store != nil {
		_ = a.store.DeleteConnection(a.ctx, id)
		_ = a.store.DeleteSchema(a.ctx, id)
	}
	return err
}

// ListConnections returns all currently active connections (passwords omitted).
func (a *App) ListConnections() []connections.ConnectionConfig {
	return a.connMgr.ListConnections()
}

// ListSavedConnections returns persisted connection configs.
func (a *App) ListSavedConnections() ([]connections.ConnectionConfig, error) {
	if a.store == nil {
		return nil, fmt.Errorf("store not initialised")
	}
	return a.store.ListSavedConnections(a.ctx)
}

// -----------------------------------------------------------------------
// Query API
// -----------------------------------------------------------------------

// ExecuteResult is the return type sent to the frontend.
type ExecuteResult struct {
	Columns      []string `json:"columns"`
	ColumnTypes  []string `json:"columnTypes"` // database type names per column
	Rows         [][]any  `json:"rows"`
	RowsAffected int64    `json:"rowsAffected"`
	Duration     int64    `json:"duration"`
	Error        string   `json:"error,omitempty"`
}

// ExecuteQuery runs a SQL query on the given connection ID.
// queryID allows cancellation via CancelQuery.
func (a *App) ExecuteQuery(connID, queryID, query string, maxRows int) ExecuteResult {
	db, err := a.connMgr.Get(connID)
	if err != nil {
		return ExecuteResult{Error: err.Error()}
	}

	ctx, cancel := context.WithCancel(a.ctx)
	a.queryMu.Lock()
	a.queryCancels[queryID] = cancel
	a.queryMu.Unlock()

	defer func() {
		a.queryMu.Lock()
		delete(a.queryCancels, queryID)
		a.queryMu.Unlock()
		cancel()
	}()

	qr := a.executor.Execute(ctx, db, query, maxRows)

	if a.store != nil {
		_ = a.store.AddQueryHistory(a.ctx, history.QueryRecord{
			ConnID:      connID,
			Query:       query,
			Duration:    qr.Duration,
			ResultCount: len(qr.Rows),
			Error:       qr.Error,
		})
	}

	return ExecuteResult(qr)
}

// CancelQuery cancels an in-flight query by its queryID.
func (a *App) CancelQuery(queryID string) {
	a.queryMu.Lock()
	defer a.queryMu.Unlock()
	if cancel, ok := a.queryCancels[queryID]; ok {
		cancel()
	}
}

// -----------------------------------------------------------------------
// Streaming query event payloads
// -----------------------------------------------------------------------

type queryStreamMeta struct {
	QueryID     string   `json:"queryId"`
	Columns     []string `json:"columns"`
	ColumnTypes []string `json:"columnTypes"`
}

type queryStreamChunk struct {
	QueryID string  `json:"queryId"`
	Rows    [][]any `json:"rows"`
	Offset  int     `json:"offset"`
}

type queryStreamDone struct {
	QueryID   string `json:"queryId"`
	TotalRows int    `json:"totalRows"`
	Duration  int64  `json:"duration"`
	Error     string `json:"error,omitempty"`
}

// ExecuteQueryStreamed runs a SQL query and pushes results to the frontend via
// Wails events instead of a single large return value. This lets the UI render
// the first rows almost immediately while the rest continue loading.
//
// Events emitted (all carry a queryId field so concurrent queries don't mix):
//
//	"query:meta"  – once, immediately after column names are known
//	"query:chunk" – once per batch of rows (first batch ≈500 rows, then ≈50 000)
//	"query:done"  – once, with the final row count, duration and any error
func (a *App) ExecuteQueryStreamed(connID, queryID, query string, maxRows int) {
	db, err := a.connMgr.Get(connID)
	if err != nil {
		runtime.EventsEmit(a.ctx, "query:done", queryStreamDone{QueryID: queryID, Error: err.Error()})
		return
	}

	if maxRows <= 0 {
		maxRows = 10_000_000
	}

	ctx, cancel := context.WithCancel(a.ctx)
	a.queryMu.Lock()
	a.queryCancels[queryID] = cancel
	a.queryMu.Unlock()

	defer func() {
		a.queryMu.Lock()
		delete(a.queryCancels, queryID)
		a.queryMu.Unlock()
		cancel()
	}()

	start := time.Now()

	dbRows, err := db.QueryContext(ctx, query)
	if err != nil {
		runtime.EventsEmit(a.ctx, "query:done", queryStreamDone{
			QueryID:  queryID,
			Duration: time.Since(start).Milliseconds(),
			Error:    err.Error(),
		})
		return
	}
	defer dbRows.Close()

	cols, err := dbRows.Columns()
	if err != nil {
		runtime.EventsEmit(a.ctx, "query:done", queryStreamDone{
			QueryID:  queryID,
			Duration: time.Since(start).Milliseconds(),
			Error:    fmt.Sprintf("columns: %v", err),
		})
		return
	}

	// Collect column type names before streaming rows.
	colTypeNames := make([]string, len(cols))
	if dbColTypes, err := dbRows.ColumnTypes(); err == nil {
		for i, ct := range dbColTypes {
			colTypeNames[i] = ct.DatabaseTypeName()
		}
	}

	// Emit column names immediately – the frontend can render the header at once.
	runtime.EventsEmit(a.ctx, "query:meta", queryStreamMeta{QueryID: queryID, Columns: cols, ColumnTypes: colTypeNames})

	const (
		firstChunkSize = 500
		chunkSize      = 50_000
	)

	ncols := len(cols)
	chunk := make([][]any, 0, firstChunkSize)
	total := 0
	firstFlushed := false

	flush := func() {
		if len(chunk) == 0 {
			return
		}
		runtime.EventsEmit(a.ctx, "query:chunk", queryStreamChunk{
			QueryID: queryID,
			Rows:    chunk,
			Offset:  total - len(chunk),
		})
		cap := chunkSize
		if !firstFlushed {
			firstFlushed = true
		}
		chunk = make([][]any, 0, cap)
	}

	for dbRows.Next() && total < maxRows {
		select {
		case <-ctx.Done():
			flush()
			runtime.EventsEmit(a.ctx, "query:done", queryStreamDone{
				QueryID:   queryID,
				TotalRows: total,
				Duration:  time.Since(start).Milliseconds(),
				Error:     "query cancelled",
			})
			return
		default:
		}

		vals := make([]any, ncols)
		ptrs := make([]any, ncols)
		for i := range vals {
			ptrs[i] = &vals[i]
		}
		if err := dbRows.Scan(ptrs...); err != nil {
			flush()
			runtime.EventsEmit(a.ctx, "query:done", queryStreamDone{
				QueryID:   queryID,
				TotalRows: total,
				Duration:  time.Since(start).Milliseconds(),
				Error:     fmt.Sprintf("scan: %v", err),
			})
			return
		}

		row := make([]any, ncols)
		for i, v := range vals {
			if b, ok := v.([]byte); ok {
				row[i] = string(b)
			} else {
				row[i] = v
			}
		}
		chunk = append(chunk, row)
		total++

		limit := chunkSize
		if !firstFlushed {
			limit = firstChunkSize
		}
		if len(chunk) >= limit {
			flush()
		}
	}

	flush()

	if err := dbRows.Err(); err != nil {
		runtime.EventsEmit(a.ctx, "query:done", queryStreamDone{
			QueryID:   queryID,
			TotalRows: total,
			Duration:  time.Since(start).Milliseconds(),
			Error:     err.Error(),
		})
		return
	}

	if a.store != nil {
		_ = a.store.AddQueryHistory(a.ctx, history.QueryRecord{
			ConnID:      connID,
			Query:       query,
			Duration:    time.Since(start).Milliseconds(),
			ResultCount: total,
		})
	}

	runtime.EventsEmit(a.ctx, "query:done", queryStreamDone{
		QueryID:   queryID,
		TotalRows: total,
		Duration:  time.Since(start).Milliseconds(),
	})
}

// -----------------------------------------------------------------------
// Schema API
// -----------------------------------------------------------------------

// GetSchema returns the schema tree for a connection.
func (a *App) GetSchema(connID string) (schema.SchemaTree, error) {
	db, err := a.connMgr.Get(connID)
	if err != nil {
		return schema.SchemaTree{}, err
	}
	cfg, ok := a.connMgr.GetConfig(connID)
	if !ok {
		return schema.SchemaTree{}, fmt.Errorf("config not found for %q", connID)
	}
	return a.inspector.GetSchema(a.ctx, db, cfg.Driver)
}

// LoadSchema retrieves the cached schema for a connection.
func (a *App) LoadSchema(connID string) (*SchemaCacheEntry, error) {
	if a.store == nil {
		return nil, fmt.Errorf("store not initialised")
	}
	schemaJson, lastRefreshedAt, hash, err := a.store.LoadSchema(a.ctx, connID)
	if err != nil {
		return nil, err
	}
	return &SchemaCacheEntry{
		SchemaJson:      schemaJson,
		LastRefreshedAt: lastRefreshedAt,
		Hash:            hash,
	}, nil
}

// SaveSchema persists the schema for a connection.
func (a *App) SaveSchema(connID string, schemaJson string, hash string) error {
	if a.store == nil {
		return fmt.Errorf("store not initialised")
	}
	return a.store.SaveSchema(a.ctx, connID, schemaJson, hash)
}

// -----------------------------------------------------------------------
// Backup / Import API
// -----------------------------------------------------------------------

func (a *App) BackupTable(connID, tableName, schemaName string) error {
	db, err := a.connMgr.Get(connID)
	if err != nil {
		return err
	}
	cfg, ok := a.connMgr.GetConfig(connID)
	if !ok {
		return fmt.Errorf("config not found for %q", connID)
	}

	filename := tableName + ".zip"
	if schemaName != "" {
		filename = schemaName + "." + tableName + ".zip"
	}

	targetPath, err := runtime.SaveFileDialog(a.ctx, runtime.SaveDialogOptions{
		Title:           "Backup Table",
		DefaultFilename: filename,
		Filters: []runtime.FileFilter{
			{
				DisplayName: "Zip Archive (*.zip)",
				Pattern:     "*.zip",
			},
		},
	})
	if err != nil {
		return err
	}
	if targetPath == "" {
		return nil
	}

	payload, err := a.buildTableBackupPayload(db, cfg.Driver, tableName, schemaName)
	if err != nil {
		return err
	}

	archive := tableBackupArchive{
		Version: 1,
		Table:   payload,
	}

	data, err := json.MarshalIndent(archive, "", "  ")
	if err != nil {
		return fmt.Errorf("marshal backup: %w", err)
	}

	var buf bytes.Buffer
	zw := zip.NewWriter(&buf)
	entry, err := zw.Create("table-backup.json")
	if err != nil {
		return fmt.Errorf("create zip entry: %w", err)
	}
	if _, err := entry.Write(data); err != nil {
		_ = zw.Close()
		return fmt.Errorf("write zip entry: %w", err)
	}
	if err := zw.Close(); err != nil {
		return fmt.Errorf("close zip: %w", err)
	}

	if err := os.WriteFile(targetPath, buf.Bytes(), 0600); err != nil {
		return fmt.Errorf("write backup file: %w", err)
	}

	return nil
}

func (a *App) ImportTable(connID string) error {
	db, err := a.connMgr.Get(connID)
	if err != nil {
		return err
	}
	cfg, ok := a.connMgr.GetConfig(connID)
	if !ok {
		return fmt.Errorf("config not found for %q", connID)
	}

	sourcePath, err := runtime.OpenFileDialog(a.ctx, runtime.OpenDialogOptions{
		Title: "Import Table Backup",
		Filters: []runtime.FileFilter{
			{
				DisplayName: "Zip Archive (*.zip)",
				Pattern:     "*.zip",
			},
		},
	})
	if err != nil {
		return err
	}
	if sourcePath == "" {
		return nil
	}

	data, err := os.ReadFile(sourcePath)
	if err != nil {
		return fmt.Errorf("read backup file: %w", err)
	}

	zr, err := zip.NewReader(bytes.NewReader(data), int64(len(data)))
	if err != nil {
		return fmt.Errorf("open zip: %w", err)
	}

	var payloadData []byte
	for _, f := range zr.File {
		if f.Name != "table-backup.json" {
			continue
		}
		rc, err := f.Open()
		if err != nil {
			return fmt.Errorf("open backup entry: %w", err)
		}
		payloadData, err = ioReadAll(rc)
		_ = rc.Close()
		if err != nil {
			return fmt.Errorf("read backup entry: %w", err)
		}
		break
	}
	if len(payloadData) == 0 {
		return fmt.Errorf("backup archive does not contain table-backup.json")
	}

	var archive tableBackupArchive
	if err := json.Unmarshal(payloadData, &archive); err != nil {
		return fmt.Errorf("parse backup archive: %w", err)
	}

	if archive.Version != 1 {
		return fmt.Errorf("unsupported backup version: %d", archive.Version)
	}

	if archive.Table.TableName == "" {
		return fmt.Errorf("backup archive missing table name")
	}

	exists, err := a.tableExists(db, cfg.Driver, archive.Table.TableName, archive.Table.SchemaName)
	if err != nil {
		return err
	}
	if exists {
		if archive.Table.SchemaName != "" {
			return fmt.Errorf("table %s.%s already exists", archive.Table.SchemaName, archive.Table.TableName)
		}
		return fmt.Errorf("table %s already exists", archive.Table.TableName)
	}

	tx, err := db.BeginTx(a.ctx, nil)
	if err != nil {
		return fmt.Errorf("begin import transaction: %w", err)
	}
	defer tx.Rollback()

	if _, err := tx.ExecContext(a.ctx, archive.Table.CreateSQL); err != nil {
		return fmt.Errorf("create table: %w", err)
	}

	for _, stmt := range archive.Table.IndexesSQL {
		stmt = strings.TrimSpace(stmt)
		if stmt == "" {
			continue
		}
		if _, err := tx.ExecContext(a.ctx, stmt); err != nil {
			return fmt.Errorf("create index: %w", err)
		}
	}

	if len(archive.Table.Rows) > 0 {
		insertSQL, args, err := a.buildInsertStatements(cfg.Driver, archive.Table.TableName, archive.Table.SchemaName, archive.Table.Columns, archive.Table.Rows)
		if err != nil {
			return err
		}
		for i, stmt := range insertSQL {
			if _, err := tx.ExecContext(a.ctx, stmt, args[i]...); err != nil {
				return fmt.Errorf("insert row %d: %w", i+1, err)
			}
		}
	}

	if err := tx.Commit(); err != nil {
		return fmt.Errorf("commit import transaction: %w", err)
	}

	return nil
}

func (a *App) buildTableBackupPayload(db *sql.DB, driver, tableName, schemaName string) (tableBackupPayload, error) {
	createSQL, err := a.getCreateTableSQL(db, driver, tableName, schemaName)
	if err != nil {
		return tableBackupPayload{}, err
	}

	indexesSQL, err := a.getCreateIndexesSQL(db, driver, tableName, schemaName)
	if err != nil {
		return tableBackupPayload{}, err
	}

	columns, rows, err := a.getTableData(db, driver, tableName, schemaName)
	if err != nil {
		return tableBackupPayload{}, err
	}

	return tableBackupPayload{
		Driver:     driver,
		SchemaName: schemaName,
		TableName:  tableName,
		CreateSQL:  createSQL,
		IndexesSQL: indexesSQL,
		Columns:    columns,
		Rows:       rows,
		CreatedAt:  time.Now().UTC().Format(time.RFC3339),
	}, nil
}

func (a *App) getCreateTableSQL(db *sql.DB, driver, tableName, schemaName string) (string, error) {
	switch driver {
	case "sqlite":
		row := db.QueryRowContext(a.ctx, `
			SELECT sql
			FROM sqlite_master
			WHERE type = 'table' AND name = ?`, tableName)
		var sqlText string
		if err := row.Scan(&sqlText); err != nil {
			return "", fmt.Errorf("load sqlite create table sql: %w", err)
		}
		return strings.TrimSpace(sqlText), nil
	case "postgres":
		qualified := a.qualifiedTableName(driver, tableName, schemaName)
		row := db.QueryRowContext(a.ctx, `
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
			GROUP BY c.oid`, qualified, tableName, schemaName)
		var sqlText string
		if err := row.Scan(&sqlText); err != nil {
			return "", fmt.Errorf("load postgres create table sql: %w", err)
		}
		return strings.TrimSpace(sqlText), nil
	case "mysql":
		qualified := a.qualifiedTableName(driver, tableName, schemaName)
		query := "SHOW CREATE TABLE " + qualified
		row := db.QueryRowContext(a.ctx, query)
		var name string
		var sqlText string
		if err := row.Scan(&name, &sqlText); err != nil {
			return "", fmt.Errorf("load mysql create table sql: %w", err)
		}
		return strings.TrimSpace(sqlText), nil
	default:
		return "", fmt.Errorf("backup not supported for driver: %s", driver)
	}
}

func (a *App) getCreateIndexesSQL(db *sql.DB, driver, tableName, schemaName string) ([]string, error) {
	switch driver {
	case "sqlite":
		rows, err := db.QueryContext(a.ctx, `
			SELECT sql
			FROM sqlite_master
			WHERE type = 'index'
			  AND tbl_name = ?
			  AND sql IS NOT NULL
			ORDER BY name`, tableName)
		if err != nil {
			return nil, fmt.Errorf("load sqlite indexes: %w", err)
		}
		defer rows.Close()
		var out []string
		for rows.Next() {
			var stmt string
			if err := rows.Scan(&stmt); err != nil {
				return nil, err
			}
			out = append(out, strings.TrimSpace(stmt))
		}
		return out, rows.Err()
	case "postgres":
		rows, err := db.QueryContext(a.ctx, `
			SELECT indexdef
			FROM pg_indexes
			WHERE schemaname = $1
			  AND tablename = $2
			ORDER BY indexname`, schemaName, tableName)
		if err != nil {
			return nil, fmt.Errorf("load postgres indexes: %w", err)
		}
		defer rows.Close()
		var out []string
		for rows.Next() {
			var stmt string
			if err := rows.Scan(&stmt); err != nil {
				return nil, err
			}
			out = append(out, strings.TrimSpace(stmt))
		}
		return out, rows.Err()
	case "mysql":
		rows, err := db.QueryContext(a.ctx, `
			SELECT INDEX_NAME,
			       NON_UNIQUE,
			       GROUP_CONCAT(COLUMN_NAME ORDER BY SEQ_IN_INDEX SEPARATOR ',')
			FROM information_schema.STATISTICS
			WHERE TABLE_SCHEMA = DATABASE()
			  AND TABLE_NAME = ?
			  AND INDEX_NAME <> 'PRIMARY'
			GROUP BY INDEX_NAME, NON_UNIQUE
			ORDER BY INDEX_NAME`, tableName)
		if err != nil {
			return nil, fmt.Errorf("load mysql indexes: %w", err)
		}
		defer rows.Close()
		var out []string
		for rows.Next() {
			var name string
			var nonUnique int
			var cols string
			if err := rows.Scan(&name, &nonUnique, &cols); err != nil {
				return nil, err
			}
			prefix := "CREATE "
			if nonUnique == 0 {
				prefix += "UNIQUE "
			}
			stmt := fmt.Sprintf("%sINDEX %s ON %s (%s)", prefix, a.quoteIdentifier(driver, name), a.qualifiedTableName(driver, tableName, schemaName), a.joinQuotedColumns(driver, strings.Split(cols, ",")))
			out = append(out, stmt)
		}
		return out, rows.Err()
	default:
		return nil, fmt.Errorf("backup not supported for driver: %s", driver)
	}
}

func (a *App) getTableData(db *sql.DB, driver, tableName, schemaName string) ([]string, []map[string]any, error) {
	query := "SELECT * FROM " + a.qualifiedTableName(driver, tableName, schemaName)
	rows, err := db.QueryContext(a.ctx, query)
	if err != nil {
		return nil, nil, fmt.Errorf("query table data: %w", err)
	}
	defer rows.Close()

	cols, err := rows.Columns()
	if err != nil {
		return nil, nil, fmt.Errorf("load columns: %w", err)
	}

	var out []map[string]any
	for rows.Next() {
		values := make([]any, len(cols))
		ptrs := make([]any, len(cols))
		for i := range values {
			ptrs[i] = &values[i]
		}
		if err := rows.Scan(ptrs...); err != nil {
			return nil, nil, fmt.Errorf("scan row: %w", err)
		}
		rowMap := make(map[string]any, len(cols))
		for i, col := range cols {
			rowMap[col] = normalizeBackupValue(values[i])
		}
		out = append(out, rowMap)
	}

	return cols, out, rows.Err()
}

func (a *App) tableExists(db *sql.DB, driver, tableName, schemaName string) (bool, error) {
	switch driver {
	case "sqlite":
		row := db.QueryRowContext(a.ctx, `
			SELECT COUNT(*)
			FROM sqlite_master
			WHERE type = 'table' AND name = ?`, tableName)
		var count int
		if err := row.Scan(&count); err != nil {
			return false, fmt.Errorf("check sqlite table exists: %w", err)
		}
		return count > 0, nil
	case "postgres":
		row := db.QueryRowContext(a.ctx, `
			SELECT COUNT(*)
			FROM information_schema.tables
			WHERE table_schema = $1 AND table_name = $2`, schemaName, tableName)
		var count int
		if err := row.Scan(&count); err != nil {
			return false, fmt.Errorf("check postgres table exists: %w", err)
		}
		return count > 0, nil
	case "mysql":
		row := db.QueryRowContext(a.ctx, `
			SELECT COUNT(*)
			FROM information_schema.tables
			WHERE table_schema = DATABASE() AND table_name = ?`, tableName)
		var count int
		if err := row.Scan(&count); err != nil {
			return false, fmt.Errorf("check mysql table exists: %w", err)
		}
		return count > 0, nil
	default:
		return false, fmt.Errorf("import not supported for driver: %s", driver)
	}
}

func (a *App) buildInsertStatements(driver, tableName, schemaName string, columns []string, rows []map[string]any) ([]string, [][]any, error) {
	if len(columns) == 0 {
		return nil, nil, nil
	}

	qualified := a.qualifiedTableName(driver, tableName, schemaName)
	columnList := a.joinQuotedColumns(driver, columns)

	var statements []string
	var args [][]any

	for _, row := range rows {
		placeholders := make([]string, len(columns))
		values := make([]any, len(columns))
		for i, col := range columns {
			placeholders[i] = a.placeholder(driver, i+1)
			values[i] = row[col]
		}
		stmt := fmt.Sprintf("INSERT INTO %s (%s) VALUES (%s)", qualified, columnList, strings.Join(placeholders, ", "))
		statements = append(statements, stmt)
		args = append(args, values)
	}

	return statements, args, nil
}

func (a *App) qualifiedTableName(driver, tableName, schemaName string) string {
	if schemaName == "" {
		return a.quoteIdentifier(driver, tableName)
	}
	return a.quoteIdentifier(driver, schemaName) + "." + a.quoteIdentifier(driver, tableName)
}

func (a *App) quoteIdentifier(driver, name string) string {
	if driver == "mysql" {
		return "`" + strings.ReplaceAll(name, "`", "``") + "`"
	}
	return `"` + strings.ReplaceAll(name, `"`, `""`) + `"`
}

func (a *App) joinQuotedColumns(driver string, columns []string) string {
	quoted := make([]string, 0, len(columns))
	for _, col := range columns {
		quoted = append(quoted, a.quoteIdentifier(driver, strings.TrimSpace(col)))
	}
	return strings.Join(quoted, ", ")
}

func (a *App) placeholder(driver string, index int) string {
	if driver == "postgres" {
		return "$" + strconv.Itoa(index)
	}
	return "?"
}

func normalizeBackupValue(v any) any {
	switch t := v.(type) {
	case []byte:
		return string(t)
	default:
		return t
	}
}

func ioReadAll(rc interface{ Read([]byte) (int, error) }) ([]byte, error) {
	buf := bytes.NewBuffer(nil)
	tmp := make([]byte, 4096)
	for {
		n, err := rc.Read(tmp)
		if n > 0 {
			buf.Write(tmp[:n])
		}
		if err != nil {
			if err.Error() == "EOF" {
				return buf.Bytes(), nil
			}
			return nil, err
		}
	}
}

// SaveCSV opens a native save-file dialog and writes the CSV content to the
// chosen path. This is required on macOS/Wails because WKWebView does not
// support blob URL downloads triggered by a simulated link click.
func (a *App) SaveCSV(csvContent string, defaultFilename string) error {
	if defaultFilename == "" {
		defaultFilename = "query_results.csv"
	}

	targetPath, err := runtime.SaveFileDialog(a.ctx, runtime.SaveDialogOptions{
		Title:           "Export CSV",
		DefaultFilename: defaultFilename,
		Filters: []runtime.FileFilter{
			{
				DisplayName: "CSV File (*.csv)",
				Pattern:     "*.csv",
			},
		},
	})
	if err != nil {
		return err
	}
	if targetPath == "" {
		// User cancelled – not an error
		return nil
	}

	if err := os.WriteFile(targetPath, []byte(csvContent), 0600); err != nil {
		return fmt.Errorf("write CSV file: %w", err)
	}
	return nil
}

// SaveFile writes the provided data to the given absolute or relative path,
// creating parent directories if necessary. It writes to a temporary file
// first and then atomically renames it into place to avoid partial writes.
func (a *App) SaveFile(path string, data []byte, perm os.FileMode) error {
	if path == "" {
		return fmt.Errorf("empty path")
	}

	// Ensure parent directory exists
	dir := filepath.Dir(path)
	if dir != "." && dir != "" {
		if err := os.MkdirAll(dir, 0o755); err != nil {
			return fmt.Errorf("create parent directories: %w", err)
		}
	}

	// Write to a temp file in the same directory then rename
	tmpPath := path + ".tmp"
	if err := os.WriteFile(tmpPath, data, perm); err != nil {
		return fmt.Errorf("write temp file: %w", err)
	}

	// Ensure rename is atomic where possible
	if err := os.Rename(tmpPath, path); err != nil {
		_ = os.Remove(tmpPath)
		return fmt.Errorf("finalise write: %w", err)
	}
	return nil
}

// -----------------------------------------------------------------------
// History API
// -----------------------------------------------------------------------

// GetQueryHistory returns recent query history records.
func (a *App) GetQueryHistory(limit int) ([]history.QueryRecord, error) {
	if a.store == nil {
		return nil, fmt.Errorf("store not initialised")
	}
	return a.store.GetQueryHistory(a.ctx, limit)
}

// GetQueryHistoryByConnID returns query history records for a specific connection.
func (a *App) GetQueryHistoryByConnID(connID string, limit int) ([]history.QueryRecord, error) {
	if a.store == nil {
		return nil, fmt.Errorf("store not initialised")
	}
	return a.store.GetQueryHistoryByConnID(a.ctx, connID, limit)
}

// ClearQueryHistory removes all history records.
func (a *App) ClearQueryHistory() error {
	if a.store == nil {
		return fmt.Errorf("store not initialised")
	}
	return a.store.ClearQueryHistory(a.ctx)
}

// ClearQueryHistoryByConnID removes all history records for a specific connection.
func (a *App) ClearQueryHistoryByConnID(connID string) error {
	if a.store == nil {
		return fmt.Errorf("store not initialised")
	}
	return a.store.ClearQueryHistoryByConnID(a.ctx, connID)
}
