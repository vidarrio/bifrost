-- Schema 4: the moderator log. What moderators and sysadmins did in season
-- and instance administration, for sysadmins to look back over. A row keeps
-- the names it was written with, so it outlives the season or account.
CREATE TABLE mod_action (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    at INTEGER NOT NULL,
    account_id INTEGER REFERENCES account(id) ON DELETE SET NULL,
    username TEXT NOT NULL,
    season_id TEXT REFERENCES season(id) ON DELETE SET NULL,
    season_name TEXT NOT NULL DEFAULT '',
    action TEXT NOT NULL,
    detail TEXT NOT NULL DEFAULT '',
    status INTEGER NOT NULL
) STRICT;
CREATE INDEX mod_action_account ON mod_action (account_id);
CREATE INDEX mod_action_season ON mod_action (season_id);
CREATE INDEX mod_action_at ON mod_action (at);
