package ui

import (
	"fmt"
	"regexp"
	"sync"
	"time"

	"multidb/backend/connections"
	"multidb/backend/schema"
	"multidb/backend/service"

	"fyne.io/fyne/v2"
)

// QueryResult holds the result of a query execution.
type QueryResult struct {
	Columns      []string
	ColumnTypes  []string
	Rows         [][]any
	RowsAffected int64
	DurationMs   int64
	Error        string
}

// TabState holds state for a single editor tab.
type TabState struct {
	ID              string
	Title           string
	ConnID          string
	SQL             string
	Result          *QueryResult
	Running         bool
	QueryID         string
	ManuallyRenamed bool
	SortCol         int    // -1 = unsorted
	SortDir         string // "asc" | "desc"
}

// ActiveConn holds state for an active database connection.
type ActiveConn struct {
	Config        connections.ConnectionConfig
	Schema        *schema.SchemaTree
	SchemaLoading bool
	SchemaError   string
}

// AppState is the central state manager for the application.
type AppState struct {
	Svc *service.Service

	// Window is the main application window. Set by Run() after w.NewWindow.
	Window fyne.Window

	mu          sync.RWMutex
	tabs        []*TabState
	activeTabID string
	conns       []*ActiveConn
	statusMsg   string
	outputTab   string // "results" | "messages" | "history"

	// UI refresh callbacks (set by UI components after creation)
	onRefreshNav    func()
	onRefreshTabs   func()
	onRefreshEditor func()
	onRefreshGrid   func()
	onRefreshStatus func()
	onRefreshOutput func()
}

// NewAppState creates and initialises a new AppState.
func NewAppState(svc *service.Service) *AppState {
	s := &AppState{
		Svc:       svc,
		outputTab: "results",
	}
	tab := s.newTabState("")
	s.tabs = []*TabState{tab}
	s.activeTabID = tab.ID
	if conns, err := svc.ListSavedConnections(); err == nil {
		for _, cfg := range conns {
			s.conns = append(s.conns, &ActiveConn{Config: cfg})
		}
	}
	return s
}

func (s *AppState) newTabState(connID string) *TabState {
	return &TabState{
		ID:      fmt.Sprintf("tab-%d", time.Now().UnixNano()),
		Title:   "Query",
		ConnID:  connID,
		SortCol: -1,
		SortDir: "asc",
	}
}

// ─── Tab API ─────────────────────────────────────────────────────────────────────────

func (s *AppState) GetTabs() []*TabState {
	s.mu.RLock()
	defer s.mu.RUnlock()
	out := make([]*TabState, len(s.tabs))
	copy(out, s.tabs)
	return out
}

func (s *AppState) GetActiveTabID() string {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return s.activeTabID
}

func (s *AppState) ActiveTab() *TabState {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return s.findTabLocked(s.activeTabID)
}

func (s *AppState) GetTab(id string) *TabState {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return s.findTabLocked(id)
}

func (s *AppState) findTabLocked(id string) *TabState {
	for _, t := range s.tabs {
		if t.ID == id {
			return t
		}
	}
	return nil
}

func (s *AppState) tabIndexLocked(id string) int {
	for i, t := range s.tabs {
		if t.ID == id {
			return i
		}
	}
	return -1
}

func (s *AppState) SetActiveTab(id string) {
	s.mu.Lock()
	s.activeTabID = id
	s.mu.Unlock()
	s.doRefreshTabs()
	s.doRefreshEditor()
	s.doRefreshGrid()
	s.doRefreshOutput()
}

func (s *AppState) AddTab(connID string) *TabState {
	s.mu.Lock()
	tab := s.newTabState(connID)
	s.tabs = append(s.tabs, tab)
	s.activeTabID = tab.ID
	s.mu.Unlock()
	s.doRefreshTabs()
	s.doRefreshEditor()
	s.doRefreshGrid()
	return tab
}
func (s *AppState) DuplicateTab(id string) {
	s.mu.Lock()
	idx := s.tabIndexLocked(id)
	if idx < 0 {
		s.mu.Unlock()
		return
	}
	orig := s.tabs[idx]
	dup := &TabState{
		ID:      fmt.Sprintf("tab-%d", time.Now().UnixNano()),
		Title:   orig.Title + " (Copy)",
		ConnID:  orig.ConnID,
		SQL:     orig.SQL,
		SortCol: orig.SortCol,
		SortDir: orig.SortDir,
	}
	newTabs := make([]*TabState, 0, len(s.tabs)+1)
	newTabs = append(newTabs, s.tabs[:idx+1]...)
	newTabs = append(newTabs, dup)
	newTabs = append(newTabs, s.tabs[idx+1:]...)
	s.tabs = newTabs
	s.activeTabID = dup.ID
	s.mu.Unlock()
	s.doRefreshTabs()
	s.doRefreshEditor()
}

func (s *AppState) CloseTab(id string) {
	s.mu.Lock()
	idx := s.tabIndexLocked(id)
	if idx < 0 {
		s.mu.Unlock()
		return
	}
	s.tabs = append(s.tabs[:idx], s.tabs[idx+1:]...)
	if len(s.tabs) == 0 {
		tab := s.newTabState("")
		s.tabs = []*TabState{tab}
		s.activeTabID = tab.ID
	} else if s.activeTabID == id {
		ni := idx
		if ni >= len(s.tabs) {
			ni = len(s.tabs) - 1
		}
		s.activeTabID = s.tabs[ni].ID
	}
	s.mu.Unlock()
	s.doRefreshTabs()
	s.doRefreshEditor()
	s.doRefreshGrid()
}

func (s *AppState) CloseOtherTabs(id string) {
	s.mu.Lock()
	kept := s.findTabLocked(id)
	if kept == nil {
		s.mu.Unlock()
		return
	}
	s.tabs = []*TabState{kept}
	s.activeTabID = kept.ID
	s.mu.Unlock()
	s.doRefreshTabs()
	s.doRefreshEditor()
	s.doRefreshGrid()
}

func (s *AppState) CloseTabsRight(id string) {
	s.mu.Lock()
	idx := s.tabIndexLocked(id)
	if idx < 0 {
		s.mu.Unlock()
		return
	}
	s.tabs = s.tabs[:idx+1]
	if s.tabIndexLocked(s.activeTabID) < 0 {
		s.activeTabID = s.tabs[len(s.tabs)-1].ID
	}
	s.mu.Unlock()
	s.doRefreshTabs()
	s.doRefreshEditor()
	s.doRefreshGrid()
}

func (s *AppState) CloseTabsLeft(id string) {
	s.mu.Lock()
	idx := s.tabIndexLocked(id)
	if idx < 0 {
		s.mu.Unlock()
		return
	}
	s.tabs = s.tabs[idx:]
	if s.tabIndexLocked(s.activeTabID) < 0 {
		s.activeTabID = s.tabs[0].ID
	}
	s.mu.Unlock()
	s.doRefreshTabs()
	s.doRefreshEditor()
	s.doRefreshGrid()
}

func (s *AppState) RenameTab(id, newTitle string) {
	s.mu.Lock()
	if tab := s.findTabLocked(id); tab != nil {
		tab.Title = newTitle
		tab.ManuallyRenamed = true
	}
	s.mu.Unlock()
	s.doRefreshTabs()
}

func (s *AppState) ReorderTabs(from, to int) {
	s.mu.Lock()
	n := len(s.tabs)
	if from < 0 || from >= n || to < 0 || to >= n || from == to {
		s.mu.Unlock()
		return
	}
	tab := s.tabs[from]
	s.tabs = append(s.tabs[:from], s.tabs[from+1:]...)
	// re-insert at adjusted position
	if to > from {
		to--
	}
	rear := append([]*TabState{}, s.tabs[to:]...)
	s.tabs = append(s.tabs[:to], tab)
	s.tabs = append(s.tabs, rear...)
	s.mu.Unlock()
	s.doRefreshTabs()
}

// MoveTabLeft moves the tab one position to the left when possible.
func (s *AppState) MoveTabLeft(id string) {
	s.mu.RLock()
	idx := s.tabIndexLocked(id)
	s.mu.RUnlock()
	if idx <= 0 {
		return
	}
	s.ReorderTabs(idx, idx-1)
}

// MoveTabRight moves the tab one position to the right when possible.
func (s *AppState) MoveTabRight(id string) {
	s.mu.RLock()
	idx := s.tabIndexLocked(id)
	n := len(s.tabs)
	s.mu.RUnlock()
	if idx < 0 || idx >= n-1 {
		return
	}
	s.ReorderTabs(idx, idx+1)
}

func (s *AppState) UpdateTabSQL(id, sql string) {
	s.mu.Lock()
	if tab := s.findTabLocked(id); tab != nil {
		tab.SQL = sql
	}
	s.mu.Unlock()
}

func (s *AppState) UpdateTabConnID(id, connID string) {
	s.mu.Lock()
	if tab := s.findTabLocked(id); tab != nil {
		tab.ConnID = connID
	}
	s.mu.Unlock()
	s.doRefreshEditor()
}

// ─── Connection API ───────────────────────────────────────────────────────────────────────

func (s *AppState) GetConns() []*ActiveConn {
	s.mu.RLock()
	defer s.mu.RUnlock()
	out := make([]*ActiveConn, len(s.conns))
	copy(out, s.conns)
	return out
}

func (s *AppState) GetConn(id string) *ActiveConn {
	s.mu.RLock()
	defer s.mu.RUnlock()
	for _, c := range s.conns {
		if c.Config.ID == id {
			return c
		}
	}
	return nil
}

func (s *AppState) UpsertConn(conn *ActiveConn) {
	s.mu.Lock()
	for i, c := range s.conns {
		if c.Config.ID == conn.Config.ID {
			s.conns[i] = conn
			s.mu.Unlock()
			s.doRefreshNav()
			return
		}
	}
	s.conns = append(s.conns, conn)
	s.mu.Unlock()
	s.doRefreshNav()
}

func (s *AppState) RemoveConn(id string) {
	s.mu.Lock()
	for i, c := range s.conns {
		if c.Config.ID == id {
			s.conns = append(s.conns[:i], s.conns[i+1:]...)
			break
		}
	}
	s.mu.Unlock()
	s.doRefreshNav()
}

func (s *AppState) SetConnSchema(connID string, tree *schema.SchemaTree, loading bool, errStr string) {
	s.mu.Lock()
	for _, c := range s.conns {
		if c.Config.ID == connID {
			c.Schema = tree
			c.SchemaLoading = loading
			c.SchemaError = errStr
			break
		}
	}
	s.mu.Unlock()
	s.doRefreshNav()
	s.doRefreshEditor()
}

// ─── Status / Output ──────────────────────────────────────────────────────────────

func (s *AppState) SetStatus(msg string) {
	s.mu.Lock()
	s.statusMsg = msg
	s.mu.Unlock()
	s.doRefreshStatus()
}

func (s *AppState) GetStatus() string {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return s.statusMsg
}

func (s *AppState) SetOutputTab(tab string) {
	s.mu.Lock()
	s.outputTab = tab
	s.mu.Unlock()
	s.doRefreshOutput()
}

func (s *AppState) GetOutputTab() string {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return s.outputTab
}

// ─── Query execution ───────────────────────────────────────────────────────────────────

// RunQuery begins a streaming query for the given tab.
func (s *AppState) RunQuery(tabID string) {
	s.mu.RLock()
	tab := s.findTabLocked(tabID)
	if tab == nil || tab.Running {
		s.mu.RUnlock()
		return
	}
	connID := tab.ConnID
	sqlText := tab.SQL
	s.mu.RUnlock()

	if connID == "" {
		s.SetStatus("No connection selected. Connect to a database first.")
		return
	}
	if sqlText == "" {
		return
	}

	queryID := fmt.Sprintf("q-%d", time.Now().UnixNano())
	s.mu.Lock()
	if t := s.findTabLocked(tabID); t != nil {
		t.Running = true
		t.QueryID = queryID
		t.Result = nil
	}
	s.mu.Unlock()

	s.SetStatus("Running query…")
	s.SetOutputTab("results")
	s.doRefreshTabs()
	s.doRefreshEditor()
	s.doRefreshGrid()

	go func() {
		var cols []string
		var colTypes []string
		rows := make([][]any, 0, 1024)

		s.Svc.StreamQuery(connID, queryID, sqlText, 1_000_000,
			func(cs []string, cts []string) {
				cols = cs
				colTypes = cts
				s.mu.Lock()
				if t := s.findTabLocked(tabID); t != nil && t.QueryID == queryID {
					t.Result = &QueryResult{
						Columns:     cols,
						ColumnTypes: colTypes,
						Rows:        rows,
					}
				}
				s.mu.Unlock()
				s.doRefreshGrid()
			},
			func(newRows [][]any, offset int) {
				rows = append(rows, newRows...)
				s.mu.Lock()
				if t := s.findTabLocked(tabID); t != nil && t.QueryID == queryID && t.Result != nil {
					t.Result.Rows = rows
				}
				s.mu.Unlock()
				s.SetStatus(fmt.Sprintf("Loading… %d rows", len(rows)))
				s.doRefreshGrid()
			},
			func(total int, dur time.Duration, errStr string) {
				s.mu.Lock()
				if t := s.findTabLocked(tabID); t != nil && t.QueryID == queryID {
					t.Running = false
					t.QueryID = ""
					if errStr != "" {
						t.Result = &QueryResult{Error: errStr}
					} else {
						if t.Result == nil {
							t.Result = &QueryResult{
								Columns:     cols,
								ColumnTypes: colTypes,
								Rows:        rows,
							}
						}
						t.Result.DurationMs = dur.Milliseconds()
						if !t.ManuallyRenamed {
							if name := extractFirstTableName(sqlText); name != "" {
								t.Title = name
							}
						}
					}
				}
				s.mu.Unlock()
				if errStr != "" {
					s.SetStatus("Error: " + errStr)
					s.SetOutputTab("messages")
				} else {
					s.SetStatus(fmt.Sprintf("%d rows · %dms", total, dur.Milliseconds()))
				}
				s.doRefreshTabs()
				s.doRefreshEditor()
				s.doRefreshGrid()
				s.doRefreshOutput()
			},
		)
	}()
}

// CancelQuery cancels the running query on the given tab.
func (s *AppState) CancelQuery(tabID string) {
	s.mu.RLock()
	tab := s.findTabLocked(tabID)
	if tab == nil || !tab.Running {
		s.mu.RUnlock()
		return
	}
	queryID := tab.QueryID
	s.mu.RUnlock()
	s.Svc.CancelQuery(queryID)
	s.mu.Lock()
	if t := s.findTabLocked(tabID); t != nil {
		t.Running = false
		t.QueryID = ""
	}
	s.mu.Unlock()
	s.SetStatus("Query cancelled")
	s.doRefreshTabs()
	s.doRefreshEditor()
}

// ─── Schema loading ────────────────────────────────────────────────────────────────────

// LoadSchemaForConn loads (or refreshes) the schema for a connection.
func (s *AppState) LoadSchemaForConn(connID string) {
	s.SetConnSchema(connID, nil, true, "")
	go func() {
		tree, err := s.Svc.LoadAndCacheSchema(connID)
		if err != nil {
			s.SetConnSchema(connID, nil, false, err.Error())
		} else {
			s.SetConnSchema(connID, &tree, false, "")
		}
	}()
}

// RefreshSchemaForConn forces a live refresh of the schema.
func (s *AppState) RefreshSchemaForConn(connID string) {
	s.SetConnSchema(connID, nil, true, "")
	go func() {
		tree, err := s.Svc.GetSchema(connID)
		if err != nil {
			s.SetConnSchema(connID, nil, false, err.Error())
		} else {
			_ = s.Svc.SaveCachedSchema(connID, tree)
			s.SetConnSchema(connID, &tree, false, "")
		}
	}()
}

// ─── Refresh triggers ───────────────────────────────────────────────────────────────────

func (s *AppState) doRefreshNav() {
	if s.onRefreshNav != nil {
		s.onRefreshNav()
	}
}
func (s *AppState) doRefreshTabs() {
	if s.onRefreshTabs != nil {
		s.onRefreshTabs()
	}
}
func (s *AppState) doRefreshEditor() {
	if s.onRefreshEditor != nil {
		s.onRefreshEditor()
	}
}
func (s *AppState) doRefreshGrid() {
	if s.onRefreshGrid != nil {
		s.onRefreshGrid()
	}
}
func (s *AppState) doRefreshStatus() {
	if s.onRefreshStatus != nil {
		s.onRefreshStatus()
	}
}
func (s *AppState) doRefreshOutput() {
	if s.onRefreshOutput != nil {
		s.onRefreshOutput()
	}
}

// ─── Helpers ───────────────────────────────────────────────────────────────────────────
var tableNameRe = regexp.MustCompile(`(?i)\b(?:FROM|JOIN)\s+(?:` + "`" + `[\w-]+` + "`" + `|"[\w-]+"|'[\w-]+'|\[[\w-]+\]|[\w-]+)(?:\s*\.\s*(?:` + "`" + `[\w-]+` + "`" + `|"[\w-]+"|'[\w-]+'|\[[\w-]+\]|([\w-]+)))?`)

func extractFirstTableName(sql string) string {
	simple := regexp.MustCompile("(?i)\\b(?:FROM|JOIN)\\s+(?:[\"\\x60'\\[]?[\\w-]+[\"\\x60'\\]]?\\s*\\.\\s*)?[\"\\x60'\\[]?([\\w-]+)[\"\\x60'\\]]?")
	if m := simple.FindStringSubmatch(sql); len(m) > 1 {
		return m[1]
	}
	return ""
}
