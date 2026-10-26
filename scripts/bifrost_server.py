#!/usr/bin/env python3
"""Run the Archipelago MultiServer for a season, with AP's paths pinned to the
season dir (see bifrost_ap) so it reads the season's host.yaml and custom
worlds and writes its save + logs there.

Invoked by Bifrost's AP server supervisor with MultiServer.py's own args
(multidata path, --host/--port, passwords). Console commands arrive on stdin."""
import os
import runpy
import sys

import bifrost_ap  # pins AP paths; must precede AP imports

sys.argv = ["MultiServer.py"] + sys.argv[1:]
runpy.run_path(os.path.join(bifrost_ap.AP_DIR, "MultiServer.py"), run_name="__main__")
