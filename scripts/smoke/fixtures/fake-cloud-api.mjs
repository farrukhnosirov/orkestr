#!/usr/bin/env node
// Fake Vercel / Netlify / Fly Machines / Kubernetes APIs for the Phase 7
// deployment acceptance (ADR-0041, "request-shape verified").
//
// Usage:
//   node scripts/smoke/fixtures/fake-cloud-api.mjs [--port 0] [--token <bearer>]
// Prints one JSON line on stdout: {"port":N,"base":"http://127.0.0.1:N"}.
// Point the core at it with ORKESTR_VERCEL_API / ORKESTR_NETLIFY_API /
// ORKESTR_FLY_API / ORKESTR_KUBERNETES_API = base (all four share one server).
//
// Every request is recorded: GET /__requests returns
// [{method, path, authOk, shapeOk, problems, contentType, digest, bodyKeys}]
// (the token itself is never recorded). POST /__reset clears the log.
// A wrong / missing bearer token → 401; an invalid body shape → 400.

import http from "node:http";
import crypto from "node:crypto";

const args = process.argv.slice(2);
const opt = (name, dflt) => {
  const i = args.indexOf(`--${name}`);
  return i >= 0 && args[i + 1] !== undefined ? args[i + 1] : dflt;
};
const PORT = Number(opt("port", "0"));
const TOKEN = opt("token", process.env.FAKE_CLOUD_TOKEN || "");

const log = [];
const polls = new Map();
const vercel = new Map(); // id → {project, url}
const netlify = new Map(); // id → {site, files, uploaded:Set}
const fly = new Map([["machines", new Map()]]);
let k8sGeneration = 1;
let seq = 0;

const json = (res, status, body) => {
  res.writeHead(status, { "content-type": "application/json" });
  res.end(JSON.stringify(body));
};
const pollCount = (key) => {
  const n = (polls.get(key) || 0) + 1;
  polls.set(key, n);
  return n;
};
const isSha1 = (s) => typeof s === "string" && /^[0-9a-f]{40}$/.test(s);

function route(req, url, raw, body, problems) {
  const p = url.pathname;
  const m = req.method;
  let r;
  // ---------------------------------------------------------------- Vercel
  if (m === "POST" && p === "/v2/files") {
    const digest = req.headers["x-vercel-digest"];
    const actual = crypto.createHash("sha1").update(raw).digest("hex");
    if (digest !== actual) problems.push("x-vercel-digest does not match the body sha1");
    return [200, {}];
  }
  if (m === "POST" && p === "/v13/deployments") {
    if (typeof body?.name !== "string") problems.push("name missing");
    if (!Array.isArray(body?.files)) problems.push("files must be an array");
    for (const f of body?.files || []) {
      if (typeof f.file !== "string" || !isSha1(f.sha) || typeof f.size !== "number") problems.push(`bad file entry ${JSON.stringify(f)}`);
    }
    const id = `dpl_${++seq}`;
    vercel.set(id, { project: body?.name, url: `${body?.name || "app"}-${seq}.vercel.app` });
    return [200, { id, url: vercel.get(id).url, readyState: "QUEUED" }];
  }
  if ((r = p.match(/^\/v13\/deployments\/([^/]+)$/)) && m === "GET") {
    const d = vercel.get(r[1]);
    if (!d) return [404, { error: { code: "not_found" } }];
    const n = pollCount(p);
    return [200, { id: r[1], url: d.url, readyState: n < 2 ? "BUILDING" : "READY" }];
  }
  if ((r = p.match(/^\/v12\/deployments\/([^/]+)\/cancel$/)) && m === "PATCH") {
    return [200, { id: r[1], readyState: "CANCELED" }];
  }
  if ((r = p.match(/^\/v9\/projects\/([^/]+)\/rollback\/([^/]+)$/)) && m === "POST") {
    if (!vercel.has(r[2])) return [404, { error: { code: "not_found" } }];
    return [201, {}];
  }
  // ---------------------------------------------------------------- Netlify
  if ((r = p.match(/^\/api\/v1\/sites\/([^/]+)\/deploys$/)) && m === "POST") {
    const files = body?.files;
    if (!files || typeof files !== "object" || Array.isArray(files)) problems.push("files must be an object {path: sha1}");
    for (const [k, v] of Object.entries(files || {})) {
      if (!k.startsWith("/") || !isSha1(v)) problems.push(`bad digest entry ${k}`);
    }
    const id = `nd_${++seq}`;
    netlify.set(id, { site: r[1], files: files || {}, uploaded: new Set() });
    return [200, { id, state: "uploading", required: [...new Set(Object.values(files || {}))] }];
  }
  if ((r = p.match(/^\/api\/v1\/deploys\/([^/]+)\/files\/(.+)$/)) && m === "PUT") {
    const d = netlify.get(r[1]);
    if (!d) return [404, { message: "Not Found" }];
    if (!String(req.headers["content-type"] || "").startsWith("application/octet-stream")) problems.push("upload content-type must be application/octet-stream");
    const sha = crypto.createHash("sha1").update(raw).digest("hex");
    const path = "/" + decodeURIComponent(r[2]);
    if (d.files[path] !== sha) problems.push(`uploaded ${path} does not match its digest`);
    d.uploaded.add(sha);
    return [200, { id: sha }];
  }
  if ((r = p.match(/^\/api\/v1\/deploys\/([^/]+)$/)) && m === "GET") {
    const d = netlify.get(r[1]);
    if (!d) return [404, { message: "Not Found" }];
    const n = pollCount(p);
    return [200, { id: r[1], state: n < 2 ? "processing" : "ready", ssl_url: `https://${d.site}.netlify.app` }];
  }
  if ((r = p.match(/^\/api\/v1\/deploys\/([^/]+)\/cancel$/)) && m === "POST") return [200, { id: r[1], state: "error" }];
  if ((r = p.match(/^\/api\/v1\/sites\/([^/]+)\/deploys\/([^/]+)\/restore$/)) && m === "POST") {
    if (!netlify.has(r[2])) return [404, { message: "Not Found" }];
    return [200, { id: r[2], state: "ready" }];
  }
  // ---------------------------------------------------------------- Fly Machines
  if ((r = p.match(/^\/v1\/apps\/([^/]+)\/machines$/))) {
    const machines = fly.get("machines");
    if (m === "GET") return [200, [...machines.values()].filter((x) => x.app === r[1])];
    if (m === "POST") {
      if (typeof body?.config?.image !== "string") problems.push("config.image missing");
      const id = `m_${++seq}`;
      machines.set(id, { id, app: r[1], state: "created", config: body?.config });
      return [200, machines.get(id)];
    }
  }
  if ((r = p.match(/^\/v1\/apps\/([^/]+)\/machines\/([^/]+)$/)) && m === "POST") {
    const mach = fly.get("machines").get(r[2]);
    if (!mach) return [404, { error: "machine not found" }];
    if (typeof body?.config?.image !== "string") problems.push("config.image missing");
    mach.config = body?.config;
    return [200, mach];
  }
  if ((r = p.match(/^\/v1\/apps\/([^/]+)\/machines\/([^/]+)\/wait$/)) && m === "GET") {
    if (url.searchParams.get("state") !== "started") problems.push("wait state must be started");
    const mach = fly.get("machines").get(r[2]);
    if (!mach) return [404, { error: "machine not found" }];
    mach.state = "started";
    return [200, { ok: true }];
  }
  // ---------------------------------------------------------------- Kubernetes
  if ((r = p.match(/^\/apis\/apps\/v1\/namespaces\/([^/]+)\/deployments\/([^/]+)$/))) {
    if (m === "PATCH") {
      if (req.headers["content-type"] !== "application/strategic-merge-patch+json") problems.push("PATCH content-type must be strategic-merge-patch+json");
      const c = body?.spec?.template?.spec?.containers;
      if (!Array.isArray(c) || typeof c[0]?.name !== "string" || typeof c[0]?.image !== "string") problems.push("containers[0].name/image missing");
      k8sGeneration += 1;
      polls.delete(`k8s:${p}`);
      return [200, { metadata: { name: r[2], namespace: r[1], generation: k8sGeneration } }];
    }
    if (m === "GET") {
      const n = pollCount(`k8s:${p}`);
      const avail = n < 2 ? 1 : 2;
      return [200, {
        metadata: { name: r[2], namespace: r[1], generation: k8sGeneration },
        spec: { replicas: 2 },
        status: { observedGeneration: k8sGeneration, replicas: 2, updatedReplicas: 2, availableReplicas: avail },
      }];
    }
  }
  return [404, { error: `fake-cloud-api: no route for ${m} ${p}` }];
}

const server = http.createServer((req, res) => {
  const chunks = [];
  req.on("data", (c) => chunks.push(c));
  req.on("end", () => {
    const url = new URL(req.url, "http://localhost");
    if (url.pathname === "/__requests") return json(res, 200, log);
    if (url.pathname === "/__reset") {
      log.length = 0;
      polls.clear();
      return json(res, 200, { ok: true });
    }
    const raw = Buffer.concat(chunks);
    let body = null;
    if (String(req.headers["content-type"] || "").includes("json") && raw.length) {
      try {
        body = JSON.parse(raw.toString("utf8"));
      } catch {
        body = undefined;
      }
    }
    const auth = req.headers.authorization || "";
    const authOk = TOKEN ? auth === `Bearer ${TOKEN}` : auth.startsWith("Bearer ") && auth.length > 7;
    const problems = [];
    if (body === undefined) problems.push("body is not valid JSON");
    const entry = {
      method: req.method,
      path: url.pathname + url.search,
      authOk,
      shapeOk: true,
      problems,
      contentType: req.headers["content-type"] || null,
      digest: req.headers["x-vercel-digest"] || null,
      bodyKeys: body && typeof body === "object" ? Object.keys(body) : [],
      body: body && typeof body === "object" ? body : null,
    };
    log.push(entry);
    if (!authOk) return json(res, 401, { error: "invalid token" });
    const [status, out] = route(req, url, raw, body, problems);
    entry.shapeOk = problems.length === 0;
    entry.status = problems.length ? 400 : status;
    if (problems.length) return json(res, 400, { error: "bad request shape", problems });
    json(res, status, out);
  });
});

server.listen(PORT, "127.0.0.1", () => {
  const { port } = server.address();
  process.stdout.write(JSON.stringify({ port, base: `http://127.0.0.1:${port}` }) + "\n");
});
for (const sig of ["SIGINT", "SIGTERM"]) process.on(sig, () => server.close(() => process.exit(0)));
