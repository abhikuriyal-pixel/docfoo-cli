#!/usr/bin/env python3
"""Install the docfoo_plugin Hermes plugin (cross-platform).

    python3 hermes/install.py [--workspace DIR] [--model PROVIDER/MODEL] [--bin PATH]
    python3 hermes/install.py --check         # exit 0 when installed
    python3 hermes/install.py --uninstall
    python3 hermes/install.py --no-enable

The plugin lives in ``<hermes-home>/plugins/docfoo_plugin/`` (default
``~/.hermes``). It registers a system prompt section + skill that teach Hermes
to answer library questions with the ``docfoo`` CLI, and wraps
``run_tool_round`` so a ``[[hermes:final]]`` tool result becomes the final
answer. It survives ``hermes update`` because the updater never touches user
plugins.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

PLUGIN_NAME = "docfoo_plugin"
SOURCE = Path(__file__).resolve().parent / "plugin"


def hermes_home(explicit: str | None) -> Path:
    if explicit:
        return Path(explicit).expanduser()
    env = os.environ.get("HERMES_HOME")
    if env:
        return Path(env).expanduser()
    return Path.home() / ".hermes"


def read_config(target: Path) -> dict:
    try:
        data = json.loads((target / "config.json").read_text(encoding="utf-8"))
        return data if isinstance(data, dict) else {}
    except Exception:
        return {}


def find_hermes() -> str | None:
    found = shutil.which("hermes")
    if found:
        return found
    candidate = Path.home() / ".local" / "bin" / "hermes"
    if candidate.is_file() and os.access(candidate, os.X_OK):
        return str(candidate)
    return None


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--hermes-home", help="Hermes home (default: $HERMES_HOME or ~/.hermes)")
    parser.add_argument("--workspace", help="DocFoo workspace (e.g. the desktop app's db folder)")
    parser.add_argument("--model", help="provider/model for kg queries")
    parser.add_argument("--scope", help="default kg scope (a resource folder; omit for the whole library)")
    parser.add_argument("--trigger", help="word that routes a message straight to the CLI (default: dofoq)")
    parser.add_argument("--bin", help="docfoo executable name or path (default: docfoo on PATH)")
    parser.add_argument("--check", action="store_true", help="Only report whether the plugin is installed")
    parser.add_argument("--uninstall", action="store_true", help="Disable and remove the plugin")
    parser.add_argument("--no-enable", action="store_true", help="Copy the plugin but do not enable it")
    args = parser.parse_args()

    target = hermes_home(args.hermes_home) / "plugins" / PLUGIN_NAME
    installed = (target / "plugin.yaml").is_file() and (target / "__init__.py").is_file()

    if args.check:
        print(f"{PLUGIN_NAME}: {'installed' if installed else 'not installed'} ({target})")
        if installed:
            print(json.dumps(read_config(target), indent=2))
        return 0 if installed else 1

    if args.uninstall:
        hermes = find_hermes()
        if hermes:
            subprocess.run([hermes, "plugins", "disable", PLUGIN_NAME], check=False)
        shutil.rmtree(target, ignore_errors=True)
        print(f"removed {target}")
        return 0

    if not SOURCE.is_dir():
        print(f"plugin source not found: {SOURCE}", file=sys.stderr)
        return 2
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copytree(SOURCE, target, dirs_exist_ok=True)

    config = read_config(target)
    if args.workspace is not None:
        config["workspace"] = args.workspace
    if args.model is not None:
        config["model"] = args.model
    if args.scope is not None:
        config["scope"] = args.scope
    if args.trigger is not None:
        config["trigger"] = args.trigger
    if args.bin is not None:
        config["bin"] = args.bin
    (target / "config.json").write_text(json.dumps(config, indent=2) + "\n", encoding="utf-8")
    print(f"installed {target}")
    if config:
        print("config: " + json.dumps(config))

    if args.no_enable:
        return 0
    hermes = find_hermes()
    if not hermes:
        print("warning: `hermes` was not found on PATH; enable it manually:", file=sys.stderr)
        print(f"  hermes plugins enable {PLUGIN_NAME} --no-allow-tool-override", file=sys.stderr)
        return 1
    result = subprocess.run(
        [hermes, "plugins", "enable", PLUGIN_NAME, "--no-allow-tool-override"],
        check=False,
    )
    if result.returncode != 0:
        listed = subprocess.run(
            [hermes, "plugins", "list"], capture_output=True, text=True, check=False
        )
        already = any(
            PLUGIN_NAME in line and "enabled" in line for line in listed.stdout.splitlines()
        )
        if already:
            print(f"{PLUGIN_NAME} is already enabled")
            return 0
        print(f"warning: `hermes plugins enable` exited {result.returncode}", file=sys.stderr)
        return result.returncode
    print(f"enabled {PLUGIN_NAME}; restart the Hermes gateway to load it")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
