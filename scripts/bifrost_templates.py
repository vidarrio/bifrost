#!/usr/bin/env python3
"""Bifrost templates helper. Writes Archipelago's own commented template YAML
for each game it can load (every option, its default and its description, as
the Archipelago website's options pages do) into <out dir>, and prints, as
JSON, which file is which game's: {"templates": {game: file name}}.

Run under the install's venv with BIFROST_SEASON_DIR set to a scratch dir: an
empty one for the install's core games, or one whose worlds/ holds the single
custom world to write a template for.

Usage: bifrost_templates.py <out dir>"""
import json
import os
import sys

try:
    import bifrost_ap  # noqa: F401  (pins AP paths; must precede AP imports)
    from Options import generate_yaml_templates
    from Utils import get_file_safe_name
    from worlds import AutoWorldRegister
except Exception as e:  # pragma: no cover
    print(json.dumps({"error": "templates init failed: %s" % e}))
    sys.exit(0)


def main() -> None:
    out = sys.argv[1]
    try:
        generate_yaml_templates(out, generate_hidden=False)
    except Exception as e:
        print(json.dumps({"error": "template generation failed: %s" % e}))
        return
    index = {}
    for game, world in AutoWorldRegister.world_types.items():
        if getattr(world, "hidden", False):
            continue
        name = get_file_safe_name(game) + ".yaml"
        if os.path.isfile(os.path.join(out, name)):
            index[game] = name
    print(json.dumps({"templates": index}))


if __name__ == "__main__":
    main()
