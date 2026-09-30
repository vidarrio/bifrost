#!/usr/bin/env python3
"""Run Archipelago generation with local/user paths pinned to the install, so
custom worlds in custom_worlds/ resolve even under Bifrost's sandbox (same fix
as bifrost_check.py). Rolls Players/*.yaml into output/AP_<seed>.zip.

Invoked by Bifrost (as the bifrost user) with any extra Generate.py args."""
import sys
import os
import runpy

AP = "/srv/archipelago"
sys.path.insert(0, AP)

import Utils

Utils.local_path.cached_path = AP
Utils.user_path.cached_path = AP
os.chdir(AP)

sys.argv = ["Generate.py"] + sys.argv[1:]
runpy.run_path(os.path.join(AP, "Generate.py"), run_name="__main__")
