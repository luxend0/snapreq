# Snapreq

A fast, lightweight API client for Linux, written in Rust. A replacement for Postman without Electron, accounts or cloud.

- Requests are plain `.http` files in your project folder (the format used by VS Code REST Client and JetBrains), so they diff well and live in Git.
- Environments come from `http-client.env.json` (and `http-client.private.env.json` for secrets).
- Window and terminal in one binary.

Measured on the release build: window in about 30 ms, 25 MB RAM.

```sh
cargo build --release
./target/release/snapreq examples              # window
./target/release/snapreq run examples/demo.http  # terminal / CI
```

Shortcuts: Ctrl+Enter send, Ctrl+S save, Ctrl+N new request, Ctrl+L focus URL.

Planned: auth helpers, import (Postman, cURL, OpenAPI, Bruno, Insomnia), WebSocket and SSE, scripts and tests.

License: GPL-3.0-only.
