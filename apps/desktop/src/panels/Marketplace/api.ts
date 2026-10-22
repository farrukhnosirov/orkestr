/**
 * Marketplace RPC (`marketplace.*`, ADR-0040) and the plugin grant call used
 * by the install permission prompt. Wire types come from `@orkestr/protocol`;
 * the WP2 additions (`tags`, `token`, `marketplace.installed`,
 * `marketplace.uninstall`) are declared here until the TS bindings are
 * regenerated at integration.
 */
import type {
  AddSourceParams,
  EntryDetailsResult,
  InstallEntryParams,
  InstallEntryResult,
  KeysResult,
  MarketplaceSearchParams,
  PackageKind,
  PermissionRequest,
  PublishPackageParams,
  PublisherKey,
  RegistryEntry,
  RegistrySource,
} from "@orkestr/protocol";

import { getClient } from "../../core/client";

export type { PackageKind, PermissionRequest, PublisherKey, RegistrySource };

export type MarketEntry = RegistryEntry & { tags?: string[] };

export interface InstalledPackage {
  id: string;
  kind: PackageKind;
  name: string;
  version: string;
  sourceId: string;
  keyId: string;
  sha256: string;
  templateIds: string[];
  pluginId?: string;
  revoked: boolean;
  updateAvailable?: string;
  installedAt: string;
}

export type EntryDetails = Omit<EntryDetailsResult, "entry"> & {
  entry: MarketEntry;
};

export interface GrantDecision {
  capability: PermissionRequest["capability"];
  scope?: string;
  allow: boolean;
}

type LooseCall = (
  method: string,
  params: unknown,
  options?: { timeoutMs?: number },
) => Promise<unknown>;

/** Install / publish download, verify, sign and (plugins) compile: allow more than the default timeout. */
const SLOW = { timeoutMs: 180_000 };

/** Untyped call: the methods / fields above are not in the generated map yet. */
function call<T>(
  method: string,
  params: unknown,
  options?: { timeoutMs?: number },
): Promise<T> {
  const client = getClient() as unknown as { call: LooseCall };
  return client.call(method, params, options) as Promise<T>;
}

export const marketplaceApi = {
  sources: () =>
    call<{ sources: RegistrySource[] }>("marketplace.sources", {}).then(
      (r) => r.sources,
    ),
  addSource: (p: AddSourceParams & { token?: string }) =>
    call<{ source: RegistrySource }>("marketplace.addSource", p).then(
      (r) => r.source,
    ),
  removeSource: (sourceId: string) =>
    call<object>("marketplace.removeSource", { sourceId }),
  refresh: (sourceId?: string) =>
    call<{ sources: RegistrySource[] }>(
      "marketplace.refresh",
      sourceId ? { sourceId } : {},
    ).then((r) => r.sources),
  search: (p: MarketplaceSearchParams & { tags?: string[] }) =>
    call<{ entries: MarketEntry[] }>("marketplace.search", p).then(
      (r) => r.entries,
    ),
  get: (id: string, sourceId?: string) =>
    call<EntryDetails>("marketplace.get", sourceId ? { id, sourceId } : { id }),
  install: (p: InstallEntryParams) =>
    call<InstallEntryResult>("marketplace.install", p, SLOW),
  installed: () =>
    call<{ packages: InstalledPackage[] }>("marketplace.installed", {}).then(
      (r) => r.packages,
    ),
  uninstall: (id: string) => call<object>("marketplace.uninstall", { id }),
  publish: (p: PublishPackageParams) =>
    call<{ entry: MarketEntry }>("marketplace.publish", p, SLOW).then(
      (r) => r.entry,
    ),
  keys: () => call<KeysResult>("marketplace.keys", {}),
  trustKey: (keyId: string, fingerprint: string) =>
    call<{ key: PublisherKey }>("marketplace.trustKey", {
      keyId,
      fingerprint,
    }).then((r) => r.key),
  untrustKey: (keyId: string) =>
    call<{ key: PublisherKey }>("marketplace.untrustKey", { keyId }).then(
      (r) => r.key,
    ),
  grant: (pluginId: string, decisions: GrantDecision[], enable: boolean) =>
    call<object>("plugins.grant", { pluginId, decisions, enable }, SLOW),
};
