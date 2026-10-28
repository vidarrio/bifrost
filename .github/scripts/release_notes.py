"""Release notes for a version, from CHANGELOG.md, checked against Cargo.toml.

    release_notes.py X.Y.Z [--dry-run] > notes.md

Prints the `## [X.Y.Z]` section's body. Fails if Cargo.toml's version isn't
X.Y.Z or the changelog has no such section, so a release can't go out with
the wrong version or without notes. With --dry-run (used when the release
workflow is tested on a pull request) it falls back to `## [Unreleased]` and
skips the Cargo.toml check.
"""
import os
import re
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))


def section(changelog, name):
    """The body of `## [name]` up to the next `## [` heading or link reference."""
    out, inside = [], False
    for line in changelog.splitlines():
        if re.match(r"^## \[", line):
            if inside:
                break
            inside = line.startswith(f"## [{name}]")
            continue
        if inside and re.match(r"^\[[^\]]+\]: ", line):
            break
        if inside:
            out.append(line)
    return "\n".join(out).strip() if inside or out else None


def main():
    version, dry = sys.argv[1], "--dry-run" in sys.argv[2:]
    changelog = open(os.path.join(ROOT, "CHANGELOG.md"), encoding="utf-8").read()
    if not dry:
        cargo = open(os.path.join(ROOT, "Cargo.toml"), encoding="utf-8").read()
        m = re.search(r'^version = "([^"]+)"', cargo, re.M)
        if not m or m.group(1) != version:
            sys.exit(f"Cargo.toml's version is {m.group(1) if m else '?'}, not {version}")
    notes = section(changelog, version)
    if notes is None and dry:
        notes = section(changelog, "Unreleased")
    if not notes:
        sys.exit(f"CHANGELOG.md has no notes for [{version}]")
    print(notes)


if __name__ == "__main__":
    main()
