#!/usr/bin/env python3
"""Install the docfoo_plugin Hermes plugin (cross-platform).

    python3 hermes/install.py                 # install + enable
    python3 hermes/install.py --check         # exit 0 when installed
    python3 hermes/install.py --uninstall
    python3 hermes/install.py --hermes-home DIR --no-enable

The plugin lives in ``<hermes-home>/plugins/docfoo_plugin/`` (default
``~/.hermes``) and wraps ``run_tool_round`` so a DocFoo CLI tool result
starting with ``[[hermes:final]]`` becomes the final answer. It survives
``hermes update`` because the updater never touches user plugins.
"""

from __future__ import annotations

import argparse
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


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--hermes-home", help="Hermes home (default: $HERMES_HOME or ~/.hermes)")
    parser.add_argument("--check", action="store_true", help="Only report whether the plugin is installed")
    parser.add_argument("--uninstall", action="store_true", help="Disable and remove the plugin")
    parser.add_argument("--no-enable", action="store_true", help="Copy the plugin but do not enable it")
    args = parser.parse_args()

    target = hermes_home(args.hermes_home) / "plugins" / PLUGIN_NAME
    installed = (target / "plugin.yaml").is_file() and (target / "__init__.py").is_file()

    if args.check:
        print(f"{PLUGIN_NAME}: {'installed' if installed else 'not installed'} ({target})")
        return 0 if installed else 1

    if args.uninstall:
        hermes = shutil.which("hermes")
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
    print(f"installed {target}")

    if args.no_enable:
        return 0
    hermes = shutil.which("hermes")
    if not hermes:
        print("warning: `hermes` was not found on PATH; enable it manually:", file=sys.stderr)
        print(f"  hermes plugins enable {PLUGIN_NAME} --no-allow-tool-override", file=sys.stderr)
        return 1
    result = subprocess.run(
        [hermes, "plugins", "enable", PLUGIN_NAME, "--no-allow-tool-override"],
        check=False,
    )
    if result.returncode != 0:
        print(f"warning: `hermes plugins enable` exited {result.returncode}", file=sys.stderr)
        return result.returncode
    print(f"enabled {PLUGIN_NAME}; restart the Hermes gateway to load it")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
