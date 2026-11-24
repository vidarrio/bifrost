-- Schema 3: notifications, shown in the header's mailbox. Read ones stay
-- listed (the most recent per account are kept).
CREATE TABLE notification (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id INTEGER NOT NULL REFERENCES account(id) ON DELETE CASCADE,
    season_id TEXT REFERENCES season(id) ON DELETE CASCADE,
    text TEXT NOT NULL,
    link TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL,
    read INTEGER NOT NULL DEFAULT 0 CHECK (read IN (0, 1))
) STRICT;
CREATE INDEX notification_account ON notification (account_id, id);
CREATE INDEX notification_season ON notification (season_id);
