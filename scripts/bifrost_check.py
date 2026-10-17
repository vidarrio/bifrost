#!/usr/bin/env python3
"""Bifrost YAML checker helper — validates an Archipelago options file against
the installed worlds, using AP's own roll_settings (the same logic the WebHost
/check page uses). Reads a YAML file path as argv[1], prints a JSON result:

  {"ok": bool, "players": [{"name","game","ok","error"?}], "errors": [str]}

Runs under the AP venv; imports only Generate + Utils (no Flask/WebHost)."""
import sys
import json
import os

AP_DIR = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, AP_DIR)

try:
    # Pin AP's local_path AND user_path to the install dir *before* importing the
    # world registry. Otherwise, when the install is read-only (as it is for the
    # sandboxed bifrost service), user_path() falls back to a home dir that
    # diverges from local_path(), and worlds/__init__ then scans custom_worlds
    # in the wrong place — silently dropping every .apworld (see worlds/__init__).
    import Utils
    Utils.local_path.cached_path = AP_DIR
    Utils.user_path.cached_path = AP_DIR

    # Loading the worlds registers their setting keys, which AP would persist to
    # host.yaml via an atexit autosave — a write the read-only sandbox refuses.
    # The checker only reads settings, so disable the autosave.
    import settings
    settings.skip_autosave = True

    from Generate import roll_settings, PlandoOptions
    from Utils import parse_yamls
except Exception as e:  # pragma: no cover
    print(json.dumps({"ok": False, "players": [], "errors": ["checker init failed: %s" % e]}))
    sys.exit(0)

PLANDO = PlandoOptions.from_set({"bosses", "items", "connections", "texts"})


def main() -> None:
    path = sys.argv[1]
    with open(path, encoding="utf-8-sig") as fh:
        raw = fh.read()

    out = {"ok": True, "players": [], "errors": []}
    try:
        docs = tuple(parse_yamls(raw))
    except Exception as e:
        out["ok"] = False
        out["errors"].append("YAML parse error: %s" % e)
        print(json.dumps(out))
        return

    for doc in docs:
        if doc is None:
            continue
        name = str(doc.get("name", "?"))
        game = str(doc.get("game", "?"))
        try:
            settings = roll_settings(doc, plando_options=PLANDO)
            game = getattr(settings, "game", None) or game
            out["players"].append({"name": name, "game": str(game), "ok": True})
        except Exception as e:
            cause = " - %s" % e.__cause__ if getattr(e, "__cause__", None) else ""
            msg = "%s%s" % (e, cause)
            out["players"].append({"name": name, "game": game, "ok": False, "error": msg})
            out["errors"].append("%s: %s" % (name, msg))
            out["ok"] = False

    print(json.dumps(out))


if __name__ == "__main__":
    main()
