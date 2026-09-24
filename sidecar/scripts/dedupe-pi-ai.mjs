// Remove pi-coding-agent's nested @earendil-works/pi-ai so the sidecar and
// ModelRuntime both resolve to the top-level copy. See DocFoo's
// agent/scripts/dedupe-pi-ai.mjs for the full rationale.
import { existsSync, rmSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const sidecarDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const nested = join(
  sidecarDir,
  "node_modules",
  "@earendil-works",
  "pi-coding-agent",
  "node_modules",
  "@earendil-works",
  "pi-ai",
);

if (existsSync(nested)) {
  rmSync(nested, { recursive: true, force: true });
  console.log("dedupe-pi-ai: removed nested pi-ai");
}
