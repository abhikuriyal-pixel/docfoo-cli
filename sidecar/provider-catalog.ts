/**
 * Shared live-discovery plumbing for the bundled providers (Inception, InferX).
 *
 * The provider's `/v1/models` list is the catalog: persistence stores the raw
 * API rows, and the pi model configs (compat shims, thinking map, costs) are
 * rebuilt from those rows on every restore. Code-level metadata changes then
 * always apply after an update, instead of being pinned by the cache.
 *
 * Pi's model store calls `refreshModels` in two phases:
 *
 *  1. Offline (`allowNetwork: false`) — rebuild the catalog from the rows
 *     persisted by a previous refresh. This runs when the provider is
 *     registered, so the last discovered models are usable immediately,
 *     including on machines that are temporarily offline.
 *  2. Network — when network access is allowed and a credential resolves.
 *     The fetch happens when the persisted rows are missing or older than
 *     {@link CATALOG_TTL_MS}, or when `force` is set (which is what
 *     `docfoo model --list --refresh` passes). A successful fetch is
 *     persisted so later starts stay offline.
 *
 * A failed fetch or a corrupt store entry must never break listing or
 * querying, so every network path falls back to the persisted list.
 */
import type { ModelRuntime } from "@earendil-works/pi-coding-agent";

type ProviderConfigInput = Parameters<ModelRuntime["registerProvider"]>[1];
export type ProviderModelConfig = NonNullable<ProviderConfigInput["models"]>[number];
type RefreshModels = NonNullable<ProviderConfigInput["refreshModels"]>;
type RefreshContext = Parameters<RefreshModels>[0];
type StoredModels = NonNullable<RefreshContext["stored"]>["models"];

/** How long a persisted catalog is trusted before the next refresh. */
const CATALOG_TTL_MS = 24 * 60 * 60 * 1000;
/** A slow provider must not hold up sidecar start or a models request. */
const FETCH_TIMEOUT_MS = 10_000;

/** The api-key fields of the resolved caller credential. */
export interface RefreshCredential {
  type: string;
  key?: string;
}

export interface CatalogSource<T> {
  /** Fetch the provider's live catalog; `null` (or a throw) keeps the cache. */
  fetchLive: (
    credential: RefreshCredential | undefined,
    signal: AbortSignal,
  ) => Promise<T[] | null>;
  /** Build the pi model config for one discovered row, with the current code. */
  toConfig: (entry: T) => ProviderModelConfig | null;
}

/**
 * Persisted rows are raw API entries (snake_case), never pi model configs
 * (camelCase `contextWindow`). Rejecting config-shaped rows lets an update
 * that changes this module's metadata invalidate its own old cache.
 */
export function isApiEntry<T>(value: unknown): value is T {
  const row = value as { id?: unknown; contextWindow?: unknown };
  return (
    typeof row?.id === "string" &&
    row.id.length > 0 &&
    typeof row.contextWindow === "undefined"
  );
}

function restoredEntries<T>(
  stored: readonly unknown[] | undefined,
  isEntry: (value: unknown) => value is T,
): T[] | null {
  if (!stored || stored.length === 0) return null;
  if (!stored.every((entry) => isEntry(entry))) return null;
  return stored as T[];
}

export function createCatalogRefresh<T>(source: CatalogSource<T>): RefreshModels {
  return async (context: RefreshContext) => {
    const entries = restoredEntries(context.stored?.models, isApiEntry);
    const configs = (rows: readonly T[]): ProviderModelConfig[] =>
      rows.flatMap((entry) => {
        const config = source.toConfig(entry);
        return config ? [config] : [];
      });
    // An unreadable cache counts as missing, so a format change or corruption
    // re-fetches instead of leaving the provider empty until the TTL expires.
    const checkedAt = context.stored?.checkedAt;
    const stale = entries === null || checkedAt === undefined || Date.now() - checkedAt >= CATALOG_TTL_MS;
    if (!context.allowNetwork || context.signal.aborted || (!context.force && !stale)) {
      return configs(entries ?? []);
    }
    try {
      const signal = AbortSignal.any([context.signal, AbortSignal.timeout(FETCH_TIMEOUT_MS)]);
      const live = await source.fetchLive(context.credential, signal);
      if (!live || live.length === 0) return configs(entries ?? []);
      await context.publish({
        persist: {
          models: live as unknown as StoredModels,
          checkedAt: Date.now(),
          lastModified: Date.now(),
        },
      });
      return configs(live);
    } catch {
      return configs(entries ?? []);
    }
  };
}
