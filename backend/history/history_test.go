package history_test

import (
	"context"
	"os"
	"path/filepath"
	"testing"

	"multidb/backend/connections"
	"multidb/backend/history"
)

func newTempStore(t *testing.T) *history.Store {
	t.Helper()
	dir := t.TempDir()
	store, err := history.NewStore(filepath.Join(dir, "test.db"))
	if err != nil {
		t.Fatalf("NewStore: %v", err)
	}
	t.Cleanup(func() { store.Close() })
	return store
}

func TestAddAndGetQueryHistory(t *testing.T) {
	store := newTempStore(t)
	ctx := context.Background()

	rec := history.QueryRecord{
		ConnID:    "conn1",
		Query:     "SELECT 1",
		Duration:  5,
		CreatedAt: "2026-01-01T00:00:00Z",
	}
	if err := store.AddQueryHistory(ctx, rec); err != nil {
		t.Fatalf("AddQueryHistory: %v", err)
	}

	records, err := store.GetQueryHistory(ctx, 10)
	if err != nil {
		t.Fatalf("GetQueryHistory: %v", err)
	}
	if len(records) != 1 {
		t.Fatalf("expected 1 record, got %d", len(records))
	}
	if records[0].Query != "SELECT 1" {
		t.Errorf("unexpected query: %q", records[0].Query)
	}
}

func TestClearQueryHistory(t *testing.T) {
	store := newTempStore(t)
	ctx := context.Background()

	for i := 0; i < 5; i++ {
		_ = store.AddQueryHistory(ctx, history.QueryRecord{ConnID: "c", Query: "SELECT 1", Duration: 1})
	}

	if err := store.ClearQueryHistory(ctx); err != nil {
		t.Fatalf("ClearQueryHistory: %v", err)
	}
	records, _ := store.GetQueryHistory(ctx, 100)
	if len(records) != 0 {
		t.Fatalf("expected 0 records after clear, got %d", len(records))
	}
}

func TestSaveAndListConnections(t *testing.T) {
	store := newTempStore(t)
	ctx := context.Background()

	cfg := connections.ConnectionConfig{
		ID:       "conn-1",
		Name:     "Test DB",
		Driver:   "sqlite",
		Database: ":memory:",
	}
	if err := store.SaveConnection(ctx, cfg); err != nil {
		t.Fatalf("SaveConnection: %v", err)
	}

	cfgs, err := store.ListSavedConnections(ctx)
	if err != nil {
		t.Fatalf("ListSavedConnections: %v", err)
	}
	if len(cfgs) != 1 {
		t.Fatalf("expected 1 saved connection, got %d", len(cfgs))
	}
	if cfgs[0].Name != "Test DB" {
		t.Errorf("unexpected name: %q", cfgs[0].Name)
	}
}

func TestDeleteConnection(t *testing.T) {
	store := newTempStore(t)
	ctx := context.Background()

	_ = store.SaveConnection(ctx, connections.ConnectionConfig{ID: "x", Name: "X", Driver: "sqlite"})
	if err := store.DeleteConnection(ctx, "x"); err != nil {
		t.Fatalf("DeleteConnection: %v", err)
	}
	cfgs, _ := store.ListSavedConnections(ctx)
	if len(cfgs) != 0 {
		t.Fatalf("expected 0 connections, got %d", len(cfgs))
	}
}

func TestSaveConnection_Upsert(t *testing.T) {
	store := newTempStore(t)
	ctx := context.Background()

	cfg := connections.ConnectionConfig{ID: "u", Name: "Original", Driver: "sqlite"}
	_ = store.SaveConnection(ctx, cfg)

	cfg.Name = "Updated"
	if err := store.SaveConnection(ctx, cfg); err != nil {
		t.Fatalf("upsert: %v", err)
	}

	cfgs, _ := store.ListSavedConnections(ctx)
	if len(cfgs) != 1 {
		t.Fatalf("expected 1 connection, got %d", len(cfgs))
	}
	if cfgs[0].Name != "Updated" {
		t.Errorf("expected Updated, got %q", cfgs[0].Name)
	}
}

// Make sure os is accessible even though unused directly
var _ = os.TempDir
