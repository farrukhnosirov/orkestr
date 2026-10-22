# ADR-0041 — Environments and deployment adapters

Status: accepted — in progress (Phase 7)

## Context

§26 (Deployment Agents) needs environments, many deployment targets, approval
for production, logs and rollback. Most cloud targets cannot be exercised
live in development or CI. Docker may or may not be installed.

## Decision

1. **Environments** (`environments.list/create/update/delete`) wrap the
   domain `Environment` and add a `DeploymentTarget` and non-secret
   `adapterSettings`. Tokens are vault secret refs and never appear in
   params, logs or events. Production is always protected. Update and delete
   require admin role and step-up for remote callers.
2. **`DeployAdapter` trait** (`orkestr_deploy`): `id`, `target`,
   `availability` (`Ready | NeedsCredential{secret} | Unavailable{reason}`),
   `plan` (redacted request previews, no side effects), and `deploy` /
   `rollback` with a `LogSink` and a `CancellationToken`. Plugin
   `DeploymentProvider`s implement the same trait (target `Plugin(id)`).
3. **Adapters in Phase 7.** `StaticSite` (publish to a served directory) and
   `LocalProcess` (supervised process plus health URL) are fully working.
   `Docker` is real when `docker` is on PATH and `Unavailable` otherwise.
   `Vercel`, `Netlify`, `Fly` and `Kubernetes` use reqwest with a base URL
   override env var (`ORKESTR_<ADAPTER>_API`) and are verified against fake
   APIs (`scripts/smoke/fixtures/fake-cloud-api.mjs`). They are labelled
   `verification: "requestShape"`; no live cloud run is claimed.
4. **Approval.** `deploy.start` to a protected environment asks the Dangerous
   Action Guard. The deployment waits in `AwaitingApproval` (with the
   `ApprovalRequested` event) and runs after approval. `deploy.start`,
   `deploy.rollback` → `MethodPolicy::dangerous()`; `deploy.cancel` →
   emergency class. Emergency stop cancels every running deployment.
5. **Audit and logs.** Every deployment is audited. Log lines are redacted
   (vault values plus known token patterns) and stored in the object store.
   Events are `DeploymentStarted`, `DeploymentProgress` (ephemeral) and
   `DeploymentFinished`. Rollback restores the previous successful version
   (or `toVersion`).

## Consequences

- Deploy commands run without a container sandbox. They need the Terminal
  capability and the guard; this residual risk is documented here.
- Deferred: AWS, GCP, Azure and Cloudflare adapters; live cloud runs; deploys
  inside a container sandbox.
