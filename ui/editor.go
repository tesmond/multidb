package ui

import (
	"fmt"
	"strings"

	"fyne.io/fyne/v2"
	"fyne.io/fyne/v2/container"
	"fyne.io/fyne/v2/driver/desktop"
	"fyne.io/fyne/v2/widget"
)

// sqlEntry is a multiline Entry that intercepts Ctrl/Cmd+Space for completion.
type sqlEntry struct {
	widget.Entry
	state      *AppState
	tabID      string
	onComplete func(pos fyne.Position)
}

func newSQLEntry(state *AppState, tabID string) *sqlEntry {
	e := &sqlEntry{state: state, tabID: tabID}
	e.MultiLine = true
	e.Wrapping = fyne.TextWrapOff
	e.TextStyle = fyne.TextStyle{Monospace: true}
	e.ExtendBaseWidget(e)
	return e
}

// TypedShortcut intercepts Ctrl+Space / Cmd+Space for completion.
func (e *sqlEntry) TypedShortcut(s fyne.Shortcut) {
	if cs, ok := s.(*desktop.CustomShortcut); ok {
		isCtrlSpace := cs.KeyName == fyne.KeySpace &&
			(cs.Modifier == fyne.KeyModifierControl || cs.Modifier == fyne.KeyModifierSuper)
		if isCtrlSpace {
			if e.onComplete != nil {
				pos := fyne.CurrentApp().Driver().AbsolutePositionForObject(e)
				pos.Y += e.Size().Height * 0.7
				e.onComplete(pos)
			}
			return
		}
	}
	e.Entry.TypedShortcut(s)
}

// completionPopup shows a floating list of SQL completions.
type completionPopup struct {
	popup  *widget.PopUp
	list   *widget.List
	items  []CompletionItem
	entry  *sqlEntry
	canvas fyne.Canvas
}

func newCompletionPopup(cv fyne.Canvas, entry *sqlEntry) *completionPopup {
	cp := &completionPopup{canvas: cv, entry: entry}
	cp.list = widget.NewList(
		func() int { return len(cp.items) },
		func() fyne.CanvasObject {
			lbl := widget.NewLabel("")
			detail := widget.NewLabel("")
			detail.Importance = widget.LowImportance
			return container.NewHBox(lbl, detail)
		},
		func(id widget.ListItemID, o fyne.CanvasObject) {
			if id >= len(cp.items) {
				return
			}
			item := cp.items[id]
			box := o.(*fyne.Container)
			box.Objects[0].(*widget.Label).SetText(item.Label)
			d := item.Detail
			if d == "" {
				d = item.Type
			}
			box.Objects[1].(*widget.Label).SetText(" [" + d + "]")
		},
	)
	cp.list.OnSelected = func(id widget.ListItemID) {
		if id >= len(cp.items) {
			return
		}
		item := cp.items[id]
		current := entry.Text
		cursor := len(current) // approximate cursor at end
		wordStart := cursor
		for wordStart > 0 && isIdentRune(rune(current[wordStart-1])) {
			wordStart--
		}
		newText := current[:wordStart] + item.Label + current[cursor:]
		entry.SetText(newText)
		entry.state.UpdateTabSQL(entry.tabID, newText)
		if cp.popup != nil {
			cp.popup.Hide()
		}
	}
	return cp
}

func isIdentRune(r rune) bool {
	return (r >= 'a' && r <= 'z') || (r >= 'A' && r <= 'Z') ||
		(r >= '0' && r <= '9') || r == '_'
}

func (cp *completionPopup) show(items []CompletionItem, pos fyne.Position) {
	cp.items = items
	if cp.popup == nil {
		cp.popup = widget.NewPopUp(cp.list, cp.canvas)
	}
	h := float32(len(items)) * 28
	if h > 220 {
		h = 220
	}
	if h < 28 {
		h = 28
	}
	cp.list.Refresh()
	cp.popup.Resize(fyne.NewSize(320, h))
	cp.popup.ShowAtPosition(pos)
}

// NewEditorPane returns the SQL editor panel for the active tab.
// It rebuilds its content whenever the active tab changes.
func NewEditorPane(state *AppState) fyne.CanvasObject {
	inner := container.NewMax()
	var currentTabID string
	var currentEntry *sqlEntry
	var popup *completionPopup

	rebuild := func() {
		tab := state.ActiveTab()
		if tab == nil {
			return
		}
		if tab.ID == currentTabID && currentEntry != nil {
			// Refresh in-place: update run/stop and sync SQL text
			running := tab.Running
			inner.Refresh()
			_ = running
			return
		}
		currentTabID = tab.ID

		// --- Connection selector ---
		conns := state.GetConns()
		connNames := make([]string, 0, len(conns)+1)
		connIDs := make([]string, 0, len(conns)+1)
		connNames = append(connNames, "— select connection —")
		connIDs = append(connIDs, "")
		for _, c := range conns {
			connNames = append(connNames, c.Config.Name)
			connIDs = append(connIDs, c.Config.ID)
		}

		connSelect := widget.NewSelect(connNames, nil)
		selectedIdx := 0
		for i, id := range connIDs {
			if id == tab.ConnID {
				selectedIdx = i
				break
			}
		}
		connSelect.SetSelectedIndex(selectedIdx)
		tabID := tab.ID // capture for callbacks
		connSelect.OnChanged = func(_ string) {
			idx := connSelect.SelectedIndex()
			if idx >= 0 && idx < len(connIDs) {
				state.UpdateTabConnID(tabID, connIDs[idx])
			}
		}

		// --- Run / Stop button ---
		var runBtn *widget.Button
		runBtn = widget.NewButton("▶ Run", func() {
			t := state.GetTab(tabID)
			if t != nil && t.Running {
				state.CancelQuery(tabID)
				runBtn.SetText("▶ Run")
			} else {
				state.RunQuery(tabID)
				runBtn.SetText("⏹ Stop")
			}
		})
		if tab.Running {
			runBtn.SetText("⏹ Stop")
		}

		toolbar := container.NewHBox(connSelect, runBtn)

		// --- SQL Entry ---
		entry := newSQLEntry(state, tabID)
		entry.SetText(tab.SQL)
		entry.OnChanged = func(text string) {
			state.UpdateTabSQL(tabID, text)
		}

		// Completion popup (lazy init — needs canvas)
		entry.onComplete = func(pos fyne.Position) {
			t := state.GetTab(tabID)
			if t == nil {
				return
			}
			conn := state.GetConn(t.ConnID)
			dbSchema := BuildSchemaForCompletion(conn)
			items := GetCompletions(dbSchema, entry.Text, len(entry.Text))
			if len(items) == 0 {
				return
			}
			if popup == nil {
				cv := fyne.CurrentApp().Driver().CanvasForObject(entry)
				if cv != nil {
					popup = newCompletionPopup(cv, entry)
				}
			}
			if popup != nil {
				popup.show(items, pos)
			}
		}

		currentEntry = entry

		editorScroll := container.NewScroll(entry)
		editorScroll.Direction = container.ScrollBoth

		pane := container.NewBorder(toolbar, nil, nil, nil, editorScroll)
		inner.Objects = []fyne.CanvasObject{pane}
		inner.Refresh()
	}

	state.onRefreshEditor = func() {
		tab := state.ActiveTab()
		if tab == nil {
			return
		}
		if tab.ID != currentTabID {
			rebuild()
			return
		}
		// Sync SQL if changed externally (e.g. from Navigator "View Data")
		if currentEntry != nil && tab.SQL != currentEntry.Text {
			currentEntry.SetText(tab.SQL)
		}
		// Sync run/stop state — rebuild toolbar
		rebuild()
	}

	rebuild()
	return inner
}

// shortcutHint returns the platform-appropriate shortcut hint string.
func shortcutHint() string {
	if strings.Contains(strings.ToLower(fmt.Sprintf("%v", fyne.CurrentApp())), "darwin") {
		return "⌘⏎"
	}
	return "Ctrl+⏎"
}
