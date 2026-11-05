# Test fixtures

`foo`, `foo-v2`, `bar` and `baz` are copies of Archipelago's ChecksFinder world
(MIT licensed, from [ArchipelagoMW/Archipelago](https://github.com/ArchipelagoMW/Archipelago))
renamed to the games "Foo Finder", "Bar Finder" and "Baz Finder", packaged as
`.apworld` files. `foo-v2` is `foo` with one extra comment line: different
bytes under the same name, for the "no swapping a world out" checks. The YAMLs
are matching player options. The catalog suite uses them as custom worlds.

`baseline.db` is a database at schema version 1, the baseline 0.3.0 freezes,
with a bit of everything Bifrost keeps. It was written by Bifrost through its
own pages (`make_baseline.py`), and the `upgrade` suite opens it with the
current build. Its accounts' password is `correct horse battery`. See
"Database changes" in CONTRIBUTING.md for when it may be remade.
