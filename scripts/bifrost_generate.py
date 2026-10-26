#!/usr/bin/env python3
"""Run Archipelago generation for a season: rolls the season's Players/*.yaml
(against its custom worlds — see bifrost_ap) into <season>/output/AP_<seed>.zip.

Invoked by Bifrost with any extra Generate.py args."""
import sys
import os
import runpy

import bifrost_ap  # pins AP paths; must precede AP imports

# Explicit paths win over anything a season host.yaml says.
sys.argv = [
    "Generate.py",
    "--player_files_path", bifrost_ap.season_path("Players"),
    "--outputpath", bifrost_ap.season_path("output"),
] + sys.argv[1:]
runpy.run_path(os.path.join(bifrost_ap.AP_DIR, "Generate.py"), run_name="__main__")
