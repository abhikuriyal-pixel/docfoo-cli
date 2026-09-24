/**
 * Provider roster, model catalog and credential management.
 *
 * Thin pass-through to Pi's own ModelRuntime (same calls as DocFoo's
 * `agent/providers.ts`): pi persists credentials in `<agentDir>/auth.json`
 * through its own locked store, so this module never touches storage itself.
 */
import type { ModelRuntime } from "@earendil-works/pi-coding-agent";

type Emit = (value: unknown) => void;

interface ModelRow {
  id: string;
  name?: string;
  contextWindow?: number;
  maxTokens?: number;
  api?: string;
}

interface ProviderRow {
  id: string;
  name: string;
  configured: boolean;
  source?: string;
  label?: string;
  modelCount: number;
  models?: ModelRow[];
}

function errorText(error: unknown): string {
  if (!(error instanceof Error)) return String(error);
  const parts: string[] = [];
  let current: unknown = error;
  for (let depth = 0; depth < 4 && current instanceof Error; depth++) {
    const message = current.message || current.name;
    if (message && !parts.some((part) => part.includes(message))) parts.push(message);
    current = (current as { cause?: unknown }).cause;
  }
  return parts.join(" — ") || String(error);
}

export class ProviderService {
  constructor(
    private readonly runtime: ModelRuntime,
    private readonly emit: Emit,
  ) {}

  async list(requestId: string, refresh = false): Promise<void> {
    try {
      if (refresh) {
        try {
          await this.runtime.refresh({ allowNetwork: true, force: true });
        } catch (error) {
          process.stderr.write(`docfoo-agent: model refresh failed: ${errorText(error)}\n`);
        }
      }
      const providers = await this.rows(true);
      this.emit({ type: "models_response", requestId, success: true, providers });
    } catch (error) {
      this.emit({ type: "models_response", requestId, success: false, error: errorText(error) });
    }
  }

  async status(requestId: string, provider?: string): Promise<void> {
    try {
      const providers = (await this.rows(false)).filter((row) => !provider || row.id === provider);
      this.emit({ type: "auth_status_response", requestId, success: true, providers });
    } catch (error) {
      this.emit({ type: "auth_status_response", requestId, success: false, error: errorText(error) });
    }
  }

  async set(requestId: string, provider: string, key: string): Promise<void> {
    try {
      if (!provider) throw new Error("auth set is missing provider");
      if (!key) throw new Error("auth set is missing key");
      await this.runtime.login(provider, "api_key", {
        signal: new AbortController().signal,
        prompt: async () => key,
        notify: () => {},
      } as never);
      this.emit({ type: "auth_set_response", requestId, success: true });
    } catch (error) {
      this.emit({ type: "auth_set_response", requestId, success: false, error: errorText(error) });
    }
  }

  async logout(requestId: string, provider: string): Promise<void> {
    try {
      if (!provider) throw new Error("auth logout is missing provider");
      await this.runtime.logout(provider);
      this.emit({ type: "auth_logout_response", requestId, success: true });
    } catch (error) {
      this.emit({ type: "auth_logout_response", requestId, success: false, error: errorText(error) });
    }
  }

  private async rows(includeModels: boolean): Promise<ProviderRow[]> {
    await this.runtime.getAvailable();
    return this.runtime.getProviders().map((provider) => {
      const status = this.runtime.getProviderAuthStatus(provider.id);
      const models = includeModels
        ? provider.getModels().map((model) => ({
            id: model.id,
            ...(model.name ? { name: model.name } : {}),
            ...(typeof model.contextWindow === "number"
              ? { contextWindow: model.contextWindow }
              : {}),
            ...(typeof model.maxTokens === "number" ? { maxTokens: model.maxTokens } : {}),
            ...(model.api ? { api: model.api } : {}),
          }))
        : undefined;
      return {
        id: provider.id,
        name: provider.name,
        configured: status.configured,
        ...(status.source ? { source: status.source } : {}),
        ...(status.label ? { label: status.label } : {}),
        modelCount: includeModels ? (models?.length ?? 0) : provider.getModels().length,
        ...(models ? { models } : {}),
      };
    });
  }
}
