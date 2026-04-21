package ui

import (
	"fmt"
	"strconv"

	"fyne.io/fyne/v2"
	"fyne.io/fyne/v2/container"
	"fyne.io/fyne/v2/dialog"
	"fyne.io/fyne/v2/layout"
	"fyne.io/fyne/v2/widget"
	"multidb/backend/connections"
)

// showConnectionDialog opens a modal dialog for creating or editing a connection.
// If cfg is nil, it shows a "New Connection" form; otherwise an "Edit" form.
func showConnectionDialog(state *AppState, cfg *connections.ConnectionConfig) {
	isNew := cfg == nil
	title := "New Connection"
	if !isNew {
		title = "Edit Connection"
	}

	// Form state
	var form connections.ConnectionConfig
	if isNew {
		form = connections.ConnectionConfig{
			ID:     fmt.Sprintf("conn-%d", len(state.GetConns())+1),
			Driver: "mysql",
			Host:   "localhost",
			Port:   3306,
		}
	} else {
		form = *cfg
	}

	// Widgets
	nameEntry := widget.NewEntry()
	nameEntry.SetText(form.Name)
	nameEntry.SetPlaceHolder("My Database")

	driverSelect := widget.NewSelect([]string{"mysql", "postgres", "sqlite"}, nil)
	driverSelect.SetSelected(form.Driver)

	hostEntry := widget.NewEntry()
	hostEntry.SetText(form.Host)
	hostEntry.SetPlaceHolder("localhost")

	portEntry := widget.NewEntry()
	portEntry.SetText(fmt.Sprintf("%d", form.Port))

	userEntry := widget.NewEntry()
	userEntry.SetText(form.Username)

	passEntry := widget.NewPasswordEntry()
	passEntry.SetText(form.Password)

	dbEntry := widget.NewEntry()
	dbEntry.SetText(form.Database)
	dbEntry.SetPlaceHolder("my_database")

	dsnEntry := widget.NewEntry()
	dsnEntry.SetText(form.DSN)
	dsnEntry.SetPlaceHolder("Optional DSN override")

	// Kubernetes fields
	kubeCheck := widget.NewCheck("Use Kubernetes port-forward", nil)
	kubeCheck.SetChecked(form.UseKubePortForward)

	kubeContextEntry := widget.NewEntry()
	kubeContextEntry.SetText(form.KubeContext)
	kubeContextEntry.SetPlaceHolder("my-cluster")

	kubeNsEntry := widget.NewEntry()
	kubeNsEntry.SetText(form.KubeNamespace)
	kubeNsEntry.SetPlaceHolder("default")

	kubeResEntry := widget.NewEntry()
	kubeResEntry.SetText(form.KubeResource)
	kubeResEntry.SetPlaceHolder("service/postgres")

	kubeLocalPortEntry := widget.NewEntry()
	kubeLocalPortEntry.SetText(fmt.Sprintf("%d", form.KubeLocalPort))

	kubeRemotePortEntry := widget.NewEntry()
	kubeRemotePortEntry.SetText(fmt.Sprintf("%d", form.KubeRemotePort))

	kubeSection := container.NewVBox(
		widget.NewSeparator(),
		widget.NewLabel("Kubernetes Port-Forward"),
		container.New(layout.NewFormLayout(),
			widget.NewLabel("Context"), kubeContextEntry,
			widget.NewLabel("Namespace"), kubeNsEntry,
			widget.NewLabel("Target"), kubeResEntry,
			widget.NewLabel("Local Port"), kubeLocalPortEntry,
			widget.NewLabel("Remote Port"), kubeRemotePortEntry,
		),
	)
	if !form.UseKubePortForward {
		kubeSection.Hide()
	}

	kubeCheck.OnChanged = func(checked bool) {
		if checked {
			kubeSection.Show()
		} else {
			kubeSection.Hide()
		}
	}

	driverSelect.OnChanged = func(d string) {
		switch d {
		case "mysql":
			portEntry.SetText("3306")
		case "postgres":
			portEntry.SetText("5432")
		case "sqlite":
			portEntry.SetText("0")
			hostEntry.SetText("")
		}
	}

	statusLabel := widget.NewLabel("")

	// Collect form into config
	collectForm := func() connections.ConnectionConfig {
		port, _ := strconv.Atoi(portEntry.Text)
		lport, _ := strconv.Atoi(kubeLocalPortEntry.Text)
		rport, _ := strconv.Atoi(kubeRemotePortEntry.Text)
		return connections.ConnectionConfig{
			ID:                 form.ID,
			Name:               nameEntry.Text,
			Driver:             driverSelect.Selected,
			Host:               hostEntry.Text,
			Port:               port,
			Username:           userEntry.Text,
			Password:           passEntry.Text,
			Database:           dbEntry.Text,
			DSN:                dsnEntry.Text,
			UseKubePortForward: kubeCheck.Checked,
			KubeContext:        kubeContextEntry.Text,
			KubeNamespace:      kubeNsEntry.Text,
			KubeResource:       kubeResEntry.Text,
			KubeLocalPort:      lport,
			KubeRemotePort:     rport,
		}
	}

	testBtn := widget.NewButton("Test Connection", func() {
		statusLabel.SetText("Testing…")
		c := collectForm()
		go func() {
			err := state.Svc.TestConnection(c)
			if err != nil {
				statusLabel.SetText("Error: " + err.Error())
			} else {
				statusLabel.SetText("Connection successful!")
			}
		}()
	})

	// SQLite: hide host/port/user/pass fields
	hostLabel := widget.NewLabel("Host")
	portLabel := widget.NewLabel("Port")
	userLabel := widget.NewLabel("Username")
	passLabel := widget.NewLabel("Password")

	showHideForDriver := func(d string) {
		if d == "sqlite" {
			hostEntry.Hide(); hostLabel.Hide()
			portEntry.Hide(); portLabel.Hide()
			userEntry.Hide(); userLabel.Hide()
			passEntry.Hide(); passLabel.Hide()
			dbEntry.SetPlaceHolder("/path/to/file.db")
		} else {
			hostEntry.Show(); hostLabel.Show()
			portEntry.Show(); portLabel.Show()
			userEntry.Show(); userLabel.Show()
			passEntry.Show(); passLabel.Show()
			dbEntry.SetPlaceHolder("my_database")
		}
	}
	showHideForDriver(form.Driver)

	driverSelect.OnChanged = func(d string) {
		switch d {
		case "mysql":
			portEntry.SetText("3306")
		case "postgres":
			portEntry.SetText("5432")
		case "sqlite":
			portEntry.SetText("0")
		}
		showHideForDriver(d)
		if kubeCheck.Checked {
			kubeSection.Show()
		} else {
			kubeSection.Hide()
		}
	}

	formContent := container.NewVBox(
		container.New(layout.NewFormLayout(),
			widget.NewLabel("Name"), nameEntry,
			widget.NewLabel("Driver"), driverSelect,
			hostLabel, hostEntry,
			portLabel, portEntry,
			userLabel, userEntry,
			passLabel, passEntry,
			widget.NewLabel("Database"), dbEntry,
			widget.NewLabel("DSN"), dsnEntry,
		),
		widget.NewSeparator(),
		kubeCheck,
		kubeSection,
		widget.NewSeparator(),
		testBtn,
		statusLabel,
	)

	scroll := container.NewVScroll(formContent)
	scroll.SetMinSize(fyne.NewSize(440, 380))

	var dlg dialog.Dialog
	saveBtn := widget.NewButton("Save & Connect", func() {
		c := collectForm()
		if c.Name == "" {
			statusLabel.SetText("Name is required")
			return
		}
		statusLabel.SetText("Connecting…")
		go func() {
			err := state.Svc.SaveAndConnect(c)
			if err != nil {
				statusLabel.SetText("Error: " + err.Error())
				return
			}
			state.UpsertConn(&ActiveConn{Config: c})
			state.SetStatus("Connected to " + c.Name)
			// Load schema in background
			state.LoadSchemaForConn(c.ID)
			dlg.Hide()
		}()
	})
	cancelBtn := widget.NewButton("Cancel", func() { dlg.Hide() })

	buttons := container.NewHBox(layout.NewSpacer(), cancelBtn, saveBtn)
	content := container.NewBorder(nil, buttons, nil, nil, scroll)

	dlg = dialog.NewCustomWithoutButtons(title, content, state.Window)
	dlg.Resize(fyne.NewSize(480, 520))
	dlg.Show()
}
