package connections

import (
	"database/sql"
	"fmt"
	"net/url"
	"sync"

	_ "github.com/go-sql-driver/mysql"
	_ "github.com/jackc/pgx/v5/stdlib"
	_ "modernc.org/sqlite"
)

// ConnectionConfig stores the configuration for a database connection.
type ConnectionConfig struct {
	ID       string `json:"id"`
	Name     string `json:"name"`
	Driver   string `json:"driver"` // mysql | postgres | sqlite
	Host     string `json:"host"`
	Port     int    `json:"port"`
	Username string `json:"username"`
	Password string `json:"password"`
	Database string `json:"database"`
	DSN      string `json:"dsn"` // optional override
}

// Manager manages active database connections.
type Manager struct {
	mu      sync.RWMutex
	conns   map[string]*sql.DB
	configs map[string]ConnectionConfig
}

// NewManager creates a new connection manager.
func NewManager() *Manager {
	return &Manager{
		conns:   make(map[string]*sql.DB),
		configs: make(map[string]ConnectionConfig),
	}
}

// buildDSN constructs a DSN from the config fields if DSN is not provided directly.
func buildDSN(cfg ConnectionConfig) (string, string) {
	if cfg.DSN != "" {
		return cfg.Driver, cfg.DSN
	}
	switch cfg.Driver {
	case "mysql":
		dsn := fmt.Sprintf("%s:%s@tcp(%s:%d)/%s?parseTime=true&multiStatements=true",
			cfg.Username, cfg.Password, cfg.Host, cfg.Port, cfg.Database)
		return "mysql", dsn
	case "postgres":
		u := &url.URL{
			Scheme:   "postgres",
			User:     url.UserPassword(cfg.Username, cfg.Password),
			Host:     fmt.Sprintf("%s:%d", cfg.Host, cfg.Port),
			Path:     "/" + cfg.Database,
			RawQuery: "sslmode=disable",
		}
		return "pgx", u.String()
	case "sqlite":
		path := cfg.Database
		if path == "" {
			path = cfg.Host
		}
		return "sqlite", path
	default:
		return cfg.Driver, cfg.DSN
	}
}

// Connect opens and registers a connection.
func (m *Manager) Connect(cfg ConnectionConfig) error {
	driver, dsn := buildDSN(cfg)

	db, err := sql.Open(driver, dsn)
	if err != nil {
		return fmt.Errorf("open: %w", err)
	}
	if err := db.Ping(); err != nil {
		db.Close()
		return fmt.Errorf("ping: %w", err)
	}
	db.SetMaxOpenConns(10)
	db.SetMaxIdleConns(5)

	m.mu.Lock()
	defer m.mu.Unlock()
	// Close existing connection with same ID if present
	if old, ok := m.conns[cfg.ID]; ok {
		old.Close()
	}
	m.conns[cfg.ID] = db
	m.configs[cfg.ID] = cfg
	return nil
}

// TestConnection opens a connection to test it without storing it.
func (m *Manager) TestConnection(cfg ConnectionConfig) error {
	driver, dsn := buildDSN(cfg)
	db, err := sql.Open(driver, dsn)
	if err != nil {
		return fmt.Errorf("open: %w", err)
	}
	defer db.Close()
	if err := db.Ping(); err != nil {
		return fmt.Errorf("ping: %w", err)
	}
	return nil
}

// Disconnect closes and removes a connection by ID.
func (m *Manager) Disconnect(id string) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	db, ok := m.conns[id]
	if !ok {
		return fmt.Errorf("connection %q not found", id)
	}
	err := db.Close()
	delete(m.conns, id)
	delete(m.configs, id)
	return err
}

// Get returns the *sql.DB for a connection ID.
func (m *Manager) Get(id string) (*sql.DB, error) {
	m.mu.RLock()
	defer m.mu.RUnlock()
	db, ok := m.conns[id]
	if !ok {
		return nil, fmt.Errorf("connection %q not found", id)
	}
	return db, nil
}

// GetConfig returns the config for a connection ID.
func (m *Manager) GetConfig(id string) (ConnectionConfig, bool) {
	m.mu.RLock()
	defer m.mu.RUnlock()
	cfg, ok := m.configs[id]
	return cfg, ok
}

// ListConnections returns all active connection configs.
func (m *Manager) ListConnections() []ConnectionConfig {
	m.mu.RLock()
	defer m.mu.RUnlock()
	out := make([]ConnectionConfig, 0, len(m.configs))
	for _, cfg := range m.configs {
		// Omit passwords from listing
		safe := cfg
		safe.Password = ""
		out = append(out, safe)
	}
	return out
}

// CloseAll closes all connections.
func (m *Manager) CloseAll() {
	m.mu.Lock()
	defer m.mu.Unlock()
	for id, db := range m.conns {
		db.Close()
		delete(m.conns, id)
		delete(m.configs, id)
	}
}
