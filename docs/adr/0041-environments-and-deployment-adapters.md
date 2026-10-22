# ADR-0041 — Environments and deployment adapters

Status: accepted — implemented (Phase 7; cloud adapters request-shape verified)

## Context

§26 (Deployment Agents) needs environments, many deployment targets, approval
for production, logs and rollback. Most cloud targets cannot be exercised
live in development or CI. Docker may or may not be installed.

## Decision

1. **Environments** (`environments.list/create/update/delete`) wrap the
   domain `Environment` and add a `DeploymentTarget` and non-secret
   `adapterSettings` (output dir, command, port, health URL / path, site or
   app name, namespace, …). Production is always protected. Update and
   delete require the admin role and step-up for remote callers.
2. **Credentials only via vault refs.** Adapters declare a
   `required_secret`; the token is read from the vault at deploy time and
   never appears in params, plans, logs or events (`Credentials` redacts
   itself in `Debug`). Secret names: `VERCEL_TOKEN`, `NETLIFY_AUTH_TOKEN`,
   `FLY_API_TOKEN`, `KUBE_TOKEN`. Without it the adapter reports
   `NeedsCredential{secret}`.
3. **`DeployAdapter` trait** (`orkestr_deploy`): `id`, `target`,
   `availability` (`Ready | NeedsCredential{secret} | Unavailable{reason}`),
   `plan` (redacted request previews, no side effects), `deploy` / `rollback`
   with a `LogSink` and a `CancellationToken`, plus defaults
   `required_secret`, `verification`, `settings_schema`, `commands`,
   `release`, `rollback_to`. `DeployService::register_adapter` accepts more
   (intended for plugin `DeploymentProvider`s, target `Plugin(id)`).
4. **Adapters in Phase 7.**
   - `StaticSite` — publishes to a served directory (own HTTP server per
     environment, optional SPA fallback, health path); restored after a core
     restart.
   - `LocalProcess` — a supervised process plus a health URL; not restarted
     after a core restart.
   - `Docker` — real when `docker` is on PATH, otherwise `Unavailable`
     (never required by tests).
   - `Vercel`, `Netlify`, `Fly`, `Kubernetes` — reqwest clients labelled
     `verification: "requestShape"`: verified against
     `scripts/smoke/fixtures/fake-cloud-api.mjs` (checks method, path,
     auth and body shape), **no live cloud run is claimed**. Base URL
     overrides: `ORKESTR_VERCEL_API`, `ORKESTR_NETLIFY_API`,
     `ORKESTR_FLY_API`, `ORKESTR_KUBERNETES_API` (alias
     `ORKESTR_DEPLOY_<ADAPTER>_BASE`).
5. **Approval via the guard.** `deploy.start` to a protected environment asks
   the Dangerous Action Guard. The deployment waits in `AwaitingApproval`
   (with `approvalId` and the `ApprovalRequested` event; a background waiter
   resumes it) and runs after approval; a rejection ends it `Cancelled`.
   `deploy.start`, `deploy.rollback` → `MethodPolicy::dangerous()`;
   `deploy.cancel` → emergency class. Emergency stop cancels every running
   deployment (`cancel_all`).
6. **Audit, logs, events.** Every deployment is audited (capability
   `Deploy`). Log lines are redacted (vault values plus known token patterns)
   and stored in the object store (`log_object_id`). Events:
   `DeploymentStarted`, `DeploymentProgress` (ephemeral, ≤ 1 per 200 ms) and
   `DeploymentFinished`.
7. **Versions and rollback.** Local versions live under
   `deploy/{static,process}/<env>/versions/<version>` with a `current`
   pointer. `deploy.rollback` restores the previous successful version (or
   `toVersion`); a failed health check after a release falls back to the
   previous version. Feeds for other services: `recent(limit)` and `logs`
   (monitor deployment-log sources), `list/get/record/current/previous`
   (Bug Detective evidence).

## Consequences

- **Residual risk:** deploy commands run without a container sandbox. They
  need the Terminal capability and pass the guard for protected
  environments; this is documented, not mitigated.
- Cloud adapters may drift from the real APIs; they are labelled
  request-shape verified until run against real accounts.
- Deferred: AWS, GCP, Azure and Cloudflare adapters; live cloud runs; deploys
  inside a container sandbox; plugin deployment providers at runtime.
