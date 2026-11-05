# Schema steps

Each change to the database schema after the baseline (`../schema.sql`) is a
file here, named after the schema version it makes: `0002.sql` takes a
database from version 1 to 2, `0003.sql` from 2 to 3, and so on. Each one is
included in `STEPS` in `../migrate.rs`, in order.

A step that's been released is never edited, renamed or removed. CI compares
these files and `schema.sql` with the latest release's, byte for byte (see
"Database changes" in CONTRIBUTING.md).
