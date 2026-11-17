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
snapreq examples                           # window for a folder
snapreq run examples/demo.http             # terminal / CI
snapreq bench examples/demo.http -r 500 -c 20   # load test: 500 requests, 20 at once
```

- **Tabs**: several requests open side by side, each keeps its own response. A request you click opens in a new tab if the current one has unsaved edits.
- **History**: the sidebar's History tab lists the last 200 sent requests. They are stored in your user data folder with `{{variables}}` left as placeholders, so secrets from environments are not copied there.
- **Code snippets**: the details panel shows the current request as cURL, Python, JavaScript, Go or Rust.
- **Load test**: `snapreq bench`, or the button in the details panel. Reports requests per second, latency percentiles and status counts. Only point it at servers you are allowed to load.

The moving backdrop can be switched off with the "Motion" button or `SNAPREQ_STATIC=1`.

Shortcuts: Ctrl+Enter send, Ctrl+S save, Ctrl+N new request, Ctrl+T new tab, Ctrl+W close tab, Ctrl+L focus URL, Ctrl+K search.

## Plugins

A plugin is a JavaScript file in `.snapreq/plugins/` of the project folder. It can define two functions:

```js
function onRequest(req) {          // before sending; change method, url, headers, body
  req.headers["X-Request-Id"] = Date.now().toString(36);
}
function onResponse(req, res) {    // res: status, reason, headers, body, ms
  if (res.status >= 500) return "Server error after " + res.ms + " ms";   // shown as a note
  snapreq.log(req.url, res.status);
}
```

Plugins run for the window and for `snapreq run`. They are sandboxed (no files, no network, 2 seconds and 32 MB per call), and the engine only starts when a plugin file exists. See `examples/.snapreq/plugins/request-id.js`.

Planned: auth helpers, import (Postman, cURL, OpenAPI, Bruno, Insomnia), WebSocket and SSE, Postman-style test scripts.

License: GPL-3.0-only.
