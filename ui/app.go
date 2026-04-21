package ui

import (
	"fyne.io/fyne/v2"
	"fyne.io/fyne/v2/app"
	"fyne.io/fyne/v2/container"
	"fyne.io/fyne/v2/driver/desktop"
	"fyne.io/fyne/v2/widget"
	"multidb/backend/service"
)

// Run initialises the Fyne application, builds the UI, and starts the event loop.
// This function blocks until the window is closed.
func Run() {
	a := app.NewWithID("io.multidb.app")

	w := a.NewWindow("multidb")
	w.Resize(fyne.NewSize(1280, 800))
	w.SetMaster()

	svc, err := service.New()
	if err != nil {
		lbl := widget.NewLabel("Failed to initialise backend: " + err.Error())
		lbl.Wrapping = fyne.TextWrapWord
		w.SetContent(container.NewCenter(lbl))
		w.ShowAndRun()
		return
	}
	defer svc.Close()

	state := NewAppState(svc)
	state.Window = w

	// results.go exposes a package-level MainWindow for clipboard operations.
	MainWindow = w

	// Build UI components.
	nav     := NewNavigator(state)
	tabBar  := NewTabBar(state)
	editor  := NewEditorPane(state)
	results := NewResultsPane(state)
	statusBar := NewStatusBar(state)

	// Editor + Results: vertical split (editor top, results bottom).
	editorResults := container.NewVSplit(editor, results)
	editorResults.Offset = 0.35

	// Right side: tab bar pinned at top, editor/results below.
	rightSide := container.NewBorder(tabBar, nil, nil, nil, editorResults)

	// Navigator (left) + right side: horizontal split.
	mainSplit := container.NewHSplit(nav, rightSide)
	mainSplit.Offset = 0.20

	// Full layout: status bar pinned at bottom.
	content := container.NewBorder(nil, statusBar, nil, nil, mainSplit)
	w.SetContent(content)

	// Ctrl/Cmd+Enter: run or cancel the active query.
	runShortcut := func(_ fyne.Shortcut) {
		if tab := state.ActiveTab(); tab != nil {
			if tab.Running {
				state.CancelQuery(tab.ID)
			} else {
				state.RunQuery(tab.ID)
			}
		}
	}
	w.Canvas().AddShortcut(&desktop.CustomShortcut{
		KeyName:  fyne.KeyReturn,
		Modifier: fyne.KeyModifierControl,
	}, runShortcut)
	w.Canvas().AddShortcut(&desktop.CustomShortcut{
		KeyName:  fyne.KeyReturn,
		Modifier: fyne.KeyModifierSuper,
	}, runShortcut)

	// Ctrl/Cmd+T: new tab.
	newTabShortcut := func(_ fyne.Shortcut) {
		connID := ""
		if tab := state.ActiveTab(); tab != nil {
			connID = tab.ConnID
		}
		state.AddTab(connID)
	}
	w.Canvas().AddShortcut(&desktop.CustomShortcut{
		KeyName:  fyne.KeyT,
		Modifier: fyne.KeyModifierControl,
	}, newTabShortcut)
	w.Canvas().AddShortcut(&desktop.CustomShortcut{
		KeyName:  fyne.KeyT,
		Modifier: fyne.KeyModifierSuper,
	}, newTabShortcut)

	// Pre-load cached schemas for every saved connection.
	go func() {
		for _, conn := range state.GetConns() {
			state.LoadSchemaForConn(conn.Config.ID)
		}
	}()

	state.SetStatus("Ready")
	w.ShowAndRun()
}
