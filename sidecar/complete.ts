/**
 * One-shot / streaming completion service.
 *
 * Adapted from DocFoo's `agent/model-query.ts`: the Rust pipelines cannot call
 * provider APIs themselves, so every request runs through Pi's own ModelRuntime
 * (`completeSimple` / `streamSimple`) with pi-resolved auth and headers.
 */
import type { ModelRuntime } from "@earendil-works/pi-coding-agent";
import type {
  Context,
  ImageContent,
  TextContent,
  ThinkingLevel,
  UserMessage,
} from "@earendil-works/pi-ai";

const REASONING_LEVELS = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];
const DATA_URL = /^data:([^;,]+);base64,([\s\S]+)$/;

export interface CompleteRequest {
  requestId?: unknown;
  model?: unknown;
  /** Legacy scan shape: one image + one prompt. */
  prompt?: unknown;
  image?: { mimeType?: unknown; data?: unknown } | null;
  /** General shape: OpenAI-style chat messages. */
  messages?: unknown;
  maxTokens?: unknown;
  /** "off" (default), "default" (omit, model decides), or a pi thinking level. */
  reasoning?: unknown;
  /** Raw JSON-schema response format, injected for OpenAI-compatible APIs. */
  responseFormat?: unknown;
  /** When true, use streamSimple and emit stream_delta frames. */
  stream?: unknown;
}

type Emit = (value: unknown) => void;
type Model = Parameters<ModelRuntime["completeSimple"]>[0];

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

/** Map OpenAI-style content (string or parts) to pi content parts. */
function contentParts(content: unknown): (TextContent | ImageContent)[] {
  const parts: (TextContent | ImageContent)[] = [];
  if (typeof content === "string") {
    if (content) parts.push({ type: "text", text: content });
    return parts;
  }
  if (!Array.isArray(content)) return parts;
  for (const part of content) {
    if (!part || typeof part !== "object") continue;
    const p = part as Record<string, unknown>;
    if (p.type === "text" && typeof p.text === "string") {
      parts.push({ type: "text", text: p.text });
    } else if (p.type === "image_url" || p.type === "input_image") {
      const url = (p.image_url as { url?: unknown } | undefined)?.url;
      const match = typeof url === "string" ? DATA_URL.exec(url) : null;
      if (match) parts.push({ type: "image", mimeType: match[1], data: match[2] });
    }
  }
  return parts;
}

/** Build a pi Context from `messages`, or from the legacy prompt+image shape. */
function buildContext(request: CompleteRequest): Context {
  if (Array.isArray(request.messages) && request.messages.length > 0) {
    const messages: UserMessage[] = [];
    let systemPrompt: string | undefined;
    for (const entry of request.messages) {
      if (!entry || typeof entry !== "object") continue;
      const { role, content } = entry as { role?: unknown; content?: unknown };
      if (role === "system" || role === "developer") {
        if (typeof content === "string" && content.trim()) {
          systemPrompt = systemPrompt ? `${systemPrompt}\n\n${content}` : content;
        }
        continue;
      }
      if (role !== "user") continue;
      const parts = contentParts(content);
      if (parts.length > 0) messages.push({ role: "user", content: parts, timestamp: Date.now() });
    }
    if (messages.length === 0) throw new Error("completion request has no usable user message");
    return systemPrompt ? { systemPrompt, messages } : { messages };
  }

  const content: (TextContent | ImageContent)[] = [];
  if (request.image && typeof request.image === "object") {
    const mimeType = typeof request.image.mimeType === "string" ? request.image.mimeType : "";
    const data = typeof request.image.data === "string" ? request.image.data : "";
    if (!mimeType || !data) throw new Error("image payload is missing mimeType or data");
    content.push({ type: "image", mimeType, data });
  }
  content.push({ type: "text", text: String(request.prompt ?? "") });
  return { messages: [{ role: "user", content, timestamp: Date.now() }] };
}

/** "off" (default) | "default" (omit the option) | an explicit level. */
function resolveReasoning(raw: unknown): ThinkingLevel | "off" | "default" {
  if (raw === "default") return "default";
  if (typeof raw === "string" && REASONING_LEVELS.includes(raw)) return raw as ThinkingLevel;
  return "off";
}

export class CompleteService {
  private readonly sessionId = crypto.randomUUID().replace(/-/g, "");
  private readonly inflight = new Map<string, AbortController>();

  constructor(
    private readonly runtime: ModelRuntime,
    private readonly emit: Emit,
  ) {}

  async handle(request: CompleteRequest): Promise<void> {
    const requestId = typeof request?.requestId === "string" ? request.requestId : "";
    try {
      if (!requestId) throw new Error("completion request is missing requestId");
      const text = await this.complete(request, requestId);
      this.emit({ type: "complete_response", requestId, success: true, text });
    } catch (error) {
      this.emit({ type: "complete_response", requestId, success: false, error: errorText(error) });
    }
  }

  cancel(request: { requestId?: unknown }): void {
    const requestId = typeof request?.requestId === "string" ? request.requestId : "";
    if (requestId) this.inflight.get(requestId)?.abort();
  }

  private async complete(request: CompleteRequest, requestId: string): Promise<string> {
    if (typeof request.model !== "string" || !request.model.trim()) {
      throw new Error("completion request is missing model");
    }
    const hasMessages = Array.isArray(request.messages) && request.messages.length > 0;
    if (!hasMessages && (typeof request.prompt !== "string" || !request.prompt.trim())) {
      throw new Error("completion request is missing messages or prompt");
    }

    const reference = request.model.trim();
    const slash = reference.indexOf("/");
    const provider = slash > 0 ? reference.slice(0, slash) : "";
    const modelId = slash > 0 ? reference.slice(slash + 1) : "";
    const model = provider && modelId ? this.runtime.getModel(provider, modelId) : undefined;
    if (!model) throw new Error(`unknown model: ${reference}`);

    const context = buildContext(request);
    const requested = Number(request.maxTokens);
    const modelCap =
      typeof model.maxTokens === "number" && Number.isFinite(model.maxTokens) && model.maxTokens > 0
        ? Math.floor(model.maxTokens)
        : Number.POSITIVE_INFINITY;
    const maxTokens =
      Number.isFinite(requested) && requested > 0
        ? Math.min(Math.floor(requested), modelCap)
        : undefined;
    const reasoning = resolveReasoning(request.reasoning);
    const responseFormat =
      request.responseFormat && typeof request.responseFormat === "object"
        ? request.responseFormat
        : undefined;

    const controller = new AbortController();
    this.inflight.set(requestId, controller);
    try {
      const options: Record<string, unknown> = {
        ...(maxTokens !== undefined ? { maxTokens } : {}),
        ...(reasoning === "default" ? {} : { reasoning }),
        sessionId: this.sessionId,
        signal: controller.signal,
      };
      if (responseFormat && model.api === "openai-completions") {
        options.samplingParams = { response_format: responseFormat };
      }
      const message = await this.invoke(model, context, options, request.stream === true, requestId);
      if (controller.signal.aborted || message.stopReason === "aborted") {
        throw new Error("cancelled");
      }
      if (message.stopReason === "error") {
        throw new Error(message.errorMessage || "the model returned an error");
      }
      const text = message.content
        .filter((part): part is TextContent => part.type === "text")
        .map((part) => part.text)
        .join("");
      if (!text.trim()) {
        throw new Error(`the model returned an empty response (${message.stopReason})`);
      }
      return text;
    } finally {
      this.inflight.delete(requestId);
    }
  }

  private async invoke(
    model: Model,
    context: Context,
    options: Record<string, unknown>,
    stream: boolean,
    requestId: string,
  ) {
    if (!stream) {
      return this.runtime.completeSimple(model, context, options as never);
    }
    const eventStream = this.runtime.streamSimple(model, context, options as never);
    for await (const event of eventStream) {
      if (event.type === "text_delta" && event.delta) {
        this.emit({ type: "stream_delta", requestId, text: event.delta });
      }
    }
    return eventStream.result();
  }
}
