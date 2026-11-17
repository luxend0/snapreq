// Example plugin. Plugins are plain JavaScript files in .snapreq/plugins/ of the project folder.
// They run in a sandbox: no files, no network, 2 seconds and 32 MB at most.

// Called before a request is sent. Change req (method, url, headers, body) as you like.
function onRequest(req) {
  req.headers["X-Request-Id"] = Date.now().toString(36) + Math.random().toString(36).slice(2, 8);
}

// Called with the response. A returned text is shown as a note next to the search field.
function onResponse(req, res) {
  if (res.status >= 500) return "Server error " + res.status + " after " + res.ms + " ms";
  snapreq.log(req.method, req.url, "->", res.status);
}
