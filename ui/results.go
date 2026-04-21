package ui

import (
	"fmt"
	"sort"
	"strings"
	"time"
	"unicode"

	"multidb/backend/history"

	"fyne.io/fyne/v2"
	"fyne.io/fyne/v2/canvas"
	"fyne.io/fyne/v2/container"
	"fyne.io/fyne/v2/driver/desktop"
	"fyne.io/fyne/v2/theme"
	"fyne.io/fyne/v2/widget"
)

// MainWindow is the application main window, required for clipboard operations.
// Set this to the fyne.Window instance after the window is created.
var MainWindow fyne.Window

// resultsGrid wraps widget.Table with sort and selection state.
// widget.Table uses a virtual rendering pattern: only visible cells are
// instantiated, so 1,000,000-row result sets remain performant.
type resultsGrid struct {
	widget.BaseWidget
	state     *AppState
	result    *QueryResult
	sortedIdx []int  // maps display row index to data row index
	sortCol   int    // -1 = unsorted
	sortDir   string // "asc" | "desc"
	selRow    int    // selected display row, -1 = none
	selCol    int    // selected column,      -1 = none
	table     *widget.Table
	colWidths []float32
}

func newResultsGrid(state *AppState) *resultsGrid {
	g := &resultsGrid{
		state:   state,
		sortCol: -1,
		sortDir: "asc",
		selRow:  -1,
		selCol:  -1,
	}
	g.ExtendBaseWidget(g)

	g.table = widget.NewTable(
		func() (int, int) {
			if g.result == nil {
				return 0, 0
			}
			return len(g.sortedIdx), len(g.result.Columns)
		},
		func() fyne.CanvasObject {
			return widget.NewLabel("")
		},
		func(id widget.TableCellID, o fyne.CanvasObject) {
			lbl := o.(*widget.Label)
			if g.result == nil ||
				id.Row >= len(g.sortedIdx) ||
				id.Col >= len(g.result.Columns) {
				lbl.Importance = widget.MediumImportance
				lbl.SetText("")
				return
			}
			dataRow := g.sortedIdx[id.Row]
			if dataRow >= len(g.result.Rows) {
				lbl.Importance = widget.MediumImportance
				lbl.SetText("")
				return
			}
			row := g.result.Rows[dataRow]
			if id.Col >= len(row) {
				lbl.Importance = widget.MediumImportance
				lbl.SetText("")
				return
			}
			v := row[id.Col]
			// Set importance before SetText so the Refresh picks it up.
			switch {
			case id.Row == g.selRow:
				lbl.Importance = widget.HighImportance
			case v == nil:
				lbl.Importance = widget.LowImportance
			default:
				lbl.Importance = widget.MediumImportance
			}
			if v == nil {
				lbl.SetText("NULL")
			} else {
				lbl.SetText(fmt.Sprintf("%v", v))
			}
		},
	)

	g.table.OnSelected = func(id widget.TableCellID) {
		g.selRow = id.Row
		g.selCol = id.Col
		g.table.Refresh()
	}

	return g
}

func (g *resultsGrid) setResult(result *QueryResult) {
	g.result = result
	g.sortCol = -1
	g.sortDir = "asc"
	g.selRow = -1
	g.selCol = -1
	g.rebuildSortedIdx()
	g.applyColWidths()
	g.table.Refresh()
}

// rebuildSortedIdx rebuilds sortedIdx. When sortCol < 0 the mapping is identity.
func (g *resultsGrid) rebuildSortedIdx() {
	if g.result == nil {
		g.sortedIdx = nil
		return
	}
	n := len(g.result.Rows)
	g.sortedIdx = make([]int, n)
	for i := range g.sortedIdx {
		g.sortedIdx[i] = i
	}
	if g.sortCol >= 0 && g.sortCol < len(g.result.Columns) {
		col := g.sortCol
		asc := g.sortDir == "asc"
		sort.SliceStable(g.sortedIdx, func(a, b int) bool {
			ra := g.result.Rows[g.sortedIdx[a]]
			rb := g.result.Rows[g.sortedIdx[b]]
			va, vb := "", ""
			if col < len(ra) && ra[col] != nil {
				va = fmt.Sprintf("%v", ra[col])
			}
			if col < len(rb) && rb[col] != nil {
				vb = fmt.Sprintf("%v", rb[col])
			}
			if asc {
				return va < vb
			}
			return va > vb
		})
	}
}

// sortByColumn toggles asc/desc when col is already sorted, else starts asc.
func (g *resultsGrid) sortByColumn(col int) {
	if g.sortCol == col {
		if g.sortDir == "asc" {
			g.sortDir = "desc"
		} else {
			g.sortDir = "asc"
		}
	} else {
		g.sortCol = col
		g.sortDir = "asc"
	}
	g.rebuildSortedIdx()
	g.table.Refresh()
}

// applyColWidths samples the first 50 rows to estimate column widths and
// calls widget.Table.SetColumnWidth for each column.
func (g *resultsGrid) applyColWidths() {
	if g.result == nil {
		return
	}
	n := len(g.result.Columns)
	g.colWidths = make([]float32, n)
	for i, colName := range g.result.Columns {
		w := float32(len(colName)*8 + 24)
		if w < 60 {
			w = 60
		}
		if w > 300 {
			w = 300
		}
		sample := 50
		if len(g.result.Rows) < sample {
			sample = len(g.result.Rows)
		}
		for r := 0; r < sample; r++ {
			row := g.result.Rows[r]
			if i < len(row) {
				s := ""
				if row[i] != nil {
					s = fmt.Sprintf("%v", row[i])
				}
				cw := float32(len(s)*7 + 24)
				if cw < 60 {
					cw = 60
				}
				if cw > 300 {
					cw = 300
				}
				if cw > w {
					w = cw
				}
			}
		}
		g.colWidths[i] = w
		g.table.SetColumnWidth(i, w)
	}
}

// autoFitColumn scans every row for the given column and expands the width
// to fit the widest value. Intended for a double-click resize-handle gesture.
func (g *resultsGrid) autoFitColumn(col int) {
	if g.result == nil || col >= len(g.result.Columns) {
		return
	}
	w := float32(len(g.result.Columns[col])*8 + 24)
	for _, row := range g.result.Rows {
		if col < len(row) {
			s := ""
			if row[col] != nil {
				s = fmt.Sprintf("%v", row[col])
			}
			cw := float32(countDisplayWidth(s)*7 + 24)
			if cw > w {
				w = cw
			}
		}
	}
	if col < len(g.colWidths) {
		g.colWidths[col] = w
	}
	g.table.SetColumnWidth(col, w)
	g.table.Refresh()
}

// countDisplayWidth returns the display width of s (capped at 80), treating
// CJK characters as width-2.
func countDisplayWidth(s string) int {
	n := 0
	for _, r := range s {
		if unicode.Is(unicode.Han, r) ||
			unicode.Is(unicode.Katakana, r) ||
			unicode.Is(unicode.Hiragana, r) {
			n += 2
		} else {
			n++
		}
	}
	if n > 80 {
		return 80
	}
	return n
}

// copySelection returns the selected row as tab-separated values, or "".
func (g *resultsGrid) copySelection() string {
	if g.result == nil || g.selRow < 0 || g.selRow >= len(g.sortedIdx) {
		return ""
	}
	dataRow := g.sortedIdx[g.selRow]
	if dataRow >= len(g.result.Rows) {
		return ""
	}
	row := g.result.Rows[dataRow]
	if g.selCol >= 0 && g.selCol < len(row) {
		if row[g.selCol] == nil {
			return "NULL"
		}
		return fmt.Sprintf("%v", row[g.selCol])
	}
	parts := make([]string, len(row))
	for i, v := range row {
		if v == nil {
			parts[i] = "NULL"
		} else {
			parts[i] = fmt.Sprintf("%v", v)
		}
	}
	return strings.Join(parts, "\t")
}

func (g *resultsGrid) CreateRenderer() fyne.WidgetRenderer {
	return widget.NewSimpleRenderer(g.table)
}

// resultsGridHeader renders a row of column-name buttons with sort indicators.
type resultsGridHeader struct {
	widget.BaseWidget
	grid    *resultsGrid
	scroll  *container.Scroll
	hbox    *fyne.Container
	lastTap map[int]time.Time
}

func newResultsGridHeader(grid *resultsGrid) *resultsGridHeader {
	h := &resultsGridHeader{grid: grid}
	h.ExtendBaseWidget(h)
	h.hbox = container.NewHBox()
	h.scroll = container.NewHScroll(h.hbox)
	h.scroll.SetMinSize(fyne.NewSize(0, 32))
	h.lastTap = make(map[int]time.Time)
	return h
}

// rebuild regenerates all column header buttons from the current result.
func (h *resultsGridHeader) rebuild() {
	if h.grid.result == nil {
		h.hbox.Objects = nil
		h.hbox.Refresh()
		return
	}
	objects := make([]fyne.CanvasObject, len(h.grid.result.Columns))
	for i, col := range h.grid.result.Columns {
		idx := i
		label := col
		indicator := ""
		if h.grid.sortCol == idx {
			if h.grid.sortDir == "asc" {
				indicator = " \u25b2"
			} else {
				indicator = " \u25bc"
			}
		}
		btn := widget.NewButton(label+indicator, func() {
			now := time.Now()
			if last, ok := h.lastTap[idx]; ok && now.Sub(last) <= 350*time.Millisecond {
				h.grid.autoFitColumn(idx)
				h.lastTap[idx] = time.Time{}
				h.rebuild()
				return
			}
			h.lastTap[idx] = now
			h.grid.sortByColumn(idx)
			h.rebuild()
		})
		btn.Importance = widget.LowImportance
		w := float32(120)
		if idx < len(h.grid.colWidths) {
			w = h.grid.colWidths[idx]
		}
		objects[idx] = container.New(&fixedWidthLayout{width: w}, btn)
	}
	h.hbox.Objects = objects
	h.hbox.Refresh()
	h.scroll.Refresh()
}

func (h *resultsGridHeader) CreateRenderer() fyne.WidgetRenderer {
	bg := canvas.NewRectangle(theme.MenuBackgroundColor())
	return &headerRenderer{bg: bg, scroll: h.scroll}
}

type headerRenderer struct {
	bg     *canvas.Rectangle
	scroll *container.Scroll
}

func (r *headerRenderer) Layout(size fyne.Size) {
	r.bg.Move(fyne.NewPos(0, 0))
	r.bg.Resize(size)
	r.scroll.Move(fyne.NewPos(0, 0))
	r.scroll.Resize(size)
}
func (r *headerRenderer) MinSize() fyne.Size { return fyne.NewSize(0, 32) }
func (r *headerRenderer) Refresh() {
	r.bg.FillColor = theme.MenuBackgroundColor()
	r.bg.Refresh()
	r.scroll.Refresh()
}
func (r *headerRenderer) Destroy() {}
func (r *headerRenderer) Objects() []fyne.CanvasObject {
	return []fyne.CanvasObject{r.bg, r.scroll}
}

// fixedWidthLayout forces its single child to a fixed pixel width.
type fixedWidthLayout struct {
	width float32
}

func (l *fixedWidthLayout) Layout(objs []fyne.CanvasObject, size fyne.Size) {
	for _, o := range objs {
		o.Move(fyne.NewPos(0, 0))
		o.Resize(fyne.NewSize(l.width, size.Height))
	}
}

func (l *fixedWidthLayout) MinSize(objs []fyne.CanvasObject) fyne.Size {
	h := float32(32)
	for _, o := range objs {
		if ms := o.MinSize(); ms.Height > h {
			h = ms.Height
		}
	}
	return fyne.NewSize(l.width, h)
}

// copyableGrid is a focusable wrapper around resultsGrid that handles
// Ctrl+C / Cmd+C via fyne.Focusable + fyne.Shortcutable.
type copyableGrid struct {
	widget.BaseWidget
	grid *resultsGrid
}

func newCopyableGrid(grid *resultsGrid) *copyableGrid {
	c := &copyableGrid{grid: grid}
	c.ExtendBaseWidget(c)
	return c
}

func (c *copyableGrid) CreateRenderer() fyne.WidgetRenderer {
	return widget.NewSimpleRenderer(c.grid)
}

// fyne.Focusable implementation.
func (c *copyableGrid) FocusGained()              {}
func (c *copyableGrid) FocusLost()                {}
func (c *copyableGrid) TypedRune(_ rune)          {}
func (c *copyableGrid) TypedKey(_ *fyne.KeyEvent) {}

// TypedShortcut satisfies fyne.Shortcutable.
// Handles *fyne.ShortcutCopy (standard Ctrl+C / Cmd+C) and any
// *desktop.CustomShortcut with KeyC.
func (c *copyableGrid) TypedShortcut(s fyne.Shortcut) {
	doCopy := func() {
		text := c.grid.copySelection()
		if text != "" && MainWindow != nil {
			MainWindow.Clipboard().SetContent(text)
			c.grid.state.SetStatus("Copied to clipboard")
		}
	}
	if cs, ok := s.(*desktop.CustomShortcut); ok {
		if cs.KeyName == fyne.KeyC {
			doCopy()
		}
		return
	}
	if _, ok := s.(*fyne.ShortcutCopy); ok {
		doCopy()
	}
}

// NewResultsPane returns the bottom output panel with three tabs:
//   - Results  - virtual table grid (handles 1,000,000 rows via widget.Table)
//   - Messages - success or error message for the active query
//   - History  - recent query history; clicking an entry opens a new editor tab
//
// It registers state.onRefreshGrid and state.onRefreshOutput callbacks so the
// rest of the application can push updates via state.doRefreshGrid() /
// state.doRefreshOutput().
func NewResultsPane(state *AppState) fyne.CanvasObject {
	grid := newResultsGrid(state)
	header := newResultsGridHeader(grid)
	copyGrid := newCopyableGrid(grid)

	infoLabel := widget.NewLabel("")
	infoLabel.Importance = widget.LowImportance

	// gridArea: column-header on top, row-count on bottom, table fills center.
	gridArea := container.NewBorder(header, infoLabel, nil, nil, copyGrid)

	emptyLabel := widget.NewLabel("Run a query to see results")
	emptyLabel.Alignment = fyne.TextAlignCenter
	emptyLabel.TextStyle = fyne.TextStyle{Italic: true}

	errorLabel := widget.NewLabel("")
	errorLabel.Importance = widget.DangerImportance
	errorLabel.Wrapping = fyne.TextWrapWord

	// resultsStack shows one of: emptyLabel, errorLabel, or gridArea.
	resultsStack := container.NewMax(emptyLabel)

	// Messages tab
	msgLabel := widget.NewLabel("No messages.")
	msgLabel.Wrapping = fyne.TextWrapWord
	messagesContent := container.NewPadded(container.NewVScroll(msgLabel))

	// History tab
	var histRecords []history.QueryRecord

	histList := widget.NewList(
		func() int { return len(histRecords) },
		func() fyne.CanvasObject {
			queryLbl := widget.NewLabel("")
			queryLbl.TextStyle = fyne.TextStyle{Monospace: true}
			queryLbl.Wrapping = fyne.TextWrapWord
			metaLbl := widget.NewLabel("")
			metaLbl.Importance = widget.LowImportance
			return container.NewVBox(queryLbl, metaLbl)
		},
		func(id widget.ListItemID, o fyne.CanvasObject) {
			if id >= len(histRecords) {
				return
			}
			rec := histRecords[id]
			box := o.(*fyne.Container)
			queryLbl := box.Objects[0].(*widget.Label)
			metaLbl := box.Objects[1].(*widget.Label)
			q := rec.Query
			if len(q) > 120 {
				q = q[:120] + "\u2026"
			}
			queryLbl.SetText(q)
			metaLbl.SetText(fmt.Sprintf("%s  %dms  %d rows",
				rec.CreatedAt, rec.Duration, rec.ResultCount))
		},
	)

	histList.OnSelected = func(id widget.ListItemID) {
		if id >= len(histRecords) {
			return
		}
		rec := histRecords[id]
		tab := state.AddTab(rec.ConnID)
		tab.SQL = rec.Query
		state.doRefreshTabs()
		state.doRefreshEditor()
	}

	histContent := container.NewBorder(nil, nil, nil, nil, histList)

	outputTabs := container.NewAppTabs(
		container.NewTabItem("Results", resultsStack),
		container.NewTabItem("Messages", messagesContent),
		container.NewTabItem("History", histContent),
	)
	outputTabs.SetTabLocation(container.TabLocationTop)

	outputTabs.OnChanged = func(tab *container.TabItem) {
		switch tab.Text {
		case "Results":
			state.SetOutputTab("results")
		case "Messages":
			state.SetOutputTab("messages")
		case "History":
			state.SetOutputTab("history")
			go func() {
				activeTab := state.ActiveTab()
				if activeTab == nil {
					return
				}
				records, err := state.Svc.GetQueryHistory(50)
				if err != nil {
					return
				}
				filtered := make([]history.QueryRecord, 0, len(records))
				for _, r := range records {
					if r.ConnID == activeTab.ConnID {
						filtered = append(filtered, r)
					}
				}
				histRecords = filtered
				histList.Refresh()
			}()
		}
	}

	// onRefreshGrid is called whenever the active tab result changes.
	state.onRefreshGrid = func() {
		tab := state.ActiveTab()
		if tab == nil || tab.Result == nil {
			emptyLabel.SetText("Run a query to see results")
			resultsStack.Objects = []fyne.CanvasObject{emptyLabel}
			resultsStack.Refresh()
			header.rebuild()
			infoLabel.SetText("")
			return
		}
		result := tab.Result

		if result.Error != "" {
			errorLabel.SetText("Error: " + result.Error)
			resultsStack.Objects = []fyne.CanvasObject{errorLabel}
			resultsStack.Refresh()
			msgLabel.Importance = widget.DangerImportance
			msgLabel.SetText("Error: " + result.Error)
			return
		}

		// Non-SELECT statement (INSERT / UPDATE / DELETE / DDL).
		if len(result.Columns) == 0 && result.RowsAffected > 0 {
			msg := fmt.Sprintf("Query OK \u2014 %d row(s) affected in %dms",
				result.RowsAffected, result.DurationMs)
			emptyLabel.SetText(msg)
			resultsStack.Objects = []fyne.CanvasObject{emptyLabel}
			resultsStack.Refresh()
			msgLabel.Importance = widget.SuccessImportance
			msgLabel.SetText(msg)
			return
		}

		// SELECT result: show virtual table.
		grid.setResult(result)
		header.rebuild()
		infoLabel.SetText(fmt.Sprintf("%d rows \u00b7 %dms",
			len(result.Rows), result.DurationMs))
		resultsStack.Objects = []fyne.CanvasObject{gridArea}
		resultsStack.Refresh()

		msgLabel.Importance = widget.SuccessImportance
		msgLabel.SetText(fmt.Sprintf("Query OK \u2014 %d rows in %dms",
			len(result.Rows), result.DurationMs))
	}

	// onRefreshOutput switches the visible tab to match state.outputTab.
	state.onRefreshOutput = func() {
		switch state.GetOutputTab() {
		case "results":
			outputTabs.SelectIndex(0)
		case "messages":
			outputTabs.SelectIndex(1)
		case "history":
			outputTabs.SelectIndex(2)
		}
	}

	return outputTabs
}
