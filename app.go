package main

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"sync"

	"multidb/backend/connections"
	"multidb/backend/history"
	"multidb/backend/queries"
	"multidb/backend/schema"
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
			ConnID:   connID,
			Query:    query,
			Duration: qr.Duration,
			Error:    qr.Error,
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

// ClearQueryHistory removes all history records.
func (a *App) ClearQueryHistory() error {
	if a.store == nil {
		return fmt.Errorf("store not initialised")
	}
	return a.store.ClearQueryHistory(a.ctx)
}
