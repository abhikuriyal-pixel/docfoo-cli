/**
 * DocFoo CLI model-completion sidecar.
 *
 * A stripped Pi runtime exposed over JSONL: no agent loop, no sessions, no
 * tools. Two transports:
 *
 *   - stdio (default): one request per line on stdin, tagged frames on stdout
 *   - `--socket PATH`: the same frames over a Unix domain socket, so a
 *     long-lived daemon skips process start and keeps provider connections warm
 *
 * Environment:
 *   DOCFOO_AGENT_DIR   Pi config dir (auth.json, models.json, models-store.json)
 */
import { unlinkSync } from "node:fs";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { ModelRuntime } from "@earendil-works/pi-coding-agent";
import { CompleteService } from "./complete.js";
import { INCEPTION_PROVIDER_ID, registerInceptionProvider } from "./inception.js";
import { INFERX_PROVIDER_ID, registerInferxProvider } from "./inferx.js";
import { ProviderService } from "./providers.js";

const AGENT_DIR = process.env.DOCFOO_AGENT_DIR || join(process.cwd(), ".agent");

/** Anything that can receive one protocol line. */
interface Target {
  write(chunk: string): unknown;
}

/** Serialized stdout writer so concurrent handlers never interleave a line. */
let outputTail = Promise.resolve();
/** Where frames without a requestId (ready/fatal) go; null in socket mode. */
let fallbackTarget: Target | null = null;
/** requestId -> connection that owns it, so socket mode can serve several callers. */
const owners = new Map<string, Target>();

export function emit(value: unknown): void {
  const frame = value as { requestId?: unknown };
  const requestId = typeof frame.requestId === "string" ? frame.requestId : "";
  const target = (requestId ? owners.get(requestId) : undefined) ?? fallbackTarget;
  if (!target) return; // no sink, or the owning connection already closed
  const line = JSON.stringify(value) + "\n";
  outputTail = outputTail
    .then(
      () =>
        new Promise<void>((done) => {
          target.write(line);
          done();
        }),
    )
    .catch(() => {});
}

export function errorText(error: unknown): string {
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

function argValue(name: string): string | undefined {
  const index = process.argv.indexOf(name);
  return index >= 0 ? process.argv[index + 1] : undefined;
}

function parseRequest(line: string): Record<string, unknown> | null {
  const trimmed = line.trim();
  if (!trimmed) return null;
  try {
    return JSON.parse(trimmed) as Record<string, unknown>;
  } catch {
    return null; // malformed frames are ignored
  }
}

async function main(): Promise<void> {
  // Same discovery as the desktop app: built-in catalogs + models-store.json
  // + custom providers in models.json, credentials in auth.json.
  const runtime = await ModelRuntime.create({
    authPath: join(AGENT_DIR, "auth.json"),
    modelsPath: join(AGENT_DIR, "models.json"),
    modelsStorePath: join(AGENT_DIR, "models-store.json"),
    allowModelNetwork: true,
    refreshOnCreate: true,
  });

  // Bundled provider modules: OpenAI-compatible providers pi's built-in
  // catalog does not ship (Inception, InferX). Their model catalogs are
  // discovered from each provider's /v1/models and cached per workspace.
  const bundledProviders = [
    [INCEPTION_PROVIDER_ID, registerInceptionProvider],
    [INFERX_PROVIDER_ID, registerInferxProvider],
  ] as const;
  const registeredProviderIds: string[] = [];
  for (const [providerId, register] of bundledProviders) {
    try {
      register(runtime);
      registeredProviderIds.push(providerId);
    } catch (error) {
      process.stderr.write(`docfoo-agent: ${providerId} provider failed: ${errorText(error)}\n`);
    }
  }

  // Discover the bundled provider catalogs lazily, when models are listed.
  // Refreshing at registration would race the runtime's own registration
  // refresh and abort the in-flight fetch. No network when the persisted
  // catalog is recent; `--refresh` bypasses the freshness check.
  async function discoverBundledCatalogs(): Promise<void> {
    if (registeredProviderIds.length === 0) return;
    try {
      const result = await runtime.refresh({
        allowNetwork: true,
        providers: registeredProviderIds,
      });
      const providers = runtime.getProviders();
      for (const [providerId, error] of result.errors) {
        const provider = providers.find((candidate) => candidate.id === providerId);
        // A missing key only matters when there is no persisted catalog to
        // fall back on; otherwise stay quiet on every list request.
        if (provider && provider.getModels().length === 0) {
          process.stderr.write(`docfoo-agent: ${providerId}: ${error.message}\n`);
        }
      }
    } catch (error) {
      process.stderr.write(`docfoo-agent: provider catalog refresh failed: ${errorText(error)}\n`);
    }
  }

  const complete = new CompleteService(runtime, emit);
  const providers = new ProviderService(runtime, emit);

  async function handle(request: Record<string, unknown>): Promise<void> {
    const requestId = typeof request.requestId === "string" ? request.requestId : "";
    try {
      switch (request.type) {
        case "complete":
          await complete.handle(request);
          break;
        case "cancel":
          complete.cancel(request);
          break;
        case "models":
          if (request.refresh !== true) await discoverBundledCatalogs();
          await providers.list(requestId, request.refresh === true);
          break;
        case "auth_status":
          await providers.status(
            requestId,
            typeof request.provider === "string" ? request.provider : undefined,
          );
          break;
        case "auth_set":
          await providers.set(requestId, String(request.provider ?? ""), String(request.key ?? ""));
          break;
        case "auth_logout":
          await providers.logout(requestId, String(request.provider ?? ""));
          break;
        case "ping":
          emit({ type: "pong", requestId });
          break;
        default:
          emit({
            type: "error_response",
            requestId,
            success: false,
            error: `unknown request type "${String(request.type)}"`,
          });
      }
    } catch (error) {
      emit({ type: "error_response", requestId, success: false, error: errorText(error) });
    }
  }

  function dispatch(socket: Target | null, line: string): void {
    const request = parseRequest(line);
    if (!request) return;
    const requestId = typeof request.requestId === "string" ? request.requestId : "";
    if (socket && requestId) owners.set(requestId, socket);
    void handle(request).finally(() => {
      if (requestId) owners.delete(requestId);
    });
  }

  const socketPath = argValue("--socket");
  if (socketPath) {
    try {
      unlinkSync(socketPath); // drop a stale socket from a previous run
    } catch {
      /* no stale file */
    }
    const buffers = new WeakMap<object, string>();
    Bun.listen({
      unix: socketPath,
      socket: {
        open(ws) {
          buffers.set(ws, "");
        },
        data(ws, data) {
          const text = (buffers.get(ws) ?? "") + data.toString();
          const lines = text.split("\n");
          buffers.set(ws, lines.pop() ?? "");
          for (const line of lines) dispatch(ws as unknown as Target, line);
        },
        close(ws) {
          buffers.delete(ws);
          for (const [id, target] of owners) {
            if (target === (ws as unknown as Target)) owners.delete(id);
          }
        },
        drain() {},
      },
    });
    process.stderr.write(`docfoo-agent listening on ${socketPath}\n`);
    return;
  }

  fallbackTarget = process.stdout;
  emit({ type: "ready", version: "0.1.3" });

  const lines = createInterface({ input: process.stdin, terminal: false });
  lines.on("line", (line) => dispatch(null, line));
  lines.on("close", () => process.exit(0));
}

main().catch((error) => {
  emit({ type: "fatal", message: errorText(error) });
  process.exit(1);
});
