/**
 * Bundled InferX provider for the DocFoo sidecar.
 *
 * This replaces the third-party `pi-inferx-provider` extension with a local
 * module. The catalog is discovered live from
 * `https://model.inferx.net/endpoints/v1/models` with the configured key, so
 * new InferX endpoints become selectable as soon as they are published: the
 * sidecar refreshes on start when the persisted catalog is older than 24 h,
 * and `docfoo model --list --refresh` forces a fetch. Discovered models are
 * cached per workspace in `<workspace>/.agent/models-store.json`.
 *
 * Response entries are mapped to pi model configs with the small set of vLLM
 * compatibility shims the gateway needs (`system` instead of `developer`,
 * `max_tokens` instead of `max_completion_tokens`, and chat-template thinking
 * control for the DeepSeek/GLM families).
 *
 * Credentials stay external: `INFERX_API_KEY` (e.g. in
 * `~/.config/systemd/user/docfoo-sidecar.env` for the systemd service) or
 * `docfoo auth --set inferx --key …` (stored per workspace in
 * `<workspace>/.agent/auth.json`).
 */
import type { ModelRuntime } from "@earendil-works/pi-coding-agent";
import {
  createCatalogRefresh,
  type ProviderModelConfig,
  type RefreshCredential,
} from "./provider-catalog.js";

export const INFERX_PROVIDER_ID = "inferx";

const BASE_URL = "https://model.inferx.net/endpoints/v1";
const API = "openai-completions";
const DEFAULT_CONTEXT_WINDOW = 128_000;
const DEFAULT_MAX_TOKENS = 16_384;

interface InferxModel {
  id?: unknown;
  name?: unknown;
  context_window?: unknown;
  max_model_len?: unknown;
  max_tokens?: unknown;
}

/**
 * vLLM chat templates generally only know system/user/assistant/tool and
 * read thinking control from `chat_template_kwargs`. DeepSeek and GLM are the
 * families verified to separate reasoning; the Qwen endpoints render their
 * chain of thought into plain `content`, so they must stay non-reasoning —
 * pi would otherwise wait on reasoning deltas that never arrive.
 */
function reasoningFor(id: string): boolean {
  return /deepseek|glm/i.test(id) && !/no[-_ ]?thinking/i.test(id);
}

function modelConfig(entry: InferxModel): ProviderModelConfig | null {
  const id = typeof entry?.id === "string" ? entry.id : "";
  if (!id) return null;
  const reasoning = reasoningFor(id);
  const context =
    typeof entry.context_window === "number"
      ? entry.context_window
      : typeof entry.max_model_len === "number"
        ? entry.max_model_len
        : undefined;
  return {
    id,
    name: typeof entry.name === "string" && entry.name ? entry.name : id,
    reasoning,
    input: ["text"],
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
    contextWindow: context ?? DEFAULT_CONTEXT_WINDOW,
    maxTokens: typeof entry.max_tokens === "number" ? entry.max_tokens : DEFAULT_MAX_TOKENS,
    compat: {
      // vLLM chat templates generally only know system/user/assistant/tool;
      // the `developer` role pi sends for reasoning models gets rejected.
      supportsDeveloperRole: false,
      // vLLM uses max_tokens, not max_completion_tokens.
      maxTokensField: "max_tokens",
      ...(reasoning
        ? {
            // DeepSeek / GLM templates on vLLM read
            // chat_template_kwargs.thinking.
            thinkingFormat: "chat-template",
            chatTemplateKwargs: { thinking: { $var: "thinking.enabled" } },
          }
        : {}),
    },
  };
}

async function fetchLive(
  credential: RefreshCredential | undefined,
  signal: AbortSignal,
): Promise<InferxModel[] | null> {
  const key = credential?.key;
  if (!key) return null;
  const response = await fetch(`${BASE_URL}/models`, {
    headers: { Authorization: `Bearer ${key}` },
    signal,
  });
  if (!response.ok) return null;
  const payload = (await response.json()) as { data?: unknown };
  if (!Array.isArray(payload.data)) return null;
  const seen = new Set<string>();
  const entries: InferxModel[] = [];
  for (const entry of payload.data) {
    const row = entry as InferxModel;
    const id = typeof row?.id === "string" ? row.id : "";
    if (!id || seen.has(id)) continue;
    seen.add(id);
    entries.push(row);
  }
  return entries.length > 0 ? entries : null;
}

export function registerInferxProvider(runtime: ModelRuntime): void {
  runtime.registerProvider(INFERX_PROVIDER_ID, {
    name: "InferX",
    baseUrl: BASE_URL,
    api: API,
    apiKey: "$INFERX_API_KEY",
    models: [],
    refreshModels: createCatalogRefresh({ fetchLive, toConfig: modelConfig }),
  });
}
