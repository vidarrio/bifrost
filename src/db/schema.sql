-- The database as of schema version 1, Bifrost 0.3.0. Frozen once that's released:
-- change the schema with a step in `STEPS` (src/db/migrate.rs), never here.
--
-- Conventions: tables are STRICT. A reference to a table is `<table>_id`, or says its
-- role (`created_by`). Seasons, games, slots and seats have generated ids (src/id.rs);
-- accounts have integer ones, as they never reach a URL. Times are epoch seconds, in
-- `<event>_at`, or `ts` for when a logged event happened. Booleans are 0 or 1.
-- An account with history can't be deleted: what it played stays. Only what's its own
-- alone goes with it. Slot and seat names ignore case, as Archipelago's do.

CREATE TABLE account (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    username TEXT NOT NULL UNIQUE COLLATE NOCASE,
    -- Set for native accounts.
    password_hash TEXT,
    -- The identity provider's stable id, for accounts it provisions.
    idp_subject TEXT UNIQUE,
    role TEXT NOT NULL DEFAULT 'user' CHECK (role IN ('user', 'moderator', 'sysadmin')),
    disabled INTEGER NOT NULL DEFAULT 0 CHECK (disabled IN (0, 1)),
    created_at INTEGER NOT NULL
) STRICT;

CREATE TABLE session (
    token_hash TEXT PRIMARY KEY NOT NULL,
    account_id INTEGER NOT NULL REFERENCES account(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
) STRICT;
CREATE INDEX session_account ON session (account_id);
CREATE INDEX session_expires ON session (expires_at);

-- 'reset' has an account and one use, 'invite' one use, 'community' `max_uses`
-- (NULL = unlimited until it expires or is revoked).
CREATE TABLE auth_link (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    token_hash TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL CHECK (kind IN ('reset', 'invite', 'community')),
    account_id INTEGER REFERENCES account(id) ON DELETE CASCADE,
    label TEXT NOT NULL DEFAULT '',
    max_uses INTEGER,
    uses INTEGER NOT NULL DEFAULT 0,
    expires_at INTEGER NOT NULL,
    revoked INTEGER NOT NULL DEFAULT 0 CHECK (revoked IN (0, 1)),
    created_by INTEGER REFERENCES account(id) ON DELETE SET NULL,
    created_at INTEGER NOT NULL,
    CHECK (kind = 'community' OR max_uses = 1),
    CHECK (kind <> 'reset' OR account_id IS NOT NULL)
) STRICT;
CREATE INDEX auth_link_account ON auth_link (account_id);
CREATE INDEX auth_link_created_by ON auth_link (created_by);

CREATE TABLE season (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'draft'
        CHECK (state IN ('draft', 'signup', 'generating', 'live', 'archived')),
    created_by INTEGER REFERENCES account(id) ON DELETE SET NULL,
    created_at INTEGER NOT NULL,
    -- The AP seed name of the deployed world. One seed names one season and its
    -- archive bundle; imports rely on that.
    seed TEXT UNIQUE,
    live_at INTEGER,
    archived_at INTEGER,
    minesweeper_enabled INTEGER NOT NULL DEFAULT 0 CHECK (minesweeper_enabled IN (0, 1)),
    mc_enabled INTEGER NOT NULL DEFAULT 0 CHECK (mc_enabled IN (0, 1)),
    -- Bonus hints a player can earn from minesweeper in any 24 hours (0 = no cap).
    mine_daily_hints INTEGER NOT NULL DEFAULT 5 CHECK (mine_daily_hints >= 0),
    -- How many games a player may sign up with (0 = no limit).
    max_games INTEGER NOT NULL DEFAULT 0 CHECK (max_games >= 0),
    -- Pages after generation are for members.
    private_live INTEGER NOT NULL DEFAULT 1 CHECK (private_live IN (0, 1)),
    -- Signups are only reachable through a link, whose key this is.
    private_signup INTEGER NOT NULL DEFAULT 0 CHECK (private_signup IN (0, 1)),
    signup_token TEXT,
    -- Where this season's AP-journal ingest resumes.
    journal_cursor TEXT,
    -- Frozen final standings (JSON), captured when archived.
    summary TEXT,
    -- The season's AP server passwords: connect (shown to players, may be empty) and
    -- admin (generated; only Bifrost uses it).
    ap_password TEXT NOT NULL DEFAULT '',
    ap_admin_password TEXT NOT NULL DEFAULT '',
    -- The port its managed AP server listens on: pinned at go-live, held through
    -- restarts and re-rolls, released on archive.
    ap_port INTEGER UNIQUE,
    -- The Archipelago version that generated the current seed.
    ap_version TEXT,
    -- While generating: the state it started from (crash recovery).
    gen_from TEXT CHECK (gen_from IN ('draft', 'signup', 'live')),
    CHECK (private_signup = 0 OR signup_token IS NOT NULL),
    CHECK (ap_port BETWEEN 1 AND 65535)
) STRICT;
CREATE INDEX season_created_by ON season (created_by);

-- Who came through a private season's signup link.
CREATE TABLE season_invited (
    season_id TEXT NOT NULL REFERENCES season(id) ON DELETE CASCADE,
    account_id INTEGER NOT NULL REFERENCES account(id) ON DELETE CASCADE,
    PRIMARY KEY (season_id, account_id)
) STRICT;
CREATE INDEX season_invited_account ON season_invited (account_id);

-- Co-moderators: besides its creator, who may run a season.
CREATE TABLE season_mod (
    season_id TEXT NOT NULL REFERENCES season(id) ON DELETE CASCADE,
    account_id INTEGER NOT NULL REFERENCES account(id) ON DELETE CASCADE,
    PRIMARY KEY (season_id, account_id)
) STRICT;
CREATE INDEX season_mod_account ON season_mod (account_id);

CREATE TABLE setting (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
) STRICT;

CREATE TABLE catalog_world (
    sha256 TEXT PRIMARY KEY NOT NULL,
    -- The name it installs as in a season's worlds/.
    file TEXT NOT NULL,
    game TEXT NOT NULL DEFAULT '',
    version TEXT NOT NULL DEFAULT '',
    note TEXT NOT NULL DEFAULT '',
    added_by TEXT NOT NULL,
    added_at INTEGER NOT NULL
) STRICT;

-- An uploaded apworld waiting for approval in a season.
CREATE TABLE world_request (
    season_id TEXT NOT NULL REFERENCES season(id) ON DELETE CASCADE,
    file TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    game TEXT NOT NULL DEFAULT '',
    uploaded_by TEXT NOT NULL,
    uploaded_at INTEGER NOT NULL,
    PRIMARY KEY (season_id, file)
) STRICT;

-- A season's slots once generated: one per AP slot, plus any co-op seats on it.
CREATE TABLE slot (
    id TEXT PRIMARY KEY NOT NULL,
    season_id TEXT NOT NULL REFERENCES season(id) ON DELETE CASCADE,
    -- A game's id: shared by its seats, and the signup's id if it had one.
    game_id TEXT NOT NULL,
    name TEXT NOT NULL COLLATE NOCASE,
    ap_slot INTEGER NOT NULL,
    game TEXT NOT NULL,
    -- The shared "common game" slot (one AP slot everyone plays).
    is_common INTEGER NOT NULL DEFAULT 0 CHECK (is_common IN (0, 1)),
    -- A co-op seat: a name of Bifrost's, not one Archipelago knows.
    is_seat INTEGER NOT NULL DEFAULT 0 CHECK (is_seat IN (0, 1)),
    UNIQUE (season_id, name)
) STRICT;
CREATE INDEX slot_ap ON slot (season_id, ap_slot);
CREATE INDEX slot_game ON slot (season_id, game_id);

CREATE TABLE claim (
    slot_id TEXT PRIMARY KEY NOT NULL REFERENCES slot(id) ON DELETE CASCADE,
    account_id INTEGER NOT NULL REFERENCES account(id),
    claimed_at INTEGER NOT NULL
) STRICT;
CREATE INDEX claim_account ON claim (account_id);

-- A signup for a season: one game, from its host.
CREATE TABLE submission (
    id TEXT PRIMARY KEY NOT NULL,
    season_id TEXT NOT NULL REFERENCES season(id) ON DELETE CASCADE,
    account_id INTEGER NOT NULL REFERENCES account(id),
    slot_name TEXT NOT NULL COLLATE NOCASE,
    game TEXT NOT NULL,
    is_custom INTEGER NOT NULL DEFAULT 0 CHECK (is_custom IN (0, 1)),
    yaml_file TEXT NOT NULL,
    -- Held until this apworld is approved, and why it can't go ahead, if so.
    held_for TEXT,
    held_err TEXT,
    -- Shared notes for the season's other players.
    notes TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL,
    UNIQUE (season_id, slot_name)
) STRICT;
CREATE INDEX submission_season_account ON submission (season_id, account_id);
CREATE INDEX submission_account ON submission (account_id);

-- Co-op partners on a signup, who join through one-use links from its host. A name
-- is unique across a season's signups and seats, which the triggers keep.
CREATE TABLE seat (
    id TEXT PRIMARY KEY NOT NULL,
    season_id TEXT NOT NULL REFERENCES season(id) ON DELETE CASCADE,
    submission_id TEXT NOT NULL REFERENCES submission(id) ON DELETE CASCADE,
    account_id INTEGER NOT NULL REFERENCES account(id),
    name TEXT NOT NULL COLLATE NOCASE,
    created_at INTEGER NOT NULL,
    UNIQUE (season_id, name),
    UNIQUE (submission_id, account_id)
) STRICT;
CREATE INDEX seat_account ON seat (account_id);

-- A link is to a signup, or to a live game by its connect name. Its token is kept
-- readable, unlike sessions' and auth links', as its host can see it again. It only
-- seats one person with an account here.
CREATE TABLE seat_link (
    token TEXT PRIMARY KEY NOT NULL,
    season_id TEXT NOT NULL REFERENCES season(id) ON DELETE CASCADE,
    submission_id TEXT REFERENCES submission(id) ON DELETE CASCADE,
    slot_name TEXT,
    created_by INTEGER NOT NULL REFERENCES account(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    CHECK ((submission_id IS NULL) <> (slot_name IS NULL))
) STRICT;
CREATE INDEX seat_link_season ON seat_link (season_id);
CREATE INDEX seat_link_submission ON seat_link (submission_id);
CREATE INDEX seat_link_created_by ON seat_link (created_by);

CREATE TRIGGER seat_name_insert BEFORE INSERT ON seat
WHEN EXISTS (SELECT 1 FROM submission WHERE season_id = NEW.season_id AND slot_name = NEW.name)
BEGIN SELECT RAISE(ABORT, 'slot name taken'); END;
CREATE TRIGGER seat_name_update BEFORE UPDATE OF name ON seat
WHEN EXISTS (SELECT 1 FROM submission WHERE season_id = NEW.season_id AND slot_name = NEW.name)
BEGIN SELECT RAISE(ABORT, 'slot name taken'); END;
CREATE TRIGGER submission_name_insert BEFORE INSERT ON submission
WHEN EXISTS (SELECT 1 FROM seat WHERE season_id = NEW.season_id AND name = NEW.slot_name)
BEGIN SELECT RAISE(ABORT, 'slot name taken'); END;
CREATE TRIGGER submission_name_update BEFORE UPDATE OF slot_name ON submission
WHEN EXISTS (SELECT 1 FROM seat WHERE season_id = NEW.season_id AND name = NEW.slot_name)
BEGIN SELECT RAISE(ABORT, 'slot name taken'); END;

-- Bonus hint credits earned through seasonal minesweeper.
CREATE TABLE hint_credit (
    season_id TEXT NOT NULL REFERENCES season(id) ON DELETE CASCADE,
    account_id INTEGER NOT NULL REFERENCES account(id) ON DELETE CASCADE,
    credits INTEGER NOT NULL DEFAULT 0 CHECK (credits >= 0),
    PRIMARY KEY (season_id, account_id)
) STRICT;
CREATE INDEX hint_credit_account ON hint_credit (account_id);

-- Hints a player has flagged to surface on the board.
CREATE TABLE hint_priority (
    season_id TEXT NOT NULL REFERENCES season(id) ON DELETE CASCADE,
    finding INTEGER NOT NULL,
    location INTEGER NOT NULL,
    item INTEGER NOT NULL,
    PRIMARY KEY (season_id, finding, location, item)
) STRICT;

-- One row per finished minesweeper game. `season_id` NULL is general play.
CREATE TABLE mine_result (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    season_id TEXT REFERENCES season(id) ON DELETE CASCADE,
    account_id INTEGER NOT NULL REFERENCES account(id),
    won INTEGER NOT NULL CHECK (won IN (0, 1)),
    duration_ms INTEGER NOT NULL,
    -- Whether this win earned a bonus hint (the daily cap counts those).
    earned INTEGER NOT NULL DEFAULT 0 CHECK (earned IN (0, 1)),
    ts INTEGER NOT NULL,
    kind TEXT NOT NULL DEFAULT 'standard'
) STRICT;
CREATE INDEX mine_result_season ON mine_result (season_id, account_id, kind);
CREATE INDEX mine_result_account ON mine_result (account_id);

-- A player's Minecraft username, for the common-game server's whitelist. Per account,
-- so a returning player keeps it; unique so two can't claim one Minecraft account.
CREATE TABLE mc_account (
    account_id INTEGER PRIMARY KEY REFERENCES account(id) ON DELETE CASCADE,
    mc_username TEXT NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;
CREATE UNIQUE INDEX mc_account_name ON mc_account (mc_username COLLATE NOCASE);

-- One row per kept AP journal event, in journal order by `id`. De-duplication across
-- runs is by the season's journal cursor, so identical events in a second both stay.
CREATE TABLE activity (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    season_id TEXT NOT NULL REFERENCES season(id) ON DELETE CASCADE,
    ts INTEGER NOT NULL,
    kind TEXT NOT NULL,
    text TEXT NOT NULL,
    sender TEXT,
    receiver TEXT,
    item_class TEXT
) STRICT;
CREATE INDEX activity_season ON activity (season_id, id);

-- Universal Tracker's in-logic cache: a slot's reachable locations, as of `sent` and
-- `received`, so it's only recomputed when those move.
CREATE TABLE inlogic (
    season_id TEXT NOT NULL REFERENCES season(id) ON DELETE CASCADE,
    ap_slot INTEGER NOT NULL,
    ts INTEGER NOT NULL,
    sent INTEGER NOT NULL DEFAULT 0,
    received INTEGER NOT NULL DEFAULT 0,
    count INTEGER NOT NULL DEFAULT 0,
    locations TEXT NOT NULL DEFAULT '[]',
    err TEXT,
    PRIMARY KEY (season_id, ap_slot)
) STRICT;
