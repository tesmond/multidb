# README

## About

multidb is a native desktop SQL client built with Go and Fyne.

It supports:

- Multiple saved database connections (PostgreSQL, MySQL, SQLite)
- SQL editor with schema-aware completion
- Streaming query execution with cancellation
- Virtualized results grid for large result sets
- Schema caching and refresh

## Prerequisites

- Go 1.25+

For Linux desktop packaging/runtime, install Fyne system dependencies as documented at:
https://docs.fyne.io/started/

## Live Development

From the repository root:

```bash
go run .
```

## Building

### Build native binary

```bash
mkdir -p build/bin
go build -o build/bin/multidb .
```

### Cross-compile

```bash
# macOS arm64
GOOS=darwin GOARCH=arm64 go build -o build/bin/multidb-macos-arm64 .

# Linux amd64
GOOS=linux GOARCH=amd64 go build -o build/bin/multidb-linux-amd64 .

# Windows amd64
GOOS=windows GOARCH=amd64 go build -o build/bin/multidb-windows-amd64.exe .
```

### Optional app packaging with Fyne

Install the Fyne CLI:

```bash
go install fyne.io/tools/cmd/fyne@latest
```

Then package from the repository root (example for macOS app bundle):

```bash
fyne package --name multidb --icon assets/icons/appicon.png
```

## Testing

```bash
go test ./...
```

## App Icons

Committed icons are stored in `assets/icons/`:

- PNG source icon: `assets/icons/appicon.png`
- macOS icon: `assets/icons/appicon.icns`
- Source artwork: `assets/icons/capy.png`

If you update the icon artwork, regenerate the platform-specific icon files before release packaging.
