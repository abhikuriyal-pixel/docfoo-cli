/**
 * Shared live-discovery plumbing for the bundled providers (Inception, InferX).
 *
 * The provider's `/v1/models` list is the catalog — the modules below only
 * map the response into pi model configs. Pi's model store calls
 * `refreshModels` in two phases:
 *
 *  1. Offline (`allowNetwork: false`) — restore the catalog persisted by a
 *     previous refresh. This runs when the provider is registered, so the
 *     last discovered models are usable immediately, including on machines
 *     that are temporarily offline.
 *  2. Network — when network access is allowed and a credential resolves.
 *     The fetch happens when the persisted catalog is missing or older than
 *     {@link CATALOG_TTL_MS}, or when `force` is set (which is what
 *     `docfoo model --list --refresh` passes). A successful fetch is persisted
 *     in the workspace store so later starts stay offline.
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

/** How long a persisted catalog is trusted before the next startup refresh. */
const CATALOG_TTL_MS = 24 * 60 * 60 * 1000;
/** A slow provider must not hold up sidecar start or a models request. */
const FETCH_TIMEOUT_MS = 10_000;

/** The api-key fields of the resolved caller credential. */
export interface RefreshCredential {
  type: string;
  key?: string;
}

export interface CatalogSource {
  /** Map the provider's `/v1/models` response to model configs. */
  fetchLive: (
    credential: RefreshCredential | undefined,
    signal: AbortSignal,
  ) => Promise<ProviderModelConfig[] | null>;
}

/** A persisted catalog is only reused when every entry looks like a model config. */
function restoredModels(stored: readonly unknown[] | undefined): ProviderModelConfig[] | null {
  if (!stored || stored.length === 0) return null;
  if (!stored.every((model) => typeof (model as { id?: unknown })?.id === "string")) return null;
  return stored as ProviderModelConfig[];
}

export function createCatalogRefresh(source: CatalogSource): RefreshModels {
  return async (context: RefreshContext) => {
    const restored = restoredModels(context.stored?.models);
    const checkedAt = context.stored?.checkedAt;
    const stale = checkedAt === undefined || Date.now() - checkedAt >= CATALOG_TTL_MS;
    if (!context.allowNetwork || context.signal.aborted || (!context.force && !stale)) {
      return restored ?? [];
    }
    try {
      const signal = AbortSignal.any([context.signal, AbortSignal.timeout(FETCH_TIMEOUT_MS)]);
      const live = await source.fetchLive(context.credential, signal);
      if (!live || live.length === 0) return restored ?? [];
      await context.publish({
        persist: {
          models: live as unknown as StoredModels,
          checkedAt: Date.now(),
          lastModified: Date.now(),
        },
      });
      return live;
    } catch {
      return restored ?? [];
    }
  };
}
