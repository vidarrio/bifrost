"""From 0.3.0 the database schema is frozen. `src/db/schema.sql` stays as the
latest release has it, and that release's migration steps stay, in order, at
the start of today's: their hashes (`STEP_SHA256` in `src/db/migrate.rs`,
which a unit test checks against the steps themselves) are a prefix of
today's. Until a 0.3.0 or later release exists there's nothing to check.

Run from the repository's root, with the release tags fetched."""
import re
import subprocess
import sys

SCHEMA = "src/db/schema.sql"
MIGRATE = "src/db/migrate.rs"


def git(*args):
    return subprocess.run(["git", *args], capture_output=True, text=True, check=True).stdout


def version(tag):
    return tuple(int(x) for x in tag[1:].split("."))


def steps(text):
    m = re.search(r"// schema-steps-begin\n(.*?)// schema-steps-end", text, re.S)
    return re.findall(r'"([0-9a-f]{64})"', m.group(1)) if m else None


def fail(file, msg):
    print(f"::error file={file}::{msg}")
    sys.exit(1)


tags = [t for t in git("tag", "--list", "v*").split() if re.fullmatch(r"v\d+\.\d+\.\d+", t)]
released = [t for t in tags if version(t) >= (0, 3, 0)]
if not released:
    print("No release from 0.3.0 on yet: the schema isn't frozen.")
    sys.exit(0)
tag = max(released, key=version)

with open(SCHEMA) as f:
    if f.read() != git("show", f"{tag}:{SCHEMA}"):
        fail(SCHEMA, f"schema.sql differs from {tag}'s. The baseline is frozen: change the "
                     "schema with a new step in STEPS (src/db/migrate.rs) instead.")
with open(MIGRATE) as f:
    now = steps(f.read())
then = steps(git("show", f"{tag}:{MIGRATE}"))
if now is None:
    fail(MIGRATE, "can't find the schema-steps-begin/end markers around STEP_SHA256")
if then is None:
    fail(MIGRATE, f"can't find {tag}'s STEP_SHA256 between its schema-steps markers")
if now[:len(then)] != then:
    fail(MIGRATE, f"{tag}'s migration steps aren't all still here, unchanged and in order: "
                  "a released step is never edited, reordered or removed.")
print(f"Schema as released in {tag}, with {len(now) - len(then)} step(s) since.")
