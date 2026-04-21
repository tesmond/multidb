package ui

import (
	"fmt"
	"strings"

	"multidb/backend/connections"
	"multidb/backend/schema"

	"fyne.io/fyne/v2"
	"fyne.io/fyne/v2/container"
	"fyne.io/fyne/v2/theme"
	"fyne.io/fyne/v2/widget"
)

// nodeKind distinguishes different tree node types by their ID prefix.
const (
	nodeConn       = "c:"   // c:{connID}
	nodeConnTables = "ct:"  // ct:{connID}
	nodeConnViews  = "cv:"  // cv:{connID}
	nodeConnIdxs   = "ci:"  // ci:{connID}
	nodeSchemas    = "cs:"  // cs:{connID}
	nodeSchemaSec  = "ss:"  // ss:{connID}:{schemaName}
	nodeSchemaTbls = "st:"  // st:{connID}:{schemaName}
	nodeSchemaVws  = "sv:"  // sv:{connID}:{schemaName}
	nodeSchemaIdxs = "sx:"  // sx:{connID}:{schemaName}
	nodeTable      = "t:"   // t:{connID}:{schemaName}:{tableName} (schemaName="" for flat)
	nodeView       = "v:"   // v:{connID}:{schemaName}:{viewName}
	nodeColumn     = "col:" // col:{connID}:{schemaName}:{tableName}:{colName}
	nodeIndex      = "idx:" // idx:{connID}:{schemaName}:{indexName}
	nodeLoading    = "loading:"
	nodeError      = "error:"
	nodeEmpty      = "empty"
)

// NewNavigator builds the navigator sidebar.
func NewNavigator(state *AppState) fyne.CanvasObject {
	var tree *widget.Tree

	// Build child ID lists for each node
	childOf := func(uid widget.TreeNodeID) []widget.TreeNodeID {
		if uid == "" {
			// Root: one node per connection
			conns := state.GetConns()
			ids := make([]widget.TreeNodeID, len(conns))
			for i, c := range conns {
				ids[i] = widget.TreeNodeID(nodeConn + c.Config.ID)
			}
			if len(ids) == 0 {
				return []widget.TreeNodeID{nodeEmpty}
			}
			return ids
		}

		s := string(uid)

		// Connection node -> its schema sections
		if strings.HasPrefix(s, nodeConn) && !strings.HasPrefix(s, nodeConnTables) &&
			!strings.HasPrefix(s, nodeConnViews) && !strings.HasPrefix(s, nodeConnIdxs) &&
			!strings.HasPrefix(s, nodeSchemas) {
			connID := s[len(nodeConn):]
			conn := state.GetConn(connID)
			if conn == nil {
				return nil
			}
			if conn.SchemaLoading {
				return []widget.TreeNodeID{widget.TreeNodeID(nodeLoading + connID)}
			}
			if conn.SchemaError != "" {
				return []widget.TreeNodeID{widget.TreeNodeID(nodeError + connID)}
			}
			if conn.Schema == nil {
				return nil
			}
			if len(conn.Schema.Schemas) > 0 {
				return []widget.TreeNodeID{widget.TreeNodeID(nodeSchemas + connID)}
			}
			// Flat (sqlite)
			var out []widget.TreeNodeID
			if len(conn.Schema.Tables) > 0 {
				out = append(out, widget.TreeNodeID(nodeConnTables+connID))
			}
			if len(conn.Schema.Views) > 0 {
				out = append(out, widget.TreeNodeID(nodeConnViews+connID))
			}
			if len(conn.Schema.Indexes) > 0 {
				out = append(out, widget.TreeNodeID(nodeConnIdxs+connID))
			}
			return out
		}

		// "Schemas" group -> individual schema sections
		if strings.HasPrefix(s, nodeSchemas) {
			connID := s[len(nodeSchemas):]
			conn := state.GetConn(connID)
			if conn == nil || conn.Schema == nil {
				return nil
			}
			out := make([]widget.TreeNodeID, len(conn.Schema.Schemas))
			for i, sc := range conn.Schema.Schemas {
				out[i] = widget.TreeNodeID(nodeSchemaSec + connID + ":" + sc.Name)
			}
			return out
		}

		// Individual schema section -> tables/views/indexes subsections
		if strings.HasPrefix(s, nodeSchemaSec) {
			rest := s[len(nodeSchemaSec):]
			parts := strings.SplitN(rest, ":", 2)
			if len(parts) != 2 {
				return nil
			}
			connID, schemaName := parts[0], parts[1]
			conn := state.GetConn(connID)
			if conn == nil || conn.Schema == nil {
				return nil
			}
			var sc *schema.Schema
			for i := range conn.Schema.Schemas {
				if conn.Schema.Schemas[i].Name == schemaName {
					sc = &conn.Schema.Schemas[i]
					break
				}
			}
			if sc == nil {
				return nil
			}
			var out []widget.TreeNodeID
			if len(sc.Tables) > 0 {
				out = append(out, widget.TreeNodeID(nodeSchemaTbls+connID+":"+schemaName))
			}
			if len(sc.Views) > 0 {
				out = append(out, widget.TreeNodeID(nodeSchemaVws+connID+":"+schemaName))
			}
			if len(sc.Indexes) > 0 {
				out = append(out, widget.TreeNodeID(nodeSchemaIdxs+connID+":"+schemaName))
			}
			return out
		}

		// Tables section -> table nodes
		if strings.HasPrefix(s, nodeSchemaTbls) {
			rest := s[len(nodeSchemaTbls):]
			parts := strings.SplitN(rest, ":", 2)
			if len(parts) != 2 {
				return nil
			}
			connID, schemaName := parts[0], parts[1]
			conn := state.GetConn(connID)
			if conn == nil || conn.Schema == nil {
				return nil
			}
			for _, sc := range conn.Schema.Schemas {
				if sc.Name == schemaName {
					out := make([]widget.TreeNodeID, len(sc.Tables))
					for i, t := range sc.Tables {
						out[i] = widget.TreeNodeID(nodeTable + connID + ":" + schemaName + ":" + t.Name)
					}
					return out
				}
			}
			return nil
		}

		// Flat tables section
		if strings.HasPrefix(s, nodeConnTables) {
			connID := s[len(nodeConnTables):]
			conn := state.GetConn(connID)
			if conn == nil || conn.Schema == nil {
				return nil
			}
			out := make([]widget.TreeNodeID, len(conn.Schema.Tables))
			for i, t := range conn.Schema.Tables {
				out[i] = widget.TreeNodeID(nodeTable + connID + "::" + t.Name)
			}
			return out
		}

		// Views section (schema)
		if strings.HasPrefix(s, nodeSchemaVws) {
			rest := s[len(nodeSchemaVws):]
			parts := strings.SplitN(rest, ":", 2)
			if len(parts) != 2 {
				return nil
			}
			connID, schemaName := parts[0], parts[1]
			conn := state.GetConn(connID)
			if conn == nil || conn.Schema == nil {
				return nil
			}
			for _, sc := range conn.Schema.Schemas {
				if sc.Name == schemaName {
					out := make([]widget.TreeNodeID, len(sc.Views))
					for i, v := range sc.Views {
						out[i] = widget.TreeNodeID(nodeView + connID + ":" + schemaName + ":" + v.Name)
					}
					return out
				}
			}
			return nil
		}

		// Flat views section
		if strings.HasPrefix(s, nodeConnViews) {
			connID := s[len(nodeConnViews):]
			conn := state.GetConn(connID)
			if conn == nil || conn.Schema == nil {
				return nil
			}
			out := make([]widget.TreeNodeID, len(conn.Schema.Views))
			for i, v := range conn.Schema.Views {
				out[i] = widget.TreeNodeID(nodeView + connID + "::" + v.Name)
			}
			return out
		}

		// Schema indexes section
		if strings.HasPrefix(s, nodeSchemaIdxs) {
			rest := s[len(nodeSchemaIdxs):]
			parts := strings.SplitN(rest, ":", 2)
			if len(parts) != 2 {
				return nil
			}
			connID, schemaName := parts[0], parts[1]
			conn := state.GetConn(connID)
			if conn == nil || conn.Schema == nil {
				return nil
			}
			for _, sc := range conn.Schema.Schemas {
				if sc.Name == schemaName {
					out := make([]widget.TreeNodeID, len(sc.Indexes))
					for i, idx := range sc.Indexes {
						out[i] = widget.TreeNodeID(nodeIndex + connID + ":" + schemaName + ":" + idx)
					}
					return out
				}
			}
			return nil
		}

		// Flat indexes section
		if strings.HasPrefix(s, nodeConnIdxs) {
			connID := s[len(nodeConnIdxs):]
			conn := state.GetConn(connID)
			if conn == nil || conn.Schema == nil {
				return nil
			}
			out := make([]widget.TreeNodeID, len(conn.Schema.Indexes))
			for i, idx := range conn.Schema.Indexes {
				out[i] = widget.TreeNodeID(nodeIndex + connID + "::" + idx)
			}
			return out
		}

		// Table node -> column nodes
		if strings.HasPrefix(s, nodeTable) {
			rest := s[len(nodeTable):]
			parts := strings.SplitN(rest, ":", 3)
			if len(parts) != 3 {
				return nil
			}
			connID, schemaName, tableName := parts[0], parts[1], parts[2]
			conn := state.GetConn(connID)
			if conn == nil || conn.Schema == nil {
				return nil
			}
			var cols []schema.Column
			if schemaName == "" {
				for _, t := range conn.Schema.Tables {
					if t.Name == tableName {
						cols = t.Columns
						break
					}
				}
			} else {
				for _, sc := range conn.Schema.Schemas {
					if sc.Name == schemaName {
						for _, t := range sc.Tables {
							if t.Name == tableName {
								cols = t.Columns
								break
							}
						}
						break
					}
				}
			}
			out := make([]widget.TreeNodeID, len(cols))
			for i, c := range cols {
				out[i] = widget.TreeNodeID(nodeColumn + connID + ":" + schemaName + ":" + tableName + ":" + c.Name)
			}
			return out
		}

		return nil
	}

	isBranch := func(uid widget.TreeNodeID) bool {
		s := string(uid)
		// Leaf nodes: column, index, loading, error, empty, view
		if strings.HasPrefix(s, nodeColumn) || strings.HasPrefix(s, nodeIndex) ||
			strings.HasPrefix(s, nodeLoading) || strings.HasPrefix(s, nodeError) ||
			strings.HasPrefix(s, nodeView) || s == nodeEmpty {
			return false
		}
		return true
	}

	createNode := func(branch bool) fyne.CanvasObject {
		icon := widget.NewIcon(theme.DocumentIcon())
		label := widget.NewLabel("")
		label.TextStyle = fyne.TextStyle{}
		detail := widget.NewLabel("")
		detail.TextStyle = fyne.TextStyle{Italic: true}
		detail.Importance = widget.LowImportance
		return container.NewHBox(icon, label, detail)
	}

	updateNode := func(uid widget.TreeNodeID, branch bool, node fyne.CanvasObject) {
		box := node.(*fyne.Container)
		icon := box.Objects[0].(*widget.Icon)
		label := box.Objects[1].(*widget.Label)
		detail := box.Objects[2].(*widget.Label)

		s := string(uid)

		switch {
		case s == nodeEmpty:
			icon.SetResource(theme.InfoIcon())
			label.SetText("No connections")
			detail.SetText("")

		case strings.HasPrefix(s, nodeLoading):
			icon.SetResource(theme.ViewRefreshIcon())
			label.SetText("Loading schema…")
			detail.SetText("")

		case strings.HasPrefix(s, nodeError):
			connID := s[len(nodeError):]
			conn := state.GetConn(connID)
			errMsg := ""
			if conn != nil {
				errMsg = conn.SchemaError
			}
			icon.SetResource(theme.ErrorIcon())
			label.SetText("Error: " + errMsg)
			detail.SetText("")

		case strings.HasPrefix(s, nodeConn) && !isSubConnNode(s):
			connID := s[len(nodeConn):]
			conn := state.GetConn(connID)
			if conn == nil {
				label.SetText("?")
				detail.SetText("")
				return
			}
			icon.SetResource(theme.StorageIcon())
			label.SetText(conn.Config.Name)
			detail.SetText("[" + conn.Config.Driver + "]")

		case strings.HasPrefix(s, nodeSchemas):
			icon.SetResource(theme.FolderIcon())
			label.SetText("Schemas")
			detail.SetText("")

		case strings.HasPrefix(s, nodeSchemaSec):
			rest := s[len(nodeSchemaSec):]
			parts := strings.SplitN(rest, ":", 2)
			schemaName := ""
			if len(parts) == 2 {
				schemaName = parts[1]
			}
			icon.SetResource(theme.FolderIcon())
			label.SetText(schemaName)
			detail.SetText("")

		case strings.HasPrefix(s, nodeSchemaTbls) || strings.HasPrefix(s, nodeConnTables):
			icon.SetResource(theme.ListIcon())
			label.SetText("Tables")
			detail.SetText("")

		case strings.HasPrefix(s, nodeSchemaVws) || strings.HasPrefix(s, nodeConnViews):
			icon.SetResource(theme.ListIcon())
			label.SetText("Views")
			detail.SetText("")

		case strings.HasPrefix(s, nodeSchemaIdxs) || strings.HasPrefix(s, nodeConnIdxs):
			icon.SetResource(theme.ListIcon())
			label.SetText("Indexes")
			detail.SetText("")

		case strings.HasPrefix(s, nodeTable):
			rest := s[len(nodeTable):]
			parts := strings.SplitN(rest, ":", 3)
			tableName := ""
			if len(parts) == 3 {
				tableName = parts[2]
			}
			icon.SetResource(theme.GridIcon())
			label.SetText(tableName)
			detail.SetText("")

		case strings.HasPrefix(s, nodeView):
			rest := s[len(nodeView):]
			parts := strings.SplitN(rest, ":", 3)
			viewName := ""
			if len(parts) == 3 {
				viewName = parts[2]
			}
			icon.SetResource(theme.VisibilityIcon())
			label.SetText(viewName)
			detail.SetText("view")

		case strings.HasPrefix(s, nodeIndex):
			rest := s[len(nodeIndex):]
			parts := strings.SplitN(rest, ":", 3)
			idxName := ""
			if len(parts) == 3 {
				idxName = parts[2]
			}
			icon.SetResource(theme.SearchIcon())
			label.SetText(idxName)
			detail.SetText("index")

		case strings.HasPrefix(s, nodeColumn):
			rest := s[len(nodeColumn):]
			parts := strings.SplitN(rest, ":", 4)
			colName, colType, colKey := "", "", ""
			if len(parts) == 4 {
				colName = parts[3]
				// Look up type info
				connID, schemaName, tableName := parts[0], parts[1], parts[2]
				conn := state.GetConn(connID)
				if conn != nil && conn.Schema != nil {
					col := findColumn(conn.Schema, schemaName, tableName, colName)
					if col != nil {
						colType = col.Type
						colKey = col.Key
					}
				}
			}
			icon.SetResource(theme.DocumentIcon())
			label.SetText(colName)
			typeStr := colType
			if colKey == "PRI" {
				typeStr = "PK " + typeStr
			}
			detail.SetText(typeStr)
		}
	}

	tree = widget.NewTree(childOf, isBranch, createNode, updateNode)
	treeWithMenu := newNavigatorTreeWithMenu(tree, state)

	// Handle selection / double-tap actions
	tree.OnSelected = func(uid widget.TreeNodeID) {
		treeWithMenu.setSelected(uid)
		s := string(uid)

		// Expand/collapse branches on click
		if isBranch(uid) {
			if tree.IsBranchOpen(uid) {
				tree.CloseBranch(uid)
			} else {
				tree.OpenBranch(uid)
				// Auto-load schema when expanding a connection node
				if strings.HasPrefix(s, nodeConn) && !isSubConnNode(s) {
					connID := s[len(nodeConn):]
					conn := state.GetConn(connID)
					if conn != nil && conn.Schema == nil && !conn.SchemaLoading {
						state.LoadSchemaForConn(connID)
					}
				}
			}
		}

		// Double-tap on table -> view data (handled via OnSelected for simplicity)
		// The context menu handles right-click, this handles double-click via menu below
	}

	// Context menu handler (secondary tap)
	tree.OnUnselected = func(uid widget.TreeNodeID) {}

	// Register refresh callback
	state.onRefreshNav = func() {
		tree.Refresh()
	}

	// Add connection button
	addBtn := widget.NewButtonWithIcon("", theme.ContentAddIcon(), func() {
		showConnectionDialog(state, nil)
	})
	addBtn.Importance = widget.LowImportance

	header := container.NewBorder(nil, nil, nil, addBtn,
		widget.NewLabelWithStyle("Connections", fyne.TextAlignLeading, fyne.TextStyle{Bold: true}),
	)

	return container.NewBorder(header, nil, nil, nil, treeWithMenu)
}

// isSubConnNode checks if a node ID that starts with "c:" is actually a sub-node
// that also starts with another prefix.
func isSubConnNode(s string) bool {
	return strings.HasPrefix(s, nodeConnTables) ||
		strings.HasPrefix(s, nodeConnViews) ||
		strings.HasPrefix(s, nodeConnIdxs) ||
		strings.HasPrefix(s, nodeSchemas) ||
		strings.HasPrefix(s, nodeSchemaSec) ||
		strings.HasPrefix(s, nodeSchemaTbls) ||
		strings.HasPrefix(s, nodeSchemaVws) ||
		strings.HasPrefix(s, nodeSchemaIdxs)
}

func findColumn(tree *schema.SchemaTree, schemaName, tableName, colName string) *schema.Column {
	var tables []schema.Table
	if schemaName == "" {
		tables = tree.Tables
	} else {
		for _, sc := range tree.Schemas {
			if sc.Name == schemaName {
				tables = sc.Tables
				break
			}
		}
	}
	for _, t := range tables {
		if t.Name == tableName {
			for i, c := range t.Columns {
				if c.Name == colName {
					return &t.Columns[i]
				}
			}
		}
	}
	return nil
}

// qualifyTableName returns a driver-appropriate qualified table name.
func qualifyTableName(driver, schemaName, tableName string) string {
	if schemaName == "" {
		return quoteIdent(driver, tableName)
	}
	return quoteIdent(driver, schemaName) + "." + quoteIdent(driver, tableName)
}
func quoteIdent(driver, name string) string {
	switch driver {
	case "mysql":
		return "`" + name + "`"
	case "postgres":
		return `"` + name + `"`
	default:
		return name
	}
}

// navigatorTreeWithMenu wraps a widget.Tree and intercepts secondary taps
// to show a context menu.
type navigatorTreeWithMenu struct {
	widget.BaseWidget
	tree     *widget.Tree
	state    *AppState
	selected widget.TreeNodeID
}

func newNavigatorTreeWithMenu(tree *widget.Tree, state *AppState) *navigatorTreeWithMenu {
	w := &navigatorTreeWithMenu{tree: tree, state: state}
	w.ExtendBaseWidget(w)
	return w
}

func (w *navigatorTreeWithMenu) setSelected(uid widget.TreeNodeID) {
	w.selected = uid
}

func (w *navigatorTreeWithMenu) CreateRenderer() fyne.WidgetRenderer {
	return widget.NewSimpleRenderer(w.tree)
}

func (w *navigatorTreeWithMenu) MinSize() fyne.Size {
	return w.tree.MinSize()
}

func (w *navigatorTreeWithMenu) Tapped(e *fyne.PointEvent) {
	// forward to tree
}

func (w *navigatorTreeWithMenu) TappedSecondary(e *fyne.PointEvent) {
	selected := w.selected
	if selected == "" {
		return
	}
	w.showContextMenu(selected, e.AbsolutePosition)
}

func (w *navigatorTreeWithMenu) showContextMenu(uid widget.TreeNodeID, pos fyne.Position) {
	s := string(uid)
	state := w.state

	if strings.HasPrefix(s, nodeConn) && !isSubConnNode(s) {
		connID := s[len(nodeConn):]
		conn := state.GetConn(connID)
		if conn == nil {
			return
		}

		items := []*fyne.MenuItem{
			fyne.NewMenuItem("Test Connection", func() {
				go func() {
					err := state.Svc.TestConnection(conn.Config)
					if err != nil {
						state.SetStatus("Test failed: " + err.Error())
					} else {
						state.SetStatus("Connection test successful: " + conn.Config.Name)
					}
				}()
			}),
			fyne.NewMenuItemSeparator(),
			fyne.NewMenuItem("Refresh Schema", func() {
				state.RefreshSchemaForConn(connID)
			}),
			fyne.NewMenuItemSeparator(),
			fyne.NewMenuItem("Edit Connection", func() {
				showConnectionDialog(state, &conn.Config)
			}),
			fyne.NewMenuItem("Disconnect & Delete", func() {
				_ = state.Svc.DeleteConnection(connID)
				state.RemoveConn(connID)
			}),
		}
		menu := fyne.NewMenu("", items...)
		widget.ShowPopUpMenuAtPosition(menu, fyne.CurrentApp().Driver().CanvasForObject(w), pos)
		return
	}

	if strings.HasPrefix(s, nodeTable) {
		rest := s[len(nodeTable):]
		parts := strings.SplitN(rest, ":", 3)
		if len(parts) != 3 {
			return
		}
		connID, schemaName, tableName := parts[0], parts[1], parts[2]
		conn := state.GetConn(connID)
		if conn == nil {
			return
		}
		qualName := qualifyTableName(conn.Config.Driver, schemaName, tableName)

		items := []*fyne.MenuItem{
			fyne.NewMenuItem("View Data", func() {
				sql := fmt.Sprintf("SELECT * FROM %s LIMIT 1000", qualName)
				tab := state.AddTab(connID)
				tab.SQL = sql
				tab.Title = tableName
				state.doRefreshTabs()
				state.doRefreshEditor()
			}),
			fyne.NewMenuItem("Copy Name", func() {
				state.Window.Clipboard().SetContent(qualName)
			}),
		}
		menu := fyne.NewMenu("", items...)
		widget.ShowPopUpMenuAtPosition(menu, fyne.CurrentApp().Driver().CanvasForObject(w), pos)
	}
}

// Ensure connections import is used via compile-time check.
var _ connections.ConnectionConfig
