/**
 * Bundled Inception provider for the DocFoo sidecar.
 *
 * Inception (https://inceptionlabs.ai) serves the Mercury models over an
 * OpenAI-compatible endpoint. The catalog is discovered live from
 * `https://api.inceptionlabs.ai/v1/models` (public), so new Mercury releases
 * show up without a sidecar rebuild: the sidecar refreshes it on start when
 * the persisted catalog is older than 24 h, and `docfoo model --list
 * --refresh` forces a fetch. Discovered models are cached per workspace in
 * `<workspace>/.agent/models-store.json`.
 *
 * Registering the provider here — as a local module compiled into
 * `docfoo-agent` — means every workspace and platform gets it, and it
 * survives `docfoo update`/`install.sh` (which only replace the two
 * binaries).
 *
 * Credentials stay external: `INCEPTION_API_KEY` (e.g. in
 * `~/.config/systemd/user/docfoo-sidecar.env` for the systemd service) or
 * `docfoo auth --set inception --key …` (stored per workspace in
 * `<workspace>/.agent/auth.json`). Unlike the list endpoint, completions
 * require the key.
 */
import type { ModelRuntime } from "@earendil-works/pi-coding-agent";
import {
  createCatalogRefresh,
  type ProviderModelConfig,
  type RefreshCredential,
} from "./provider-catalog.js";

export const INCEPTION_PROVIDER_ID = "inception";

const BASE_URL = "https://api.inceptionlabs.ai/v1";
const API = "openai-completions";
const DEFAULT_CONTEXT_WINDOW = 128_000;
const DEFAULT_MAX_TOKENS = 8_192;
const PER_MILLION = 1_000_000;

interface InceptionModel {
  id?: unknown;
  name?: unknown;
  input_modalities?: unknown;
  context_length?: unknown;
  max_output_length?: unknown;
  pricing?: {
    prompt?: unknown;
    completion?: unknown;
    input_cache_reads?: unknown;
    input_cache_writes?: unknown;
  };
}

/** Pricing is published per token; pi's cost fields are per million tokens. */
function perMillion(value: unknown): number {
  const parsed = typeof value === "string" ? Number(value) : value;
  return typeof parsed === "number" && Number.isFinite(parsed) && parsed > 0
    ? parsed * PER_MILLION
    : 0;
}

function modelConfig(entry: InceptionModel): ProviderModelConfig | null {
  const id = typeof entry?.id === "string" ? entry.id : "";
  if (!id) return null;
  // The Mercury family is a reasoning model served with `reasoning_effort`;
  // the level map below mirrors the documented settings in docs/HERMES.md.
  const reasoning = /mercury/i.test(id);
  const name = typeof entry.name === "string" && entry.name ? entry.name : id;
  return {
    id,
    name: name.replace(/^inception:\s*/i, ""),
    reasoning,
    input: Array.isArray(entry.input_modalities) && entry.input_modalities.includes("image")
      ? ["text", "image"]
      : ["text"],
    cost: {
      input: perMillion(entry.pricing?.prompt),
      output: perMillion(entry.pricing?.completion),
      cacheRead: perMillion(entry.pricing?.input_cache_reads),
      cacheWrite: perMillion(entry.pricing?.input_cache_writes),
    },
    contextWindow:
      typeof entry.context_length === "number" ? entry.context_length : DEFAULT_CONTEXT_WINDOW,
    maxTokens:
      typeof entry.max_output_length === "number" ? entry.max_output_length : DEFAULT_MAX_TOKENS,
    compat: {
      // The Inception gateway rejects pi's `developer` system role.
      supportsDeveloperRole: false,
      ...(reasoning
        ? {
            supportsReasoningEffort: true,
            maxTokensField: "max_completion_tokens",
            thinkingFormat: "openai",
          }
        : {}),
    },
    ...(reasoning
      ? {
          thinkingLevelMap: {
            off: null,
            minimal: "low",
            low: "low",
            medium: "medium",
            high: "high",
            xhigh: "high",
            max: "high",
          },
          samplingParams: { temperature: 0.75 },
        }
      : {}),
  };
}

async function fetchLive(
  credential: RefreshCredential | undefined,
  signal: AbortSignal,
): Promise<ProviderModelConfig[] | null> {
  const key = credential?.key;
  const response = await fetch(`${BASE_URL}/models`, {
    headers: key ? { Authorization: `Bearer ${key}` } : {},
    signal,
  });
  if (!response.ok) return null;
  const payload = (await response.json()) as { data?: unknown };
  if (!Array.isArray(payload.data)) return null;
  const seen = new Set<string>();
  const configs: ProviderModelConfig[] = [];
  for (const entry of payload.data) {
    const config = modelConfig(entry as InceptionModel);
    if (!config || seen.has(config.id)) continue;
    seen.add(config.id);
    configs.push(config);
  }
  return configs.length > 0 ? configs : null;
}

export function registerInceptionProvider(runtime: ModelRuntime): void {
  runtime.registerProvider(INCEPTION_PROVIDER_ID, {
    name: "Inception",
    baseUrl: BASE_URL,
    api: API,
    apiKey: "$INCEPTION_API_KEY",
    models: [],
    refreshModels: createCatalogRefresh({ fetchLive }),
  });
}
