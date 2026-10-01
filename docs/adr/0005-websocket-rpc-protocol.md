# ADR-0005: WebSocket JSON RPC-like protocol shared by all clients

- **Status:** Accepted
- **Date:** 2026-10-02
- **Phase:** 1 (and every later client)
- **Spec:** §2, §65 deliverable #19 (the contract §4 heading says "§19 deliverable", but §19 is MCP Hub; see CA-12), §35, §49, §51 ("UI does not touch the DB"), §67
- **Review:** RR-G1, RR-G2, RR-P1…RR-P10, CA-1, CA-5
- **Spike:** SP-14 · **Risks:** R-02, R-09

## Context

Desktop (Phase 1), Web (Phase 4) and Mobile (Phase 5) must share one application protocol (§65 #19, §67). Web must work over a relay without RDP (§2, §35). The protocol carries request/response calls and high-rate server push: agent output deltas, terminal output, events (§49).

## Decision drivers

1. One transport for requests **and** push, so subscriptions and the auth context are the same.
2. Browsers cannot set WebSocket headers (Web Phase 4), so in-band auth is needed.
3. Types generated from Rust (ADR-0006), with no IDL toolchain on Windows.
4. Works through a reverse proxy / relay (Phase 4) as plain WS.
5. Resume after a reconnect (mobile, §37).

## Considered options

| Option | Verdict |
|---|---|
| **A. JSON frames over one WebSocket, `type`-tagged, JSON-RPC-like (`req`/`res`/`sub`/`event`)** | **Chosen** |
| B. Strict JSON-RPC 2.0 | Rejected. Its notifications have no subscription semantics or seq, and it is no simpler for us |
| C. gRPC / gRPC-web + protobuf | Rejected. Needs a proxy for browsers, protoc tooling, and RN support is awkward. Revisit for node↔node in Phase 6 (ADR-0033) |
| D. REST + SSE | Rejected. Two channels, and SSE is one-way with auth problems in browsers |
| E. Tauri IPC for desktop + HTTP for remote | Rejected. Two code paths, and it breaks RR-P5 |

## Decision

### Transport

`ws://127.0.0.1:<port>/rpc` (TLS terminates at the gateway in Phase 4). `GET /health` (no auth) returns `{"status":"ok","version":"0.1.0","protocolVersion":1}`. The server is axum 0.8 in `orkestr-core::rpc`.

### Frames (contract §4 + additive extensions)

Contract frames, unchanged:
```json
{"type":"auth","token":"…"}
{"type":"req","id":"c-42","method":"tasks.transition","params":{"taskId":"…","to":"Ready"}}
{"type":"res","id":"c-42","ok":true,"result":{…}}
{"type":"res","id":"c-42","ok":false,"error":{"code":"conflict","message":"…","data":{"reason":"invalid_transition","from":"Planned","to":"Completed"}}}
{"type":"sub","id":"s-1","topics":["project:018f…","global"]}
{"type":"event","seq":1234,"event":{"seq":1234,"id":"…","ts":"…","kind":"TaskStatusChanged","projectId":"…","taskId":"…","payload":{"from":"Ready","to":"Running"}}}
```

Additive extensions (Accepted here; CA-5 asks the integrator to fold them into the contract):
```json
{"type":"hello","protocolVersion":1,"serverVersion":"0.1.0","nodeId":"…","sessionId":"…","capabilities":["voice","robots"]}  // server→client after successful auth
{"type":"sub","id":"s-1","topics":["task:…"],"sinceSeq":1200}                  // replay durable events with seq > 1200
{"type":"res","id":"s-1","ok":true,"result":{"topics":["task:…"],"lastSeq":1234}} // sub ack
{"type":"unsub","id":"s-2","topics":["task:…"]}
{"type":"ping","id":"p-1"}   →   {"type":"pong","id":"p-1"}
{"type":"lagged","topics":["project:…"],"lastSeq":1500}   // server dropped events for a slow client; client re-subs with sinceSeq
```

### Rust types (`orkestr-protocol::frames`)

```rust
#[derive(Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum ClientFrame {
    Auth  { token: String },
    Req   { id: String, method: String, params: serde_json::Value,
            #[serde(default, skip_serializing_if = "Option::is_none")] idempotency_key: Option<String> },
    Sub   { id: String, topics: Vec<Topic>, #[ts(type = "number | null")] since_seq: Option<u64> },
    Unsub { id: String, topics: Vec<Topic> },
    Ping  { id: String },
}

#[derive(Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum ServerFrame {
    Hello  { protocol_version: u32, server_version: String, node_id: NodeId, session_id: String, capabilities: Vec<String> },
    Res    { id: String, ok: bool, #[serde(skip_serializing_if = "Option::is_none")] result: Option<serde_json::Value>,
             #[serde(skip_serializing_if = "Option::is_none")] error: Option<RpcError> },
    Event  { #[ts(type = "number")] seq: u64, event: Event },
    Lagged { topics: Vec<Topic>, #[ts(type = "number")] last_seq: u64 },
    Pong   { id: String },
}

#[derive(Serialize, Deserialize, TS)] #[serde(rename_all = "snake_case")] #[ts(export)]
pub enum ErrorCode { Unauthorized, BadRequest, NotFound, Conflict, Forbidden, ApprovalRequired,
                     RateLimited, BudgetExceeded, ProviderError, Internal /* + Unimplemented, InvalidTransition per CA-1 */ }

pub type Topic = String; // "global" | "project:<id>" | "task:<id>" | "agent:<id>" | "terminal:<id>"
```

### Server semantics

- **Auth:**
  - The connection must authenticate within 5 s, through the header or the first frame. Otherwise the server closes with code **4401**.
  - The token is compared in constant time (`subtle::ConstantTimeEq`). After 5 failed auths per minute from one connection source, the server stops answering.
  - The token resolves to `Session{ actor: Actor, device_id: DeviceId, scopes }` (RR-P1). Every handler receives `&Session`.
- **Dispatch:**
  - `Router::register(area_handlers)`. Each method is `async fn(Ctx, Params) -> Result<Result, RpcError>`.
  - Unknown method returns `bad_request` with `data.reason = "unknown_method"`.
  - Params that fail serde return `bad_request` with the serde path.
  - Requests run concurrently. Responses may come back out of order, matched by `id`.
- **Idempotency:** for mutating methods, `(sessionId, idempotencyKey)` maps to the cached response for 10 min (RR-P8).
- **Subscriptions:** each connection has one bounded mpsc of 4096 frames. The bus filters events by topic, and the project/task/agent ids decide topic membership; one event can match several topics but is delivered once per connection.
  - When the mpsc is full, the server drops events and sends `Lagged` once.
  - `TerminalOutput` and `AgentMessage` are coalesced per 16 ms per terminal or agent before they are enqueued.
- **Authorization:** `Session.scopes` is checked per area. Phase 1's local session has full scope. Terminal methods also run `PermissionEngine::check(actor, Terminal, …)` (RR-P7).
- **Frame limits:** maximum incoming frame 8 MiB, outgoing frames chunked at 4 MiB (`files.read` paging, RR-P6).
- **Versioning:** `protocolVersion` is a major version. Methods, params fields (optional only), event kinds and error codes are **append-only**. A breaking change bumps the major version, and the server serves N and N-1 for one release.

### Method catalogue (Phase 1, non-exhaustive)

`system.health`, `system.resumeSummary`, `system.settings.get|set`, `projects.list|get|create|update|delete|open`, `tasks.list|get|create|update|transition|addDependency|removeDependency`, `agents.listTemplates|spawn|list|get|stop|send`, `manager.send|plan|approvePlan`, `providers.list|get|configure|test|listModels`, `approvals.list|resolve`, `files.list|read|write|stat|watch`, `git.status|diff|branches|log|worktrees`, `terminal.create|write|resize|kill|list|snapshot`, `snapshots.create|list|rollbackTask|rollbackWorkspace`, `memory.list|upsert|delete|search`, `costs.summary|budgets.get|budgets.set`, `events.query`, `ui.getConfig|applyPatch|history|revert`, `secrets.list|put|delete` (never `get` over RPC: values are never returned), `emergency.stopAll|stopProject|disableFullAuto|lockVault`.

### Client (`@orkestr/client`)

```ts
export interface OrkestrClient {
  call<M extends keyof Methods>(method: M, params: Methods[M][0], opts?: { idempotencyKey?: string; timeoutMs?: number }): Promise<Methods[M][1]>;
  subscribe(topics: Topic[], onEvent: (e: Event) => void): Unsubscribe; // tracks lastSeq per topic, auto-resubscribes with sinceSeq
  readonly state: "connecting" | "authenticating" | "ready" | "reconnecting" | "closed";
  onStateChange(cb: (s: OrkestrClient["state"]) => void): Unsubscribe;
}
export function createClient(opts: { url: string; token: string | (() => Promise<string>); WebSocketImpl?: typeof WebSocket }): OrkestrClient;
```

Reconnect uses exponential backoff from 250 ms to 10 s with jitter. Pending calls are rejected with `{code:"internal",data:{reason:"disconnected"}}` unless they carry an `idempotencyKey`, in which case they are retried once.

## Consequences

- **Positive:** one protocol for every client; push and requests share auth; trivially relayable; easy to debug (plain JSON).
- **Negative:** JSON overhead on the terminal and agent streams. Mitigated by coalescing, and the 16 ms batching is enough at local speeds. If SP-14 shows `TerminalOutput` above 2 MB/s, a binary sub-frame type can be added (additive).
- **Negative:** RPC-like with no schema validation at runtime in TS. Mitigated because types are generated and the server validates params.

## Implementation notes (Phase 0 foundation, 2026-10-02)

Status and errata only; the decision above is unchanged. The normative description of what is built is `../api/protocol.md` §0; summary of the differences:

- **Implemented:** `auth` (frame or `Authorization` header), `req`/`res`, `sub` with ack `{topics}`, `unsub`, `event`; the ten error codes; loopback-only bind and `Host` check; constant-time token compare; `GET /health` → `{status, version}` (no `protocolVersion`).
- **Replaced:** `hello` → the `auth` response `res{id:"auth", result: AuthResult{sessionId, serverVersion, protocolVersion, lastSeq}}`; `lagged` → a local `Log` event with `seq: 0`; `sinceSeq` resume → `events.replay {fromSeq, topics}` (done by `@orkestr/client` after reconnects).
- **Not implemented yet:** `hello.capabilities`, `ping`/`pong` frames, `req.idempotencyKey`, `Session`/scopes, failed-auth rate limiting, 16 ms coalescing, close codes other than 4401/1001, 8 MiB frames (v1: 4 MiB), the 4096-frame per-connection queue (v1: 512), `unknown_method` reason data.
- **Different values:** auth deadline 10 s (not 5 s); token 64 hex chars.
- **Rust types as built:** `ClientFrame { Auth{token, id, client}, Req{id, method, params}, Sub{id, topics}, Unsub{id, topics} }`, `ServerFrame { Res(ResFrame), Event{seq, event} }`, `RpcErrorCode` (not `ErrorCode`); topics are `String`s and also accept `*` and `node:<id>`.
- **Method names:** `costs.*` → `cost.*`, `git.*` → `vcs.*`, `terminal.create/kill` → `terminal.open/close`, `system.settings.get|set` → `settings.get/update`, `events.query` → `events.replay` (+ planned `events.query`); see `../api/protocol.md` §8.27.
- **Client as built:** `new OrkestrClient(url, token, options)` + `connect()`, `call`, `subscribe`, `onStateChange`, `state` (`idle | connecting | authenticating | open | reconnecting | closed`); no `createClient`, no token callback, no idempotent retry.
