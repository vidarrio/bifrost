"""Shared bootstrap for Bifrost's AP helper scripts. Import it FIRST, before any
Archipelago module.

Bifrost runs the helpers with two env vars:

  BIFROST_AP_DIR      the Archipelago install (read-only runtime)
  BIFROST_SEASON_DIR  the season's state dir, laid out as an AP *user dir*:
                      Players/, worlds/, output/, logs/, host.yaml

Pinning AP's local_path to the install and user_path to the season dir makes AP
resolve everything season-specific from the season dir with its own defaults:
custom worlds (worlds/__init__ scans user_path("worlds") when user dir != install
dir), player YAMLs, output, logs and host.yaml (also looked up in the cwd, which
Bifrost sets to the season dir). The pins must be set before `worlds` is first
imported — the world registry computes its folders at import time."""
import os
import sys

AP_DIR = os.environ["BIFROST_AP_DIR"]
SEASON_DIR = os.environ["BIFROST_SEASON_DIR"]
sys.path.insert(0, AP_DIR)

import Utils  # noqa: E402

Utils.local_path.cached_path = AP_DIR
Utils.user_path.cached_path = SEASON_DIR

# Loading worlds registers their setting keys, which AP would persist back into
# host.yaml via an atexit autosave. The helpers only read settings (and a
# sandboxed run may not be able to write), so disable it; host.yaml stays as the
# operator set it.
import settings  # noqa: E402

settings.skip_autosave = True


def season_path(*parts: str) -> str:
    return os.path.join(SEASON_DIR, *parts)


def name_can_change(doc):
    """Whether a player's triggers or linked options can set their `name:`, so that
    what's rolled isn't what the YAML says. Bifrost finds a slot by its name."""
    for key in ("triggers", "linked_options"):
        rules = doc.get(key)
        for rule in rules if isinstance(rules, list) else []:
            options = rule.get("options") if isinstance(rule, dict) else None
            if not isinstance(options, dict):
                continue
            # Root options are under a null or empty category.
            for category in (None, "null", ""):
                root = options.get(category)
                if isinstance(root, dict) and any(k.lstrip("+-") == "name" for k in root if isinstance(k, str)):
                    return True
    return False
