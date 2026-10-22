# ADR-0040 — Marketplace registry and package signing

Status: accepted — implemented (Phase 7)

## Context

Agent templates, plugins and widgets need a distribution channel (§6, §48,
§68 #1). Users must be able to self-host it and work offline. Updates must
not let a different publisher take over a package, and a compromised
package or key must be revocable.

## Decision

1. **Static signed registry.** A registry is `index.json`, `index.sig` and
   package files.
   - `index.json`: `{format: 1, name, keys[{keyId, publisher, publicKey}],
     entries[{id, kind, name, version, publisher, description, tags,
     permissions, keyId, sha256, signature, url, revoked}], revokedKeys[]}`.
     `kind` is `Plugin`, `Widget` or `AgentTemplate`; `permissions` repeats
     the plugin manifest's requests so the prompt can be shown before
     download.
   - `index.sig` is a **JSON detached signature** `{keyId, publicKey,
     signature}` (ed25519 by the registry key over the exact bytes of
     `index.json`).
   - Entry `signature` is the publisher's ed25519 signature over the raw
     32-byte sha256 of the package. `keyId` = first 16 hex of sha256 of the
     raw public key (same as ADR-0034). Verification reuses
     `orkestr_plugins::signing` (`Marketplace::set_verifier`).
2. **Sources** (`marketplace.sources/addSource/removeSource/refresh`):
   - `Official` (id `official`): the bundled catalogue
     (`resources/official-registry/`, two signed agent templates, stored with
     `-text` in `.gitattributes`), verified with a key **embedded in the
     binary** (`OFFICIAL_PUBLIC_KEY`). The private seed is kept outside the
     repository; re-sign with `cargo run -p orkestr-marketplace --example
     sign_registry -- resources/official-registry <seed>`.
   - `Directory`: a local folder.
   - `Http`: an HTTP(S) base URL, optionally with a bearer token (stored in
     the vault as `MARKETPLACE_SOURCE_TOKEN_<ID>`).
   For `Directory` and `Http` the **registry key is pinned on first use**
   (TOFU) when the source is added; a later index signed by another key is
   refused. A tampered or badly signed index sets the source's `lastError`
   and the last verified index stays in use.
3. **Gateway registry.** `orkestr-gateway` serves a self-hosted registry
   from `<data_dir>/registry` (registry key `registry.key`):
   `GET /registry/index.json`, `GET /registry/index.sig`,
   `GET /registry/packages/<sha256>`; `POST /registry/publish` (bearer
   session or core token; body `{entry, publicKey, signature,
   packageBase64}`; duplicate `id@version` → 409; the gateway re-signs the
   index) and `POST /registry/revoke` (`{id, version?}` or `{keyId}`).
4. **Publisher keys.** The UI shows the key fingerprint. An unknown
   publisher key is `Untrusted`; `marketplace.trustKey` trusts it (TOFU),
   `marketplace.untrustKey` removes it. The first key that installs a
   package id is **pinned per package**; an update signed by a different key
   is refused. `revokedKeys` and per-entry `revoked` block installs and
   updates; installed packages show `revoked: true`.
5. **Install order.** Revocation → publisher trust (an untrusted key needs
   `trustKey`) → per-package key pin → sha256 → signature → install. Plugins
   and widgets go through `PluginService::install_bytes` (source
   `marketplace:<source>/<id>`, pending grants go to the permission prompt);
   agent templates go through `templates.import`. `marketplace.installed`
   and `marketplace.uninstall` manage what was installed.
6. **Local publishing.** `marketplace.publish {path, kind, sourceId,
   publisher}` signs with the publisher key held in the vault
   (`MARKETPLACE_PUBLISHER_KEY`, created on first publish; its fingerprint
   is listed by `marketplace.keys` as the local publisher) and either
   rewrites a directory registry (re-signing its index) or POSTs to a
   gateway registry.
7. **Policy.** `marketplace.install`, `addSource`, `removeSource`,
   `trustKey`, `untrustKey`, `uninstall` and `publish` are `local_only`;
   `sources`, `keys`, `installed`, `search` and `get` are reads.

## Consequences

- No central service. Discovery spans the configured sources only.
- Deferred: a public hosted marketplace, ratings and payments, plugin
  auto-update.
