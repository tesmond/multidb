# README

## About

This is the official Wails Svelte-TS template.

## Live Development

To run in live development mode, run `wails dev` in the project directory. This will run a Vite development
server that will provide very fast hot reload of your frontend changes. If you want to develop in a browser
and have access to your Go methods, there is also a dev server that runs on http://localhost:34115. Connect
to this in your browser, and you can call your Go code from devtools.

## Building

To build a redistributable, production mode package, use `wails build`.

## App Icons

This project uses committed application icons for desktop builds:

- macOS icon: `resources/appicon.icns`
- Windows icon: `resources/appicon.ico`

The Wails config points at the committed macOS icon in `wails.json`. The Windows `.ico` file is also stored in `resources/` so it can be used during Windows packaging without relying on generated files in `build/`.

If you update the source image (`frontend/src/assets/images/capy.png`), regenerate and replace the committed icon files in `resources/` before building release artifacts.
