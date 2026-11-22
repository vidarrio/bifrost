-- Schema 2: a catalog world's public note, shown to players on the Games page.
-- `note` stays the sysadmins' own (source, who vetted it).
ALTER TABLE catalog_world ADD COLUMN public_note TEXT NOT NULL DEFAULT '';
