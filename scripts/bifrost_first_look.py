#!/usr/bin/env python3
"""Bifrost's first look at a YAML whose world isn't approved yet: what it says,
read with Archipelago's own parser, without loading any world. Reads a YAML file
path as argv[1], prints a JSON result:

  {"players": [{"name": str|null, "games": [str]}], "error": str|null}

`name` is null when it isn't plain text."""
import json
import sys

import bifrost_ap  # noqa: F401  (pins AP paths; must precede AP imports)
from Utils import parse_yamls

MAX_GAMES = 100


def games_of(game):
    """The games a `game:` can roll: a name, names with weights, or a list."""
    if isinstance(game, str):
        return [game]
    if isinstance(game, dict):
        names = (g for g, weight in game.items() if weight)
    elif isinstance(game, list):
        names = game
    else:
        return []
    return [g for g in names if isinstance(g, str)][:MAX_GAMES]


def main() -> None:
    with open(sys.argv[1], encoding="utf-8-sig") as fh:
        raw = fh.read()
    out = {"players": [], "error": None}
    try:
        for doc in parse_yamls(raw):
            if doc is None:
                continue
            if not isinstance(doc, dict):
                out["error"] = "it isn't a set of options"
                break
            name = doc.get("name")
            out["players"].append(
                {
                    "name": name if isinstance(name, str) else None,
                    "games": games_of(doc.get("game")),
                }
            )
    except Exception as e:
        out["error"] = str(e)[:500]
    print(json.dumps(out))


if __name__ == "__main__":
    main()
