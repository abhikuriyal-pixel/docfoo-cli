/**
 * DocFoo CLI model-completion sidecar.
 *
 * A stripped Pi runtime exposed over JSONL: no agent loop, no sessions, no
 * tools. The Rust CLI writes one request per line on stdin; this process
 * answers with tagged frames on stdout (see PLAN.md §3.4).
 *
 * Environment:
 *   DOCFOO_AGENT_DIR   Pi config dir (auth.json, models.json, models-store.json)
 */
import { join } from "node:path";
import { createInterface } from "node:readline";
import { ModelRuntime } from "@earendil-works/pi-coding-agent";
import { CompleteService } from "./complete.js";
import { ProviderService } from "./providers.js";

const AGENT_DIR = process.env.DOCFOO_AGENT_DIR || join(process.cwd(), ".agent");

/** Serialized stdout writer so concurrent handlers never interleave a line. */
let outputTail = Promise.resolve();
export function emit(value: unknown): void {
  const line = JSON.stringify(value) + "\n";
  outputTail = outputTail
    .then(() => new Promise<void>((done) => process.stdout.write(line, () => done())))
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

  const complete = new CompleteService(runtime, emit);
  const providers = new ProviderService(runtime, emit);

  emit({ type: "ready", version: "0.1.0" });

  const lines = createInterface({ input: process.stdin, terminal: false });
  lines.on("line", (line) => {
    void handleLine(line);
  });
  lines.on("close", () => process.exit(0));

  async function handleLine(line: string): Promise<void> {
    const trimmed = line.trim();
    if (!trimmed) return;
    let request: Record<string, unknown>;
    try {
      request = JSON.parse(trimmed) as Record<string, unknown>;
    } catch {
      return; // malformed frames are ignored
    }
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
          await providers.list(requestId);
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
}

main().catch((error) => {
  emit({ type: "fatal", message: errorText(error) });
  process.exit(1);
});
