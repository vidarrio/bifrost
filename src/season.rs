//! Season archive: a frozen, human-readable summary of a finished season, kept
//! for posterity so it survives the full reset (which wipes the live slots,
//! save, and world) and the seed rollover (which hides the old season's
//! minesweeper leaderboards). Captured by the admin "Archive & finalize season"
//! action while the season is still live, stored as JSON in `season_archive`,
//! and rendered on the Past Seasons page.

use serde::{Deserialize, Serialize};

/// One archived season's complete displayable summary.
#[derive(Serialize, Deserialize, Default)]
pub struct SeasonSummary {
    pub name: String,
    pub seed: String,
    pub finalized_at: i64,
    pub player_count: usize,
    pub games: Vec<SeasonGame>,
    pub minesweeper: Vec<MineBoard>,
}

/// Final standing for one game (one AP slot) at archive time.
#[derive(Serialize, Deserialize, Default)]
pub struct SeasonGame {
    pub game: String,
    /// The seat names (co-op games have several), primary player first.
    pub players: Vec<String>,
    pub goaled: bool,
    /// Locations checked and the world's total, with the derived percentage.
    pub checks: u64,
    pub total: u32,
    pub pct: u32,
}

/// A frozen minesweeper leaderboard for one board kind, as of archive time.
#[derive(Serialize, Deserialize, Default)]
pub struct MineBoard {
    pub kind: String,
    pub label: String,
    /// (player, best_ms), fastest first.
    pub time: Vec<(String, i64)>,
    /// (player, wins, total), highest rate first.
    pub rate: Vec<(String, i64, i64)>,
}

impl MineBoard {
    /// Whether this board has any entries worth showing.
    pub fn is_empty(&self) -> bool {
        self.time.is_empty() && self.rate.is_empty()
    }
}
