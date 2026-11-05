"""From 0.3.0 the database schema is frozen. Its baseline (`src/db/schema.sql`)
and every migration step the latest release has (`src/db/steps/NNNN.sql`)
stay exactly as that release has them, byte for byte; new steps are new
files. The baseline database the upgrade suite opens
(`tests/e2e/fixtures/baseline.tar.gz`) stays as released too. A unit test
(`the_schema_is_its_files` in `src/db/migrate.rs`) checks that the code uses
exactly these files, in order. Until a 0.3.0 or later release exists there's
nothing to check, and pre-releases (`v0.3.0-rc.1`) don't freeze anything.
Before then, once `Cargo.toml` says 0.3.0 or later (the release pull
request), the fixture has to have been made by that version, as it's about to
be frozen.

Run from the repository's root, with the release tags fetched."""
import os
import re
import sqlite3
import subprocess
import sys
import tarfile
import tempfile

SCHEMA = "src/db/schema.sql"
STEPS = "src/db/steps"
FIXTURE = "tests/e2e/fixtures/baseline.tar.gz"


def git(*args):
    return subprocess.run(["git", *args], capture_output=True, check=True).stdout


def version(tag):
    return tuple(int(x) for x in tag[1:].split("."))


def fail(file, msg):
    print(f"::error file={file}::{msg}")
    sys.exit(1)


def same(tag, path):
    """Whether `path` is in the tree just as `tag` has it."""
    try:
        with open(path, "rb") as f:
            return f.read() == git("show", f"{tag}:{path}")
    except FileNotFoundError:
        return False


tags = [t for t in git("tag", "--list", "v*").decode().split() if re.fullmatch(r"v\d+\.\d+\.\d+", t)]
released = [t for t in tags if version(t) >= (0, 3, 0)]
if not released:
    with open("Cargo.toml") as f:
        cargo = re.search(r'^version = "(\d+\.\d+\.\d+)"$', f.read(), re.M)
    if cargo and version("v" + cargo.group(1)) >= (0, 3, 0):
        with tempfile.TemporaryDirectory() as d, tarfile.open(FIXTURE) as tar:
            tar.extract("data/bifrost.db", d, filter="data")
            with sqlite3.connect(os.path.join(d, "data", "bifrost.db")) as db:
                made = db.execute("SELECT value FROM setting WHERE key = 'bifrost_version'").fetchone()
            db.close()
        if not made or made[0] != cargo.group(1):
            fail(FIXTURE, f"the baseline database was made by Bifrost {made and made[0]}, not "
                          f"{cargo.group(1)}: it's frozen with this release, so remake it from this "
                          "build (tests/e2e/fixtures/make_baseline.py).")
    print("No release from 0.3.0 on yet: the schema isn't frozen.")
    sys.exit(0)
tag = max(released, key=version)

if not same(tag, SCHEMA):
    fail(SCHEMA, f"schema.sql differs from {tag}'s. The baseline is frozen: change the "
                 f"schema with a new file in {STEPS}/ instead.")
steps = [p for p in git("ls-tree", "--name-only", f"{tag}:{STEPS}/").decode().split() if p.endswith(".sql")] \
    if git("ls-tree", tag, f"{STEPS}/") else []
for name in steps:
    path = f"{STEPS}/{name}"
    if not same(tag, path):
        fail(path if os.path.exists(path) else STEPS,
             f"{path} isn't as {tag} released it: a released step is never edited, renamed or removed.")
if git("ls-tree", tag, FIXTURE) and not same(tag, FIXTURE):
    fail(FIXTURE, f"the baseline database isn't as {tag} released it: it stands for what that release "
                  "wrote, so it isn't remade.")
now = len([n for n in os.listdir(STEPS) if n.endswith(".sql")])
print(f"Schema as released in {tag}, with {now - len(steps)} step(s) since.")
