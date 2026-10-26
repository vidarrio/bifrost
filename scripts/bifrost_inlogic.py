#!/usr/bin/env python3
"""Bifrost in-logic helper — connects Universal Tracker (headless, no GUI) to
the local AP server as a read-only tracker for ONE slot, captures that slot's
currently-in-logic location names via UT's own callback, and writes them as
JSON to a file. Console stdout/stderr carry UT/AP logging noise and are ignored
by the caller — all real output goes to the JSON file (robust, no scraping).

Usage: bifrost_inlogic.py <slot_name> <password> <host:port> <out_json>
Writes: {"slot","game"?,"count","in_logic":[...],"captured":bool,"error"?}

Runs under the AP venv; AP paths are pinned to the season by bifrost_ap, so UT
finds the season's custom worlds and regenerates from its Players/ YAMLs."""
import sys
import json
import asyncio
import logging

slot, password, connect, outpath = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
result = {"slot": slot, "in_logic": [], "count": 0, "captured": False}


def _write():
    try:
        with open(outpath, "w", encoding="utf-8") as fh:
            json.dump(result, fh)
    except Exception as e:  # pragma: no cover
        sys.stderr.write("write failed: %s\n" % e)


try:
    import bifrost_ap  # pins AP paths; must precede AP imports
    logging.getLogger().setLevel(logging.ERROR)
    import worlds.tracker.TrackerClient as _tcl
    from worlds.tracker.TrackerClient import TrackerGameContext, server_loop
    import worlds.tracker.TrackerCore as _tc
    # Root fix for headless: UT's colour lookup (get_ut_color) imports kvui/kivy —
    # which tries to open a window and sys.exit(1)s with no display — ONLY when
    # `gui_enabled` is True. Kivy is installed here so UT guesses True; force it
    # off (we are headless) so the colour path returns a constant with no kivy
    # import. This is what makes games that reach the colour path (e.g. Starcraft
    # 2) work — the log_all_to_tab stub below alone didn't cover them.
    _tcl.gui_enabled = False
    # Belt-and-suspenders: updateTracker() ends by painting UT's UI tab; we only
    # want the computed list (delivered via the callback), so neuter the paint.
    _tc.TrackerCore.log_all_to_tab = lambda self: None
except Exception as e:
    result["error"] = "init: %s" % e
    _write()
    sys.exit(0)


def cb(locs):
    # UT hands us the in-logic location names as a list[str] (as shown on its tab).
    # Write immediately here: UT's run_cli() tears the process down on stdin-EOF,
    # so an end-of-script write can be preempted — but this callback fires during
    # updateTracker, before any shutdown, so the file is always captured.
    result["in_logic"] = sorted(str(x) for x in locs)
    result["count"] = len(result["in_logic"])
    result["captured"] = True
    _write()
    return True


async def run():
    # print_count=True sets quit_after_update, so UT connects, syncs this slot's
    # received items + checks, runs the logic once, fires our callback, and exits.
    ctx = TrackerGameContext(connect, password, print_count=True)
    ctx.auth = slot
    ctx.tags.add("Bifrost")  # quiet join/leave on Bifrost's AP fork (src/ap.rs QUIET_TAG)
    # Regenerate from the season's YAMLs regardless of any host.yaml override.
    ctx.tracker_core.player_folder_override = bifrost_ap.season_path("Players")
    ctx.set_callback(cb)
    ctx.server_task = asyncio.create_task(server_loop(ctx), name="server loop")
    ctx.run_generator()
    # No run_cli(): the console reader sys.exit()s on stdin-EOF. The watcher in
    # server_loop drives connect->sync->updateTracker, which fires our callback.
    for _ in range(600):  # ~120s cap
        if result["captured"] or ctx.exit_event.is_set():
            break
        # Some worlds' authors set the apworld's `disable_ut` flag (e.g. Starcraft
        # 2): UT connects, refuses to track, and just sits idle forever — the
        # callback never fires. Detect that and exit fast with a clear status
        # instead of waiting out the whole timeout.
        tc = getattr(ctx, "tracker_core", None)
        if tc is not None and getattr(tc, "tracker_disabled", False):
            result["error"] = "disabled"
            break
        await asyncio.sleep(0.2)
    else:
        result.setdefault("error", "timeout")
    try:
        if ctx.game:
            result["game"] = ctx.game
    except Exception:
        pass
    await ctx.shutdown()


try:
    asyncio.run(run())
except Exception as e:
    result.setdefault("error", "run: %s" % e)

_write()
