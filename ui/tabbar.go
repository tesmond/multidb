package ui

import (
	"image/color"

	"fyne.io/fyne/v2"
	"fyne.io/fyne/v2/canvas"
	"fyne.io/fyne/v2/container"
	"fyne.io/fyne/v2/dialog"
	"fyne.io/fyne/v2/driver/desktop"
	"fyne.io/fyne/v2/theme"
	"fyne.io/fyne/v2/widget"
)

// tabItemWidget is a clickable tab button with right-click context menu support.
type tabItemWidget struct {
	widget.BaseWidget
	tabID string
	state *AppState
}

func newTabItemWidget(tabID string, state *AppState) *tabItemWidget {
	w := &tabItemWidget{tabID: tabID, state: state}
	w.ExtendBaseWidget(w)
	return w
}

func (w *tabItemWidget) CreateRenderer() fyne.WidgetRenderer {
	title := "Query"
	running := false
	active := false
	if tab := w.state.GetTab(w.tabID); tab != nil {
		title = tab.Title
		running = tab.Running
		active = w.state.GetActiveTabID() == w.tabID
	}

	label := widget.NewLabel(title)
	if running {
		label.SetText("⟳ " + title)
	}

	closeBtn := widget.NewButtonWithIcon("", theme.WindowCloseIcon(), func() {
		w.state.CloseTab(w.tabID)
	})
	closeBtn.Importance = widget.LowImportance

	bg := canvas.NewRectangle(color.RGBA{R: 80, G: 80, B: 80, A: 40})
	if active {
		bg.FillColor = color.RGBA{R: 70, G: 130, B: 180, A: 100}
	}
	bg.CornerRadius = 4

	row := container.NewHBox(label, closeBtn)
	padded := container.NewPadded(row)

	return &tabItemRenderer{w: w, bg: bg, label: label, padded: padded}
}

type tabItemRenderer struct {
	w      *tabItemWidget
	bg     *canvas.Rectangle
	label  *widget.Label
	padded *fyne.Container
}

func (r *tabItemRenderer) Layout(size fyne.Size) {
	r.bg.Resize(size)
	r.padded.Resize(size)
	r.padded.Move(fyne.NewPos(0, 0))
}

func (r *tabItemRenderer) MinSize() fyne.Size {
	ms := r.padded.MinSize()
	return fyne.NewSize(ms.Width+4, ms.Height)
}

func (r *tabItemRenderer) Refresh() {
	tab := r.w.state.GetTab(r.w.tabID)
	active := r.w.state.GetActiveTabID() == r.w.tabID
	if tab != nil {
		title := tab.Title
		if tab.Running {
			title = "⟳ " + title
		}
		r.label.SetText(title)
	}
	if active {
		r.bg.FillColor = color.RGBA{R: 70, G: 130, B: 180, A: 100}
	} else {
		r.bg.FillColor = color.RGBA{R: 80, G: 80, B: 80, A: 40}
	}
	r.bg.Refresh()
	r.padded.Refresh()
}

func (r *tabItemRenderer) Destroy() {}
func (r *tabItemRenderer) Objects() []fyne.CanvasObject {
	return []fyne.CanvasObject{r.bg, r.padded}
}

// Tapped handles left-click: activate this tab.
func (w *tabItemWidget) Tapped(*fyne.PointEvent) {
	w.state.SetActiveTab(w.tabID)
}

// TappedSecondary handles right-click.
func (w *tabItemWidget) TappedSecondary(e *fyne.PointEvent) {
	w.showContextMenu(e.AbsolutePosition)
}

// MouseDown handles mouse button events (implements desktop.Mouseable).
func (w *tabItemWidget) MouseDown(e *desktop.MouseEvent) {
	if e.Button == desktop.MouseButtonSecondary {
		w.showContextMenu(e.AbsolutePosition)
	} else {
		w.state.SetActiveTab(w.tabID)
	}
}

func (w *tabItemWidget) MouseUp(*desktop.MouseEvent) {}

// tabbarWindow returns the first available application window for parenting dialogs.
func tabbarWindow() fyne.Window {
	if wins := fyne.CurrentApp().Driver().AllWindows(); len(wins) > 0 {
		return wins[0]
	}
	return nil
}

func (w *tabItemWidget) showContextMenu(pos fyne.Position) {
	items := []*fyne.MenuItem{
		fyne.NewMenuItem("Move Left", func() {
			w.state.MoveTabLeft(w.tabID)
		}),
		fyne.NewMenuItem("Move Right", func() {
			w.state.MoveTabRight(w.tabID)
		}),
		fyne.NewMenuItemSeparator(),
		fyne.NewMenuItem("Rename", func() {
			entry := widget.NewEntry()
			if tab := w.state.GetTab(w.tabID); tab != nil {
				entry.SetText(tab.Title)
			}
			win := tabbarWindow()
			if win == nil {
				return
			}
			dlg := dialog.NewCustomConfirm("Rename Tab", "Rename", "Cancel", entry,
				func(ok bool) {
					if ok && entry.Text != "" {
						w.state.RenameTab(w.tabID, entry.Text)
					}
				}, win)
			dlg.Show()
		}),
		fyne.NewMenuItem("Duplicate", func() {
			w.state.DuplicateTab(w.tabID)
		}),
		fyne.NewMenuItemSeparator(),
		fyne.NewMenuItem("Close Others", func() {
			w.state.CloseOtherTabs(w.tabID)
		}),
		fyne.NewMenuItem("Close to the Right", func() {
			w.state.CloseTabsRight(w.tabID)
		}),
		fyne.NewMenuItem("Close to the Left", func() {
			w.state.CloseTabsLeft(w.tabID)
		}),
	}
	menu := fyne.NewMenu("", items...)
	widget.ShowPopUpMenuAtPosition(menu, fyne.CurrentApp().Driver().CanvasForObject(w), pos)
}

// NewTabBar creates and returns the horizontal scrollable tab bar.
func NewTabBar(state *AppState) fyne.CanvasObject {
	hbox := container.NewHBox()
	scroll := container.NewHScroll(hbox)
	scroll.SetMinSize(fyne.NewSize(100, 40))

	addBtn := widget.NewButtonWithIcon("", theme.ContentAddIcon(), func() {
		connID := ""
		if tab := state.ActiveTab(); tab != nil {
			connID = tab.ConnID
		}
		state.AddTab(connID)
	})
	addBtn.Importance = widget.LowImportance

	rebuild := func() {
		tabs := state.GetTabs()
		objs := make([]fyne.CanvasObject, len(tabs))
		for i, t := range tabs {
			objs[i] = newTabItemWidget(t.ID, state)
		}
		hbox.Objects = objs
		hbox.Refresh()
		scroll.Refresh()
	}

	state.onRefreshTabs = rebuild
	rebuild()

	return container.NewBorder(nil, nil, nil, addBtn, scroll)
}
