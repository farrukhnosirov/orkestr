# ADR-0002: npm workspaces instead of pnpm

- **Status:** Accepted
- **Date:** 2026-10-02
- **Phase:** 0
- **Spec:** §60, §65 #4
- **Related:** ADR-0001

## Context

The TS side is `apps/desktop` plus `packages/{protocol,client,ui}`, then `apps/web` (Phase 4) and `apps/mobile` (Phase 5). The development host is Windows 11 with Node 24. pnpm is **not installed** on the host, and the build agents are not allowed to install global tooling. React Native (Phase 5) has a long history of problems with pnpm's symlinked `node_modules` and Metro resolution.

## Decision drivers

1. Zero extra tooling on a fresh Windows machine or CI runner (Node ships npm).
2. Metro / React Native compatibility in Phase 5.
3. Deterministic installs with a lockfile.
4. Workspace protocol linking for `@orkestr/*` packages.

## Considered options

| Option | Pros | Cons |
|---|---|---|
| **npm workspaces** | Bundled with Node 24; flat hoisted `node_modules` that Metro and Tauri handle; `package-lock.json` | Slower installs; phantom dependencies are possible; no `workspace:` protocol (use `"*"`) |
| pnpm | Fast, strict, disk-efficient | Not installed; Windows symlink/junction edge cases; RN needs `node-linker=hoisted`, which removes most of the benefit |
| Yarn Berry (PnP) | Strict and fast | PnP breaks Tauri/Vite plugins and Metro; would need corepack setup |
| Bun | Fast | Windows maturity and lockfile stability are not proven for this stack |

## Decision

Use **npm workspaces**. The root `package.json` declares `"workspaces": ["apps/desktop", "packages/*"]`. Internal dependencies use `"@orkestr/client": "*"`. Strictness is restored by:

- `eslint-plugin-import` `no-extraneous-dependencies` per package, so phantom dependencies fail lint;
- `npm ci` in CI with the committed `package-lock.json`;
- the integrator owns installs: parallel agents never run `npm install` (contract §8).

Scripts run through `npm run <script> -w <workspace>` or `--workspaces --if-present`.

## Consequences

- **Positive:** runs on a fresh Windows box with nothing else installed, and Metro compatibility is preserved for Phase 5.
- **Negative:** installs are slower and `node_modules` is larger, which is acceptable. Version conflicts between workspaces resolve through nested `node_modules`, so watch for duplicate React. The guard is a root `overrides` entry for `react`/`react-dom` plus a CI check (`npm ls react` must show a single version).
- **Revisit:** if pnpm becomes available on all hosts and RN is not in the tree yet, a superseding ADR may switch. Switching is cheap because only the lockfile and `workspace:` specifiers change.

## Implementation notes (Phase 0 foundation, 2026-10-02)

Status and errata only; the decision above is unchanged.

- The root `package.json` declares `"workspaces": ["apps/*", "packages/*"]` (so `apps/web`/`apps/mobile` will be picked up automatically when they appear).
- Internal dependencies use the exact workspace version (`"@orkestr/protocol": "0.1.0"`), not `"*"`; both resolve to the workspace package.
- There is no root `overrides` entry for `react`/`react-dom` yet; React 18 is pinned through the root `devDependencies` and the packages' `peerDependencies`. The `npm ls react` CI check does not exist yet (no CI).
- `package-lock.json` is present.
