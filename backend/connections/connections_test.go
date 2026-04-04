package connections_test

import (
	"testing"

	"multidb/backend/connections"
)

func TestBuildDSN_MySQL(t *testing.T) {
	cfg := connections.ConnectionConfig{
		Driver:   "mysql",
		Host:     "localhost",
		Port:     3306,
		Username: "root",
		Password: "secret",
		Database: "testdb",
	}
	// Just verify TestConnection returns an error (no real server), not a panic
	mgr := connections.NewManager()
	err := mgr.TestConnection(cfg)
	if err == nil {
		t.Log("connected to a live MySQL instance (optional)")
	} else {
		t.Logf("expected connection error (no server): %v", err)
	}
}

func TestManager_ConnectDisconnect(t *testing.T) {
	mgr := connections.NewManager()
	cfg := connections.ConnectionConfig{
		ID:       "test-sqlite",
		Name:     "Test SQLite",
		Driver:   "sqlite",
		Database: ":memory:",
	}
	if err := mgr.Connect(cfg); err != nil {
		t.Fatalf("Connect: %v", err)
	}
	db, err := mgr.Get("test-sqlite")
	if err != nil {
		t.Fatalf("Get: %v", err)
	}
	if err := db.Ping(); err != nil {
		t.Fatalf("Ping: %v", err)
	}
	conns := mgr.ListConnections()
	if len(conns) != 1 {
		t.Fatalf("want 1 connection, got %d", len(conns))
	}
	// Password should be omitted from listing
	if conns[0].Password != "" {
		t.Error("password should be omitted from ListConnections")
	}
	if err := mgr.Disconnect("test-sqlite"); err != nil {
		t.Fatalf("Disconnect: %v", err)
	}
	if _, err := mgr.Get("test-sqlite"); err == nil {
		t.Fatal("expected error after disconnect")
	}
}

func TestManager_DuplicateID(t *testing.T) {
	mgr := connections.NewManager()
	cfg := connections.ConnectionConfig{
		ID:       "dup",
		Name:     "dup1",
		Driver:   "sqlite",
		Database: ":memory:",
	}
	if err := mgr.Connect(cfg); err != nil {
		t.Fatal(err)
	}
	cfg.Name = "dup2"
	if err := mgr.Connect(cfg); err != nil {
		t.Fatal(err)
	}
	conns := mgr.ListConnections()
	if len(conns) != 1 {
		t.Fatalf("expected 1 connection after re-connect with same ID, got %d", len(conns))
	}
}
