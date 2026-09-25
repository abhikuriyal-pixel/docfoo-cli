/**
 * Model catalog helpers plus the fixed reasoning cycle.
 *
 * The picker is session-only: chosen keys travel with each query, and nothing
 * here ever persists to the shared model-selection.json.
 */

/** Fixed bolt cycle; pi clamps levels the selected model does not support. */
export const REASONING_LEVELS = ['off', 'minimal', 'low', 'medium', 'high', 'max'];

function finite(value) {
  const number = typeof value === 'number' ? value : Number(value);
  return Number.isFinite(number) ? number : undefined;
}

/** Coerce the `/api/models` payload into `{id, name, configured, models[]}`. */
export function coerceModels(payload) {
  const providers = payload && Array.isArray(payload.providers) ? payload.providers : [];
  return providers.flatMap((provider) => {
    if (!provider || typeof provider !== 'object' || typeof provider.id !== 'string') return [];
    const models = Array.isArray(provider.models)
      ? provider.models.flatMap((model) => {
          if (!model || typeof model !== 'object' || typeof model.id !== 'string' || !model.id) return [];
          return [{
            id: model.id,
            name: typeof model.name === 'string' && model.name ? model.name : undefined,
            contextWindow: finite(model.contextWindow),
            maxTokens: finite(model.maxTokens),
            api: typeof model.api === 'string' ? model.api : undefined,
          }];
        })
      : [];
    return [{
      id: provider.id,
      name: typeof provider.name === 'string' && provider.name ? provider.name : provider.id,
      configured: provider.configured === true,
      label: typeof provider.label === 'string' ? provider.label : undefined,
      modelCount: Number.isFinite(Number(provider.modelCount)) ? Number(provider.modelCount) : models.length,
      models,
    }];
  });
}

/** `provider/model`, tolerating model ids that already include the provider. */
export function modelKey(provider, model) {
  if (!model || typeof model.id !== 'string') return '';
  return model.id.includes('/') ? model.id : `${provider.id}/${model.id}`;
}

export function modelLabel(key, catalog = []) {
  if (!key) return 'No model selected';
  for (const provider of catalog) {
    for (const model of provider.models) {
      if (modelKey(provider, model) === key) return model.name || model.id;
    }
  }
  return key;
}

export function providerLabel(key, catalog = []) {
  if (!key) return '';
  const providerId = key.includes('/') ? key.slice(0, key.indexOf('/')) : key;
  const provider = catalog.find((entry) => entry.id === providerId);
  return (provider && (provider.label || provider.name)) || providerId;
}

/**
 * Keep the server's `default` sentinel (model default reasoning) until the
 * user cycles the bolt, which then starts at `off`.
 */
export function normalizeReasoning(value) {
  const lower = String(value ?? '').toLowerCase();
  if (lower === 'default') return 'default';
  return REASONING_LEVELS.includes(lower) ? lower : 'off';
}

export function nextReasoning(value) {
  const current = normalizeReasoning(value);
  const index = REASONING_LEVELS.indexOf(current);
  return REASONING_LEVELS[(index + 1) % REASONING_LEVELS.length];
}
