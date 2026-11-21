// SPDX-License-Identifier: MIT OR Apache-2.0
// Dev proxy: serve index.html and ui.js from disk, proxy everything else to
// the Rust host. This keeps JS/CSS edits live without rebuilding the Rust
// binary just to re-embed two static files.
//
//   node crates/soil-ui-proto/web/dev-proxy.mjs
//   open http://127.0.0.1:7879/

import http from "node:http";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, "../../..");
const upstream = process.env.SOIL_UI_UPSTREAM ?? "http://127.0.0.1:7878";
const port = Number(process.env.SOIL_UI_PORT ?? 7879);

const hotClients = new Set();
const webFiles = new Set([path.join(here, "index.html"), path.join(here, "ui.js")]);
const soilDirs = [
  path.join(repoRoot, "examples/ui"),
  path.join(repoRoot, "sites/soil-lang"),
  path.join(repoRoot, "crates/soil-lang/stdlib"),
];

function walkSoil(directory, into) {
  let entries;
  try {
    entries = fs.readdirSync(directory, { withFileTypes: true });
  } catch {
    return;
  }
  for (const entry of entries) {
    const file = path.join(directory, entry.name);
    if (entry.isDirectory()) walkSoil(file, into);
    else if (entry.name.endsWith(".soil")) {
      try {
        into.set(file, fs.statSync(file).mtimeMs);
      } catch {}
    }
  }
}

function snapshot() {
  const changes = new Map();
  for (const file of webFiles) {
    try {
      changes.set(file, fs.statSync(file).mtimeMs);
    } catch {}
  }
  for (const directory of soilDirs) walkSoil(directory, changes);
  return changes;
}

let lastSnapshot = snapshot();
setInterval(() => {
  const next = snapshot();
  const changed = [];
  for (const [file, mtime] of next) {
    if (lastSnapshot.get(file) !== mtime) changed.push(file);
  }
  for (const file of lastSnapshot.keys()) {
    if (!next.has(file)) changed.push(file);
  }
  if (!changed.length) return;
  lastSnapshot = next;
  const reload = changed.some(
    (file) => file.endsWith("index.html") || file.endsWith("ui.js"),
  );
  const event = reload ? "reload" : "source";
  for (const client of hotClients) {
    client.write(`event: ${event}\ndata: {}\n\n`);
  }
}, 400);

function asset(name, type, response) {
  const body = fs.readFileSync(path.join(here, name));
  response.writeHead(200, {
    "content-type": type,
    "content-length": body.length,
    "cache-control": "no-store",
  });
  response.end(body);
}

const server = http.createServer(async (request, response) => {
  const pathname = new URL(request.url, upstream).pathname;
  if (pathname === "/") return asset("index.html", "text/html; charset=utf-8", response);
  if (pathname === "/ui.js") return asset("ui.js", "text/javascript; charset=utf-8", response);
  if (pathname.startsWith("/embed/")) {
    return asset("index.html", "text/html; charset=utf-8", response);
  }
  if (pathname === "/__hot") {
    response.writeHead(200, { "content-type": "text/plain", "cache-control": "no-store" });
    response.end("ok");
    return;
  }
  if (pathname === "/__reload") {
    response.writeHead(200, {
      "content-type": "text/event-stream",
      "cache-control": "no-store",
      "connection": "keep-alive",
    });
    response.write(": connected\n\n");
    hotClients.add(response);
    request.on("close", () => hotClients.delete(response));
    return;
  }

  // Static example pages used by the inline workbench. `/examples` without a
  // file is the API listing, so only intercept paths with a trailing file.
  if (pathname.startsWith("/examples/")) {
    const file = path.join(here, "../../../sites/soil-lang/dist", pathname);
    if (fs.existsSync(file) && fs.statSync(file).isFile()) {
      response.writeHead(200, {
        "content-type": "text/html; charset=utf-8",
        "cache-control": "no-store",
      });
      response.end(fs.readFileSync(file));
      return;
    }
  }

  const chunks = [];
  for await (const chunk of request) chunks.push(chunk);
  const body = chunks.length ? Buffer.concat(chunks) : undefined;
  const target = new URL(request.url, upstream);
  const fetched = await fetch(target, {
    method: request.method,
    headers: {
      "content-type": request.headers["content-type"] ?? "application/octet-stream",
    },
    body: request.method === "GET" || request.method === "HEAD" ? undefined : body,
  });
  response.writeHead(fetched.status, {
    "content-type": fetched.headers.get("content-type") ?? "application/octet-stream",
    "cache-control": "no-store",
  });
  response.end(Buffer.from(await fetched.arrayBuffer()));
});

server.listen(port, "127.0.0.1", () => {
  console.log(`soil-ui dev proxy: http://127.0.0.1:${port}/ -> ${upstream}`);
});
