// A static server for the harness.
//
// The interface fetches its locale files at start-up, and Chromium refuses the
// Fetch API on a `file://` page whatever flags it is given. Serving `ui/` over
// HTTP is also closer to the real thing: inside the app the front end is served
// over Tauri's own protocol, not off the disk.

const fs = require("fs");
const http = require("http");
const path = require("path");

const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".woff2": "font/woff2",
  ".jpg": "image/jpeg",
  ".png": "image/png",
};

/// Serves `root` on a free port, plus any single files named in `extras`.
///
/// `extras` maps a URL path to a file anywhere on disk: the harness's sample
/// frame lives under `target/`, outside the interface folder, and a page served
/// over HTTP cannot reach it through `file://`.
function serve(root, extras = {}) {
  const server = http.createServer((request, response) => {
    const asked = decodeURIComponent(new URL(request.url, "http://x").pathname);
    const file = extras[asked] ?? path.join(root, asked === "/" ? "index.html" : asked);
    if (extras[asked]) {
      fs.readFile(file, (error, body) => {
        if (error) return void response.writeHead(404).end();
        response.writeHead(200, { "Content-Type": TYPES[path.extname(file)] ?? "application/octet-stream" });
        response.end(body);
      });
      return;
    }
    // Nothing outside the folder, however the path is written.
    if (!path.resolve(file).startsWith(path.resolve(root))) {
      response.writeHead(403).end();
      return;
    }
    fs.readFile(file, (error, body) => {
      if (error) {
        response.writeHead(404).end();
        return;
      }
      response.writeHead(200, { "Content-Type": TYPES[path.extname(file)] ?? "application/octet-stream" });
      response.end(body);
    });
  });

  return new Promise((resolve) => {
    server.listen(0, "127.0.0.1", () => {
      const { port } = server.address();
      resolve({
        url: `http://127.0.0.1:${port}`,
        close: () => new Promise((done) => server.close(done)),
      });
    });
  });
}

module.exports = { serve };
