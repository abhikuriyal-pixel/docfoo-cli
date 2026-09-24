"""docfoo_plugin — DocFoo CLI integration for Hermes.

Three parts:

1. A **system prompt section** telling the model to answer questions about the
   user's documents with the ``docfoo`` CLI and relay the output verbatim.
2. A plugin-scoped **skill** (``docfoo_plugin:docfoo``) with the full command
   reference, loaded on demand.
3. A ``run_tool_round`` wrapper so a tool result starting with
   ``[[hermes:final]]`` becomes the final answer instead of being sent back to
   the model for a paraphrase — the same terminate semantics DocFoo's Buddy
   uses for ``query_kg``.

Installed under ``~/.hermes/plugins/docfoo_plugin/`` so ``hermes update``
never touches it. If a future Hermes renames ``run_tool_round``, the wrapper
simply stops applying; the sentinel is ignored and the turn still completes.
"""

from __future__ import annotations

import asyncio
import functools
import json
import logging
import re
import subprocess
from pathlib import Path

logger = logging.getLogger(__name__)

SENTINEL = "[[hermes:final]]"
PLUGIN_DIR = Path(__file__).resolve().parent
CONFIG_PATH = PLUGIN_DIR / "config.json"
SKILL_PATH = PLUGIN_DIR / "SKILL.md"
SECTION_ID = "docfoo.cli"
DEFAULT_BIN = "docfoo"


def load_config() -> dict:
    """Read ``config.json`` written by ``hermes/install.py`` (missing = defaults)."""
    try:
        data = json.loads(CONFIG_PATH.read_text(encoding="utf-8"))
    except Exception:
        data = {}
    if not isinstance(data, dict):
        data = {}
    return {
        "bin": str(data.get("bin") or DEFAULT_BIN).strip() or DEFAULT_BIN,
        "workspace": str(data.get("workspace") or "").strip(),
        "model": str(data.get("model") or "").strip(),
        "scope": str(data.get("scope") or "").strip(),
    }


def query_command(cfg: dict) -> str:
    parts = [cfg["bin"]]
    if cfg["workspace"]:
        parts += ["--workspace", cfg["workspace"]]
    parts += ["kg", "--query", '"<question>"']
    if cfg["scope"]:
        parts += ["--scope", cfg["scope"]]
    if cfg["model"]:
        parts += ["--model", cfg["model"]]
    parts += ["--format", "slack", "--hermes-final"]
    return " ".join(parts)


# --- direct keyword route ---------------------------------------------------
# A message containing the word "docfoo" is answered by the CLI directly: the
# gateway hook below runs the command and sends the result, so the message never
# reaches the model (saves the whole first LLM call).

DOCFOO_WORD = re.compile(r"(?i)\bdocfoo\b")


def _strip_keyword(text: str) -> str:
    cleaned = DOCFOO_WORD.sub("", text, count=1)
    return cleaned.strip(" \t\r\n,;:-\u2014\u2013")


def _run_cli(cfg: dict, question: str) -> str:
    """Run ``docfoo kg --query`` and return the answer with the sentinel stripped."""
    argv = [cfg["bin"]]
    if cfg["workspace"]:
        argv += ["--workspace", cfg["workspace"]]
    argv += ["kg", "--query", question]
    if cfg["scope"]:
        argv += ["--scope", cfg["scope"]]
    if cfg["model"]:
        argv += ["--model", cfg["model"]]
    argv += ["--format", "slack", "--hermes-final"]
    try:
        proc = subprocess.run(argv, capture_output=True, text=True, timeout=300)
    except Exception as exc:
        return f"docfoo failed: {exc}"
    out = (proc.stdout or "").lstrip()
    if out.startswith(SENTINEL):
        out = out[len(SENTINEL):].lstrip("\n")
    if proc.returncode != 0 or not out.strip():
        detail = (proc.stderr or "").strip() or "no output"
        return f"docfoo failed: {detail}"
    return out


def _metadata_for(gateway, source):
    builder = getattr(gateway, "_thread_metadata_for_source", None)
    if callable(builder):
        try:
            meta = builder(source, None)
            if isinstance(meta, dict):
                return meta
        except Exception:
            pass
    thread_id = getattr(source, "thread_id", None)
    return {"thread_id": thread_id} if thread_id else None


async def _answer_direct(gateway, event, question: str) -> None:
    cfg = load_config()
    source = event.source
    adapter = (getattr(gateway, "adapters", None) or {}).get(source.platform)
    if adapter is None:
        logger.warning("docfoo_plugin: no adapter for %s; falling back", source.platform)
        return
    metadata = _metadata_for(gateway, source)
    typing = getattr(adapter, "send_typing", None)
    if callable(typing):
        try:
            await typing(source.chat_id, metadata=metadata)
        except Exception:
            pass
    answer = await asyncio.to_thread(_run_cli, cfg, question)
    try:
        media_files, cleaned = adapter.extract_media(answer)
    except Exception:
        media_files, cleaned = [], answer
    filter_paths = getattr(adapter, "filter_media_delivery_paths", None)
    if callable(filter_paths):
        try:
            media_files = filter_paths(media_files)
        except Exception:
            pass
    if cleaned.strip():
        try:
            await adapter.send(chat_id=source.chat_id, content=cleaned, metadata=metadata)
        except Exception as exc:
            logger.warning("docfoo_plugin: direct send failed: %s", exc)
    for media_path, _is_voice in (media_files or []):
        sender = getattr(adapter, "send_image_file", None)
        if callable(sender):
            try:
                await sender(chat_id=source.chat_id, image_path=media_path, metadata=metadata)
            except Exception as exc:
                logger.warning("docfoo_plugin: direct image send failed: %s", exc)


def _on_pre_gateway_dispatch(event=None, gateway=None, session_store=None, **kwargs):
    """Route messages containing "docfoo" straight to the CLI (no model call)."""
    text = (getattr(event, "text", "") or "").strip()
    if not text or text.startswith("/") or not DOCFOO_WORD.search(text):
        return None
    source = getattr(event, "source", None)
    if source is None or gateway is None:
        return None
    authorized = getattr(gateway, "_is_user_authorized_for_source", None)
    if callable(authorized):
        try:
            if not authorized(source):
                return None
        except Exception:
            return None
    try:
        loop = asyncio.get_running_loop()
    except RuntimeError:
        return None
    question = _strip_keyword(text) or text
    loop.create_task(_answer_direct(gateway, event, question))
    logger.info("docfoo_plugin: direct keyword route (%d chars)", len(question))
    return {"action": "skip", "reason": "docfoo keyword route"}


def section_content(cfg: dict) -> str:
    if cfg["scope"]:
        scope_note = (
            f"The configured scope is `{cfg['scope']}`; for a different document pass "
            "`--scope <resource-folder>` instead (list them with `docfoo resources --list --json`), "
            "or omit `--scope` for the whole library. "
        )
    else:
        scope_note = (
            "Pass `--scope <resource-folder>` to focus on one document "
            "(`docfoo resources --list --json` lists them). "
        )
    return (
        "## DocFoo document library\n"
        "The user's own documents (papers, PDFs, notes) live in a DocFoo library that you can "
        "query with the `docfoo` CLI through the terminal tool. For any question about the "
        "user's documents, run:\n\n"
        f"    {query_command(cfg)}\n\n"
        "stdout is the complete, Slack-ready answer (figures as `MEDIA:` lines, tables, exact "
        "citations, `Sources:`). Output it verbatim as your final message: do not paraphrase, "
        "summarize, translate, reorder or add commentary; keep `MEDIA:` lines exactly where they "
        "are. Do not answer from memory or from earlier context — run the command for each "
        "question. If the command fails, report its error text.\n"
        + scope_note
        + "For other library tasks (outline, search, read a section, notes, backups, indexing, "
        'collections), load the full reference: skill_view("docfoo_plugin:docfoo").'
    )


def _sentinel_payload(text):
    if not isinstance(text, str):
        return None
    stripped = text.lstrip()
    if stripped.startswith(SENTINEL):
        return stripped[len(SENTINEL):].lstrip("\n")
    return None


def extract_final(messages):
    """Return the sentinel payload from the trailing tool results, or None.

    Handles plain strings, structured content blocks, and Hermes' terminal
    envelope ``{"output": "...", "exit_code": 0, "error": null}``.
    """
    for message in reversed(messages or []):
        if not isinstance(message, dict) or message.get("role") != "tool":
            break
        content = message.get("content")
        if isinstance(content, list):
            content = "\n".join(
                block.get("text", "")
                for block in content
                if isinstance(block, dict) and block.get("type") in ("text", "output_text")
            )
        payload = _sentinel_payload(content)
        if payload is not None:
            return payload
        if isinstance(content, str) and content.lstrip().startswith("{"):
            try:
                data = json.loads(content.lstrip())
            except Exception:
                data = None
            if isinstance(data, dict):
                for key in ("output", "stdout", "content"):
                    payload = _sentinel_payload(data.get(key))
                    if payload is not None:
                        return payload
    return None


def _finish(verdict, text, agent):
    """Turn a continuing verdict into the final answer."""
    verdict.action = "break"
    verdict.final_response = text
    verdict._turn_exit_reason = "tool_final"
    try:
        from agent.message_metadata import append_message

        append_message(verdict.messages, {"role": "assistant", "content": text})
    except Exception:
        try:
            verdict.messages.append({"role": "assistant", "content": text})
        except Exception:
            pass
    callback = getattr(agent, "stream_delta_callback", None) if agent is not None else None
    if callback is not None:
        try:
            callback(text)
            callback(None)
        except Exception:
            pass
    logger.info("docfoo_plugin: returning the DocFoo tool result as the final answer")


def install_wrapper() -> bool:
    """Wrap ``run_tool_round`` in the agent loop. Idempotent per process."""
    try:
        import agent.turn_tool_round as turn_tool_round
    except Exception as exc:
        logger.warning("docfoo_plugin: cannot import agent.turn_tool_round: %s", exc)
        return False

    original = getattr(turn_tool_round, "run_tool_round", None)
    if original is None:
        logger.warning("docfoo_plugin: agent.turn_tool_round.run_tool_round not found")
        return False
    if getattr(original, "_docfoo_final_wrapper", False):
        return True

    @functools.wraps(original)
    def run_tool_round(*args, **kwargs):
        verdict = original(*args, **kwargs)
        if getattr(verdict, "action", None) == "continue":
            text = extract_final(getattr(verdict, "messages", None))
            if text is not None:
                agent = args[0] if args else kwargs.get("agent")
                _finish(verdict, text, agent)
        return verdict

    run_tool_round._docfoo_final_wrapper = True
    turn_tool_round.run_tool_round = run_tool_round

    # The loop may have imported the function already; patch its global too.
    # If it imports later, it picks up the wrapped attribute above.
    import sys

    loop = sys.modules.get("agent.conversation_loop")
    if loop is not None and hasattr(loop, "run_tool_round"):
        loop.run_tool_round = run_tool_round
    return True


def register(ctx):
    cfg = load_config()
    try:
        ctx.register_system_prompt_section(SECTION_ID, section_content(cfg))
    except Exception as exc:
        logger.warning("docfoo_plugin: system prompt section registration failed: %s", exc)
    try:
        if SKILL_PATH.is_file():
            ctx.register_skill(
                "docfoo",
                SKILL_PATH,
                description=(
                    "DocFoo CLI reference: query the user's document library with "
                    "`docfoo kg --query`, browse resources, notes, backups, indexing."
                ),
            )
    except Exception as exc:
        logger.warning("docfoo_plugin: skill registration failed: %s", exc)
    try:
        ctx.register_hook("pre_gateway_dispatch", _on_pre_gateway_dispatch)
    except Exception as exc:
        logger.warning("docfoo_plugin: gateway dispatch hook registration failed: %s", exc)
    install_wrapper()
