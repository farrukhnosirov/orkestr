// Tiny HTTP service of the Phase 7 acceptance run (deployed as a LocalProcess).
//   GET /        → 200 "greeter service" (deploy health URL)
//   GET /health  → 200 {status, greeting, version}; 500 + stack trace when the
//                  greeter throws (the stack is also written to stderr, the
//                  way a log shipper would pick it up).
import http from "node:http";

import { greeter } from "./greeter.mjs";

const port = Number(process.env.PORT || 8080);
const version = process.env.APP_VERSION || "dev";

http
  .createServer((req, res) => {
    if (req.url === "/health") {
      try {
        const body = JSON.stringify({ status: "ok", greeting: greeter(), version });
        res.writeHead(200, { "content-type": "application/json" });
        res.end(body);
      } catch (err) {
        console.error(`GET /health failed: ${err.stack}`);
        res.writeHead(500, { "content-type": "text/plain" });
        res.end(`GET /health failed: ${err.stack}`);
      }
      return;
    }
    res.writeHead(200, { "content-type": "text/plain" });
    res.end("greeter service\n");
  })
  .listen(port, "127.0.0.1", () => console.log(`greeter service listening on 127.0.0.1:${port}`));
