package connections

import (
	"database/sql"
	"fmt"
	"net"
	"net/url"
	"os/exec"
	"sync"
	"time"

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

	// Kubernetes port-forwarding
	UseKubePortForward bool   `json:"useKubePortForward"`
	KubeContext        string `json:"kubeContext"`
	KubeNamespace      string `json:"kubeNamespace"`
	KubeResource       string `json:"kubeResource"` // e.g. "service/postgres"
	KubeLocalPort      int    `json:"kubeLocalPort"`
	KubeRemotePort     int    `json:"kubeRemotePort"`
}

// Manager manages active database connections.
type Manager struct {
	mu      sync.RWMutex
	conns   map[string]*sql.DB
	configs map[string]ConnectionConfig
	pfCmds  map[string]*exec.Cmd // running kubectl port-forward processes
}

// NewManager creates a new connection manager.
func NewManager() *Manager {
	return &Manager{
		conns:   make(map[string]*sql.DB),
		configs: make(map[string]ConnectionConfig),
		pfCmds:  make(map[string]*exec.Cmd),
	}
}

// buildDSN constructs a DSN from the config fields if DSN is not provided directly.
func buildDSN(cfg ConnectionConfig) (string, string) {
	if cfg.DSN != "" {
		return cfg.Driver, cfg.DSN
	}
	switch cfg.Driver {
	case "mysql":
		dsn := fmt.Sprintf("%s:%s@tcp(%s:%d)/%s?parseTime=true&multiStatements=true&tls=preferred",
			cfg.Username, cfg.Password, cfg.Host, cfg.Port, cfg.Database)
		return "mysql", dsn
	case "postgres":
		u := &url.URL{
			Scheme:   "postgres",
			User:     url.UserPassword(cfg.Username, cfg.Password),
			Host:     fmt.Sprintf("%s:%d", cfg.Host, cfg.Port),
			Path:     "/" + cfg.Database,
			RawQuery: "sslmode=prefer",
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

// startPortForward launches a kubectl port-forward process and waits for the
// local port to become available. Returns the running Cmd on success.
func startPortForward(cfg ConnectionConfig) (*exec.Cmd, error) {
	var args []string
	if cfg.KubeContext != "" {
		args = append(args, "--context="+cfg.KubeContext)
	}
	args = append(args, "port-forward")
	if cfg.KubeNamespace != "" {
		args = append(args, "-n", cfg.KubeNamespace)
	}
	args = append(args, cfg.KubeResource,
		fmt.Sprintf("%d:%d", cfg.KubeLocalPort, cfg.KubeRemotePort))

	cmd := exec.Command("kubectl", args...) // #nosec G204 -- args are user-supplied connection config values
	if err := cmd.Start(); err != nil {
		return nil, fmt.Errorf("kubectl port-forward: %w", err)
	}

	if err := waitForLocalPort(cfg.KubeLocalPort, 30*time.Second); err != nil {
		cmd.Process.Kill()
		_ = cmd.Wait()
		return nil, fmt.Errorf("port-forward not ready: %w", err)
	}
	return cmd, nil
}

// waitForLocalPort polls until a TCP listener is available on the given port or
// the timeout elapses.
func waitForLocalPort(port int, timeout time.Duration) error {
	deadline := time.Now().Add(timeout)
	addr := fmt.Sprintf("127.0.0.1:%d", port)
	for time.Now().Before(deadline) {
		c, err := net.DialTimeout("tcp", addr, time.Second)
		if err == nil {
			c.Close()
			return nil
		}
		time.Sleep(300 * time.Millisecond)
	}
	return fmt.Errorf("port %d not ready after %s", port, timeout)
}

// Connect opens and registers a connection.
func (m *Manager) Connect(cfg ConnectionConfig) error {
	var pfCmd *exec.Cmd
	effectiveCfg := cfg

	if cfg.UseKubePortForward {
		cmd, err := startPortForward(cfg)
		if err != nil {
			return err
		}
		pfCmd = cmd
		effectiveCfg.Host = "127.0.0.1"
		effectiveCfg.Port = cfg.KubeLocalPort
	}

	driver, dsn := buildDSN(effectiveCfg)

	db, err := sql.Open(driver, dsn)
	if err != nil {
		if pfCmd != nil {
			pfCmd.Process.Kill()
			_ = pfCmd.Wait()
		}
		return fmt.Errorf("open: %w", err)
	}
	if err := db.Ping(); err != nil {
		db.Close()
		if pfCmd != nil {
			pfCmd.Process.Kill()
			_ = pfCmd.Wait()
		}
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
	// Kill existing port-forward for same ID if present
	if oldCmd, ok := m.pfCmds[cfg.ID]; ok {
		oldCmd.Process.Kill()
		_ = oldCmd.Wait()
	}
	m.conns[cfg.ID] = db
	m.configs[cfg.ID] = cfg
	if pfCmd != nil {
		m.pfCmds[cfg.ID] = pfCmd
	} else {
		delete(m.pfCmds, cfg.ID)
	}
	return nil
}

// TestConnection opens a connection to test it without storing it.
func (m *Manager) TestConnection(cfg ConnectionConfig) error {
	effectiveCfg := cfg

	if cfg.UseKubePortForward {
		cmd, err := startPortForward(cfg)
		if err != nil {
			return err
		}
		defer func() {
			cmd.Process.Kill()
			_ = cmd.Wait()
		}()
		effectiveCfg.Host = "127.0.0.1"
		effectiveCfg.Port = cfg.KubeLocalPort
	}

	driver, dsn := buildDSN(effectiveCfg)
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
	if cmd, ok := m.pfCmds[id]; ok {
		cmd.Process.Kill()
		_ = cmd.Wait()
		delete(m.pfCmds, id)
	}
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
	for id, cmd := range m.pfCmds {
		cmd.Process.Kill()
		_ = cmd.Wait()
		delete(m.pfCmds, id)
	}
}
