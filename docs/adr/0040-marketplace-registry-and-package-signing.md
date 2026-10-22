# ADR-0040 — Marketplace registry and package signing

Status: accepted — in progress (Phase 7)

## Context

Agent templates, plugins and widgets need a distribution channel (§6, §48,
§68 #1). Users must be able to self-host it and work offline. Updates must
not let a different publisher take over a package, and a compromised
package must be revocable.

## Decision

1. **Static signed registry.** A registry is a directory with `index.json`,
   `index.sig` and package files. `index.json` is `{format: 1, name,
   entries[{id, kind, name, version, publisher, description, keyId, sha256,
   url, revoked}], revokedKeys[]}`. `index.sig` is an ed25519 signature over
   the exact bytes of `index.json`, made with the registry key. A package is
   accepted only if its sha256 matches the entry and its own publisher
   signature verifies (ADR-0034).
2. **Sources** (`marketplace.sources/addSource/removeSource/refresh`):
   - `Official`: the bundled catalogue (`resources/official-registry/`),
     verified with a key embedded in the binary.
   - `Directory`: a local folder.
   - `Http`: any HTTP(S) base URL. `orkestr-gateway` serves one at
     `/registry/*` from `<data_dir>/registry`, so a team can self-host.
3. **Publisher keys.** The UI shows the fingerprint (sha256 of the key,
   grouped hex). Trust is **on first use**: `marketplace.trustKey` requires
   the fingerprint the user confirmed. The trusted key is **pinned** per
   package id, and an update signed by a different key is refused. The index
   `revokedKeys` list and per-entry `revoked` flags block installs and
   updates. Revocation shows as `KeyTrust::Revoked`.
4. **Install.** `marketplace.install` handles plugins and widgets through
   `plugins.install` (pending grants are returned to the permission prompt)
   and agent templates through the existing templates import. Installs,
   source additions, key trust and publishing are `local_only`.
5. **Local publishing.** `marketplace.publish` signs with a publisher key
   held in the vault (created on first publish), adds the package to a
   writable directory source or uploads it to a gateway registry, and
   re-signs the index.

## Consequences

- No central service. Discovery spans the configured sources only.
- Deferred: a public hosted marketplace, ratings and payments.
