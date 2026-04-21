package ui

import (
	"fyne.io/fyne/v2"
	"fyne.io/fyne/v2/canvas"
	"fyne.io/fyne/v2/theme"
	"fyne.io/fyne/v2/widget"
)

// StatusBar is a simple status label at the bottom of the window.
type StatusBar struct {
	widget.BaseWidget
	label *widget.Label
	state *AppState
}

func NewStatusBar(state *AppState) *StatusBar {
	sb := &StatusBar{
		label: widget.NewLabel("Ready"),
		state: state,
	}
	sb.ExtendBaseWidget(sb)

	state.onRefreshStatus = func() {
		sb.label.SetText(state.GetStatus())
	}
	return sb
}

func (sb *StatusBar) CreateRenderer() fyne.WidgetRenderer {
	bg := canvas.NewRectangle(theme.MenuBackgroundColor())
	return &statusBarRenderer{bg: bg, label: sb.label, sb: sb}
}

type statusBarRenderer struct {
	bg    *canvas.Rectangle
	label *widget.Label
	sb    *StatusBar
}

func (r *statusBarRenderer) Layout(size fyne.Size) {
	r.bg.Resize(size)
	r.label.Move(fyne.NewPos(theme.InnerPadding(), theme.InnerPadding()/2))
	r.label.Resize(fyne.NewSize(size.Width-theme.InnerPadding()*2, size.Height-theme.InnerPadding()))
}

func (r *statusBarRenderer) MinSize() fyne.Size {
	return fyne.NewSize(200, r.label.MinSize().Height+theme.InnerPadding())
}

func (r *statusBarRenderer) Refresh() {
	r.bg.FillColor = theme.MenuBackgroundColor()
	r.bg.Refresh()
	r.label.Refresh()
}

func (r *statusBarRenderer) Destroy() {}

func (r *statusBarRenderer) Objects() []fyne.CanvasObject {
	return []fyne.CanvasObject{r.bg, r.label}
}
