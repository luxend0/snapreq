# Snapreq

A fast, lightweight API client for Linux, written in Rust. A replacement for Postman without Electron, accounts or cloud.

- Requests are plain `.http` files in your project folder (the format used by VS Code REST Client and JetBrains), so they diff well and live in Git.
- Environments come from `http-client.env.json` (and `http-client.private.env.json` for secrets).
- Window and terminal in one binary.

## Install

Linux and macOS (Apple Silicon), for the current user, no root:

```sh
curl -fsSL https://raw.githubusercontent.com/luxend0/snapreq/main/install.sh | sh
```

Windows (PowerShell):

```powershell
irm https://raw.githubusercontent.com/luxend0/snapreq/main/install.ps1 | iex
```

Arch Linux: `packaging/PKGBUILD` builds `snapreq-git`.

Developed and tested on Linux. The Windows and macOS builds compile and pass the tests in CI but have not been tried by hand yet.

From source: `cargo build --release`.

## Use

```sh
snapreq examples                 # window for a folder
snapreq run examples/demo.http   # terminal / CI
```

The moving backdrop can be switched off with the "Motion" button or `SNAPREQ_STATIC=1`.

Shortcuts: Ctrl+Enter send, Ctrl+S save, Ctrl+N new request, Ctrl+L focus URL.

Planned: auth helpers, import (Postman, cURL, OpenAPI, Bruno, Insomnia), WebSocket and SSE, scripts and tests.

License: GPL-3.0-only.
