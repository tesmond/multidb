package service

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sync"
	"time"

	"multidb/backend/connections"
	"multidb/backend/history"
	"multidb/backend/queries"
	"multidb/backend/schema"
)

// QueryResult holds the result of executing a SQL query.
type QueryResult struct {
	Columns      []string
	ColumnTypes  []string
	Rows         [][]any
	RowsAffected int64
	Duration     time.Duration
	Error        string
}

// SchemaCacheEntry is a cached schema entry.
type SchemaCacheEntry struct {
	SchemaJSON      string
	LastRefreshedAt string
	Hash            string
}

// Service provides all backend operations to the UI layer.
type Service struct {
	connMgr   *connections.Manager
	executor  *queries.Executor
	inspector *schema.Inspector
	store     *history.Store

	ctx      context.Context
	cancelFn context.CancelFunc

	queryMu      sync.Mutex
	queryCancels map[string]context.CancelFunc
}

// New creates and initialises a new Service. It opens the history/cache
// SQLite database and reconnects all previously saved connections.
func New() (*Service, error) {
	ctx, cancel := context.WithCancel(context.Background())

	s := &Service{
		connMgr:      connections.NewManager(),
		executor:     queries.NewExecutor(),
		inspector:    schema.NewInspector(),
		ctx:          ctx,
		cancelFn:     cancel,
		queryCancels: make(map[string]context.CancelFunc),
	}

	// Open SQLite history / cache store.
	dataDir, err := os.UserConfigDir()
	if err != nil {
		dataDir = os.TempDir()
	}
	appDir := filepath.Join(dataDir, "multidb")
	if err := os.MkdirAll(appDir, 0700); err != nil {
		return nil, fmt.Errorf("create app dir: %w", err)
	}

	store, err := history.NewStore(filepath.Join(appDir, "history.db"))
	if err != nil {
		return nil, fmt.Errorf("history store: %w", err)
	}
	s.store = store

	// Reconnect previously saved connections.
	saved, err := store.ListSavedConnections(ctx)
	if err == nil {
		for _, cfg := range saved {
			_ = s.connMgr.Connect(cfg)
		}
	}

	return s, nil
}

// Close shuts down all connections and the store.
func (s *Service) Close() {
	s.cancelFn()
	s.connMgr.CloseAll()
	if s.store != nil {
		_ = s.store.Close()
	}
}

// --- Connection API -------------------------------------------------------

// SaveAndConnect persists a connection config and opens the connection.
func (s *Service) SaveAndConnect(cfg connections.ConnectionConfig) error {
	if err := s.connMgr.Connect(cfg); err != nil {
		return err
	}
	if s.store != nil {
		_ = s.store.SaveConnection(s.ctx, cfg)
	}
	return nil
}

// TestConnection tests connectivity without storing.
func (s *Service) TestConnection(cfg connections.ConnectionConfig) error {
	return s.connMgr.TestConnection(cfg)
}

// DeleteConnection closes a connection and removes it from persistence.
func (s *Service) DeleteConnection(id string) error {
	err := s.connMgr.Disconnect(id)
	if s.store != nil {
		_ = s.store.DeleteConnection(s.ctx, id)
		_ = s.store.DeleteSchema(s.ctx, id)
	}
	return err
}

// ListSavedConnections returns all persisted connection configs.
func (s *Service) ListSavedConnections() ([]connections.ConnectionConfig, error) {
	if s.store == nil {
		return nil, fmt.Errorf("store not initialised")
	}
	return s.store.ListSavedConnections(s.ctx)
}

// GetConfig returns the config for a live connection.
func (s *Service) GetConfig(id string) (connections.ConnectionConfig, bool) {
	return s.connMgr.GetConfig(id)
}

// --- Query API -----------------------------------------------------------

// ExecuteQuery runs a SQL query synchronously and returns results.
func (s *Service) ExecuteQuery(connID, queryID, query string, maxRows int) QueryResult {
	db, err := s.connMgr.Get(connID)
	if err != nil {
		return QueryResult{Error: err.Error()}
	}
	if maxRows <= 0 {
		maxRows = 10_000_000
	}

	ctx, cancel := context.WithCancel(s.ctx)
	s.queryMu.Lock()
	s.queryCancels[queryID] = cancel
	s.queryMu.Unlock()

	defer func() {
		s.queryMu.Lock()
		delete(s.queryCancels, queryID)
		s.queryMu.Unlock()
		cancel()
	}()

	start := time.Now()
	qr := s.executor.Execute(ctx, db, query, maxRows)

	if s.store != nil {
		_ = s.store.AddQueryHistory(s.ctx, history.QueryRecord{
			ConnID:      connID,
			Query:       query,
			Duration:    qr.Duration,
			ResultCount: len(qr.Rows),
			Error:       qr.Error,
		})
	}

	_ = start
	return QueryResult{
		Columns:      qr.Columns,
		ColumnTypes:  qr.ColumnTypes,
		Rows:         qr.Rows,
		RowsAffected: qr.RowsAffected,
		Duration:     time.Duration(qr.Duration) * time.Millisecond,
		Error:        qr.Error,
	}
}

// CancelQuery cancels an in-flight query.
func (s *Service) CancelQuery(queryID string) {
	s.queryMu.Lock()
	defer s.queryMu.Unlock()
	if cancel, ok := s.queryCancels[queryID]; ok {
		cancel()
	}
}

// StreamQuery executes a query and streams results via the provided callbacks.
// onMeta is called once with column names. onRows is called with each batch.
// onDone is called when the query completes or errors.
func (s *Service) StreamQuery(connID, queryID, query string, maxRows int,
	onMeta func(cols []string, colTypes []string),
	onRows func(rows [][]any, offset int),
	onDone func(total int, dur time.Duration, err string),
) {
	db, err := s.connMgr.Get(connID)
	if err != nil {
		onDone(0, 0, err.Error())
		return
	}
	if maxRows <= 0 {
		maxRows = 10_000_000
	}

	ctx, cancel := context.WithCancel(s.ctx)
	s.queryMu.Lock()
	s.queryCancels[queryID] = cancel
	s.queryMu.Unlock()

	defer func() {
		s.queryMu.Lock()
		delete(s.queryCancels, queryID)
		s.queryMu.Unlock()
		cancel()
	}()

	start := time.Now()

	dbRows, err := db.QueryContext(ctx, query)
	if err != nil {
		onDone(0, time.Since(start), err.Error())
		return
	}
	defer dbRows.Close()

	cols, err := dbRows.Columns()
	if err != nil {
		onDone(0, time.Since(start), fmt.Sprintf("columns: %v", err))
		return
	}

	colTypes := make([]string, len(cols))
	if dbColTypes, err := dbRows.ColumnTypes(); err == nil {
		for i, ct := range dbColTypes {
			colTypes[i] = ct.DatabaseTypeName()
		}
	}
	onMeta(cols, colTypes)

	const (
		firstChunk = 500
		chunkSize  = 50_000
	)

	ncols := len(cols)
	chunk := make([][]any, 0, firstChunk)
	total := 0
	firstFlushed := false

	flush := func() {
		if len(chunk) == 0 {
			return
		}
		onRows(chunk, total-len(chunk))
		limit := chunkSize
		if !firstFlushed {
			firstFlushed = true
		}
		chunk = make([][]any, 0, limit)
	}

	for dbRows.Next() && total < maxRows {
		select {
		case <-ctx.Done():
			flush()
			onDone(total, time.Since(start), "query cancelled")
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
			onDone(total, time.Since(start), fmt.Sprintf("scan: %v", err))
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
			limit = firstChunk
		}
		if len(chunk) >= limit {
			flush()
		}
	}
	flush()

	if err := dbRows.Err(); err != nil {
		onDone(total, time.Since(start), err.Error())
		return
	}

	if s.store != nil {
		_ = s.store.AddQueryHistory(s.ctx, history.QueryRecord{
			ConnID:      connID,
			Query:       query,
			Duration:    time.Since(start).Milliseconds(),
			ResultCount: total,
		})
	}
	onDone(total, time.Since(start), "")
}

// --- Schema API ----------------------------------------------------------

// GetSchema fetches the live schema from the database.
func (s *Service) GetSchema(connID string) (schema.SchemaTree, error) {
	db, err := s.connMgr.Get(connID)
	if err != nil {
		return schema.SchemaTree{}, err
	}
	cfg, ok := s.connMgr.GetConfig(connID)
	if !ok {
		return schema.SchemaTree{}, fmt.Errorf("config not found for %q", connID)
	}
	return s.inspector.GetSchema(s.ctx, db, cfg.Driver)
}

// LoadCachedSchema loads the schema from the SQLite cache.
func (s *Service) LoadCachedSchema(connID string) (*SchemaCacheEntry, error) {
	if s.store == nil {
		return nil, fmt.Errorf("store not initialised")
	}
	schemaJSON, lastRefreshedAt, hash, err := s.store.LoadSchema(s.ctx, connID)
	if err != nil {
		return nil, err
	}
	return &SchemaCacheEntry{
		SchemaJSON:      schemaJSON,
		LastRefreshedAt: lastRefreshedAt,
		Hash:            hash,
	}, nil
}

// SaveCachedSchema persists the schema to the SQLite cache.
func (s *Service) SaveCachedSchema(connID string, tree schema.SchemaTree) error {
	if s.store == nil {
		return fmt.Errorf("store not initialised")
	}
	data, err := json.Marshal(tree)
	if err != nil {
		return err
	}
	hash := fmt.Sprintf("%d", len(data))
	return s.store.SaveSchema(s.ctx, connID, string(data), hash)
}

// LoadAndCacheSchema loads from cache OR fetches live if cache is empty,
// then persists the result. Returns the schema tree.
func (s *Service) LoadAndCacheSchema(connID string) (schema.SchemaTree, error) {
	// Try cache first
	if cached, err := s.LoadCachedSchema(connID); err == nil && cached.SchemaJSON != "" {
		var tree schema.SchemaTree
		if err := json.Unmarshal([]byte(cached.SchemaJSON), &tree); err == nil {
			// Refresh in background
			go func() {
				if live, err := s.GetSchema(connID); err == nil {
					_ = s.SaveCachedSchema(connID, live)
				}
			}()
			return tree, nil
		}
	}
	// Fall back to live fetch
	tree, err := s.GetSchema(connID)
	if err != nil {
		return schema.SchemaTree{}, err
	}
	_ = s.SaveCachedSchema(connID, tree)
	return tree, nil
}

// --- History API ---------------------------------------------------------

// GetQueryHistory returns recent query history.
func (s *Service) GetQueryHistory(limit int) ([]history.QueryRecord, error) {
	if s.store == nil {
		return nil, fmt.Errorf("store not initialised")
	}
	return s.store.GetQueryHistory(s.ctx, limit)
}
