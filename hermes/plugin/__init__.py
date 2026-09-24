"""docfoo_plugin — let a DocFoo CLI tool result end the turn.

The DocFoo CLI emits ``[[hermes:final]]`` as the first line of a
``--hermes-final`` tool result. This plugin wraps ``run_tool_round`` so that
such a result becomes the assistant's final message instead of being sent back
to the model for a paraphrase — the same terminate semantics DocFoo's Buddy
uses for its ``query_kg`` tool.

Installed under ``~/.hermes/plugins/docfoo_plugin/`` so ``hermes update``
never touches it. If a future Hermes renames ``run_tool_round`` the wrapper
simply stops applying; the sentinel is then ignored and the turn continues.
"""

from __future__ import annotations

import functools
import logging

logger = logging.getLogger(__name__)

SENTINEL = "[[hermes:final]]"


def extract_final(messages):
    """Return the sentinel payload from the trailing tool results, or None."""
    for message in reversed(messages or []):
        if not isinstance(message, dict) or message.get("role") != "tool":
            break
        content = message.get("content")
        if isinstance(content, str):
            stripped = content.lstrip()
            if stripped.startswith(SENTINEL):
                return stripped[len(SENTINEL):].lstrip("\n")
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


def register(ctx):  # noqa: ARG001 - Hermes passes the plugin context
    install_wrapper()
