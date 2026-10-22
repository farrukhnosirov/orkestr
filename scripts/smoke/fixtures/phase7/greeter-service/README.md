# Greeter service (Phase 7 acceptance fixture)

Tiny Node HTTP service: `GET /` and `GET /health`. `npm test` runs the
`node:test` suite in `tests/`.

The acceptance run (`scripts/smoke/phase7-acceptance.mjs`) copies this folder
into a temporary git repository, deploys it as a LocalProcess, then commits
`../broken/greeter.mjs` over `src/greeter.mjs` (a regression: `opts` is
dereferenced without a default, so `greeter()` throws and `/health` answers
500 with a stack trace). The Bug Detective must trace the alert back to that
commit. The offline Mock fix agent writes `src/<subject>.mjs` with a correct
`<subject>(name = "world")` greeter, where the subject comes from the task
title ("Bug Detective fix: Greeter module …" → `greeter`): that is exactly the
fix this regression needs, so the tests pass on the `detective/<id>` branch.
