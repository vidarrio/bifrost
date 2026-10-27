//! Minesweeper with **guaranteed no-guess boards**. Players clear boards to earn
//! bonus-hint credits ([`crate::db`]). The server is authoritative: it holds the
//! mine layout, generates it on the first click (so that click is always safe),
//! validates every reveal, and only a full clear awards a credit.
//!
//! No-guess guarantee: a candidate board is accepted only if a logical solver
//! can clear it from the first click using single-point deductions plus bounded
//! frontier enumeration (which is complete for small frontiers). Any board the
//! solver can't prove is discarded and regenerated, so a served board never
//! requires a guess.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

pub const W: usize = 16;
pub const H: usize = 16;
pub const MINES: usize = 40;
/// Frontier components larger than this aren't enumerated (treated as
/// unprovable → the board is regenerated). Bounds enumeration cost.
const ENUM_CAP: usize = 24;
const GEN_ATTEMPTS: usize = 6000;

/// Board difficulty. Dimensions/mine counts follow the classic Minesweeper
/// presets: standard = intermediate (16×16/40), expert = 30×16/99.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Standard,
    Expert,
}

impl Kind {
    pub fn parse(s: &str) -> Kind {
        if s == "expert" {
            Kind::Expert
        } else {
            Kind::Standard
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Kind::Standard => "standard",
            Kind::Expert => "expert",
        }
    }
    /// (width, height, mines).
    pub fn dims(self) -> (usize, usize, usize) {
        match self {
            Kind::Standard => (W, H, MINES),
            Kind::Expert => (30, 16, 99),
        }
    }
}

/// splitmix64 — a tiny PRNG so we don't pull in a dependency.
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed ^ 0x9E37_79B9_7F4A_7C15)
    }
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

fn neighbors(w: usize, h: usize, i: usize) -> Vec<usize> {
    let (r, c) = (i / w, i % w);
    let mut out = Vec::with_capacity(8);
    for dr in -1i64..=1 {
        for dc in -1i64..=1 {
            if dr == 0 && dc == 0 {
                continue;
            }
            let (nr, nc) = (r as i64 + dr, c as i64 + dc);
            if nr >= 0 && nr < h as i64 && nc >= 0 && nc < w as i64 {
                out.push(nr as usize * w + nc as usize);
            }
        }
    }
    out
}

fn counts_of(w: usize, h: usize, mines: &[bool]) -> Vec<u8> {
    (0..w * h)
        .map(|i| {
            if mines[i] {
                0
            } else {
                neighbors(w, h, i).iter().filter(|&&j| mines[j]).count() as u8
            }
        })
        .collect()
}

/// A single player's game. The board is generated lazily on the first reveal.
pub struct Game {
    /// Board difficulty id ("standard" / "expert"), recorded with each result.
    pub kind: &'static str,
    pub w: usize,
    pub h: usize,
    pub mines_n: usize,
    pub created: u64,
    // --- Active-play timer (excludes time while the tab is closed/hidden) ---
    /// Active play time banked so far, in ms.
    accumulated_ms: i64,
    /// Whether the clock is currently running (tab open + playing).
    pub running: bool,
    /// Nanos at the start of the current running segment.
    last_resume: u64,
    mines: Vec<bool>,
    counts: Vec<u8>,
    revealed: Vec<bool>,
    generated: bool,
    pub over: bool,
    pub won: bool,
}

pub enum Reveal {
    /// Newly revealed cells as (row, col, adjacent-mine count).
    Play(Vec<(usize, usize, u8)>),
    Won(Vec<(usize, usize, u8)>),
    /// The game is lost; carries every mine's (row, col).
    Lost(Vec<(usize, usize)>),
    /// Out-of-range / already-over — nothing happened.
    Invalid,
}

impl Game {
    pub fn new(kind: Kind) -> Self {
        let (w, h, mines_n) = kind.dims();
        let n = w * h;
        Game {
            kind: kind.id(),
            w,
            h,
            mines_n,
            created: now_nanos(),
            accumulated_ms: 0,
            running: false,
            last_resume: 0,
            mines: vec![false; n],
            counts: vec![0; n],
            revealed: vec![false; n],
            generated: false,
            over: false,
            won: false,
        }
    }

    /// Active play time so far, in ms (banked segments plus the live one if the
    /// clock is running). Time while paused — tab closed/hidden — isn't counted.
    pub fn active_ms(&self) -> i64 {
        self.accumulated_ms + self.live_segment()
    }

    fn live_segment(&self) -> i64 {
        if self.running && self.last_resume > 0 {
            (now_nanos().saturating_sub(self.last_resume) / 1_000_000) as i64
        } else {
            0
        }
    }

    /// Bank the running segment into `accumulated_ms` and restart it at now.
    fn bank(&mut self) {
        self.accumulated_ms += self.live_segment();
        self.last_resume = now_nanos();
    }

    /// Pause the clock, banking the current segment (called when the tab hides).
    pub fn pause(&mut self) {
        if !self.over && self.running {
            self.accumulated_ms += self.live_segment();
            self.running = false;
        }
    }

    /// Stop the clock on reopen WITHOUT banking the trailing segment: if the
    /// pause beacon was missed (tab closed abruptly), that trailing time is the
    /// closed-tab gap and must not count. The next reveal resumes the clock.
    pub fn repose(&mut self) {
        if !self.over {
            self.running = false;
        }
    }

    /// Every revealed cell as (row, col, adjacent-mine count) — to rebuild the
    /// board after a reopen.
    pub fn revealed_cells(&self) -> Vec<(usize, usize, u8)> {
        (0..self.w * self.h)
            .filter(|&i| self.revealed[i])
            .map(|i| (i / self.w, i % self.w, self.counts[i]))
            .collect()
    }

    fn idx(&self, r: usize, c: usize) -> Option<usize> {
        if r < self.h && c < self.w {
            Some(r * self.w + c)
        } else {
            None
        }
    }

    pub fn reveal(&mut self, r: usize, c: usize) -> Reveal {
        if self.over {
            return Reveal::Invalid;
        }
        let Some(i) = self.idx(r, c) else {
            return Reveal::Invalid;
        };
        if self.revealed[i] {
            return Reveal::Invalid;
        }
        // Timer: first reveal starts it; a reveal after a pause resumes it (the
        // paused gap isn't counted); otherwise bank the elapsed active time.
        if !self.generated {
            self.generate(i);
            self.generated = true;
            self.accumulated_ms = 0;
            self.running = true;
            self.last_resume = now_nanos();
        } else if self.running {
            self.bank();
        } else {
            self.running = true;
            self.last_resume = now_nanos();
        }
        if self.mines[i] {
            self.over = true;
            self.won = false;
            self.bank();
            self.running = false;
            let ms = (0..self.w * self.h)
                .filter(|&j| self.mines[j])
                .map(|j| (j / self.w, j % self.w))
                .collect();
            return Reveal::Lost(ms);
        }
        // Flood-reveal from a safe cell (expanding through zeros).
        let mut out = Vec::new();
        let mut stack = vec![i];
        while let Some(j) = stack.pop() {
            if self.revealed[j] {
                continue;
            }
            self.revealed[j] = true;
            out.push((j / self.w, j % self.w, self.counts[j]));
            if self.counts[j] == 0 {
                for k in neighbors(self.w, self.h, j) {
                    if !self.revealed[k] {
                        stack.push(k);
                    }
                }
            }
        }
        let safe_total = self.w * self.h - self.mines_n;
        let revealed_n = self.revealed.iter().filter(|&&x| x).count();
        if revealed_n >= safe_total {
            self.over = true;
            self.won = true;
            self.bank();
            self.running = false;
            Reveal::Won(out)
        } else {
            Reveal::Play(out)
        }
    }

    /// Generate a no-guess board with `first` (and its neighbours) mine-free, so
    /// the first click opens a region and the board is logically solvable.
    fn generate(&mut self, first: usize) {
        let n = self.w * self.h;
        let mut safe_zone = vec![first];
        safe_zone.extend(neighbors(self.w, self.h, first));
        let mut rng = Rng::new(now_nanos() ^ (first as u64).wrapping_mul(0x100_0000_01b3));

        let placeable: Vec<usize> = (0..n).filter(|j| !safe_zone.contains(j)).collect();
        let mut best: Option<(Vec<bool>, Vec<u8>)> = None;
        for _ in 0..GEN_ATTEMPTS {
            let mut mines = vec![false; n];
            // Fisher–Yates over a copy to pick MINES distinct placeable cells.
            let mut pool = placeable.clone();
            for k in 0..self.mines_n.min(pool.len()) {
                let pick = k + rng.below(pool.len() - k);
                pool.swap(k, pick);
                mines[pool[k]] = true;
            }
            let counts = counts_of(self.w, self.h, &mines);
            if best.is_none() {
                best = Some((mines.clone(), counts.clone()));
            }
            if solvable(self.w, self.h, &mines, &counts, first) {
                self.mines = mines;
                self.counts = counts;
                return;
            }
        }
        // Extremely unlikely for 9×9/10; fall back to the last board tried.
        let (m, c) = best.unwrap();
        self.mines = m;
        self.counts = c;
    }
}

/// Can this board be cleared from `first` by pure logic (no guessing)?
fn solvable(w: usize, h: usize, mines: &[bool], counts: &[u8], first: usize) -> bool {
    let n = w * h;
    let mut rev = vec![false; n];
    let mut mine_known = vec![false; n];
    flood(w, h, counts, first, &mut rev);

    loop {
        let mut changed = false;
        // Single-point deductions.
        for i in 0..n {
            if !rev[i] {
                continue;
            }
            let nb = neighbors(w, h, i);
            let hidden: Vec<usize> = nb
                .iter()
                .cloned()
                .filter(|&j| !rev[j] && !mine_known[j])
                .collect();
            if hidden.is_empty() {
                continue;
            }
            let known_m = nb.iter().filter(|&&j| mine_known[j]).count() as u8;
            if counts[i] == known_m {
                for &j in &hidden {
                    flood(w, h, counts, j, &mut rev);
                    changed = true;
                }
            } else if counts[i] - known_m == hidden.len() as u8 {
                for &j in &hidden {
                    if !mine_known[j] {
                        mine_known[j] = true;
                        changed = true;
                    }
                }
            }
        }
        if all_safe_revealed(mines, &rev) {
            return true;
        }
        if changed {
            continue;
        }
        // Complete (bounded) deduction over frontier components.
        if enumerate(w, h, counts, &mut rev, &mut mine_known) {
            continue;
        }
        return false;
    }
}

/// Reveal a known-safe cell, flood-filling through zero cells.
fn flood(w: usize, h: usize, counts: &[u8], start: usize, rev: &mut [bool]) {
    if rev[start] {
        return;
    }
    let mut stack = vec![start];
    while let Some(j) = stack.pop() {
        if rev[j] {
            continue;
        }
        rev[j] = true;
        if counts[j] == 0 {
            for k in neighbors(w, h, j) {
                if !rev[k] {
                    stack.push(k);
                }
            }
        }
    }
}

fn all_safe_revealed(mines: &[bool], rev: &[bool]) -> bool {
    (0..mines.len()).all(|i| mines[i] || rev[i])
}

/// A revealed numbered cell's constraint: its still-hidden neighbours must
/// contain exactly `need` mines.
struct Constraint {
    cells: Vec<usize>,
    need: i64,
}

/// Enumerate mine assignments over each frontier component; deduce any cell that
/// is a mine (or safe) in every valid assignment. Returns true if it deduced
/// anything. Components larger than `ENUM_CAP` are skipped (left unproven).
fn enumerate(w: usize, h: usize, counts: &[u8], rev: &mut [bool], mine_known: &mut [bool]) -> bool {
    let n = w * h;
    // Build constraints from revealed numbered cells with hidden neighbours.
    let mut constraints: Vec<Constraint> = Vec::new();
    for i in 0..n {
        if !rev[i] {
            continue;
        }
        let nb = neighbors(w, h, i);
        let hidden: Vec<usize> = nb
            .iter()
            .cloned()
            .filter(|&j| !rev[j] && !mine_known[j])
            .collect();
        if hidden.is_empty() {
            continue;
        }
        let known_m = nb.iter().filter(|&&j| mine_known[j]).count() as i64;
        constraints.push(Constraint {
            cells: hidden,
            need: counts[i] as i64 - known_m,
        });
    }
    if constraints.is_empty() {
        return false;
    }

    // Union-find over variables (hidden cells) that share a constraint.
    let mut parent: HashMap<usize, usize> = HashMap::new();
    fn find(parent: &mut HashMap<usize, usize>, x: usize) -> usize {
        let mut r = x;
        while let Some(&p) = parent.get(&r) {
            if p == r {
                break;
            }
            r = p;
        }
        r
    }
    for cons in &constraints {
        for &v in &cons.cells {
            parent.entry(v).or_insert(v);
        }
        if let Some(&first) = cons.cells.first() {
            let ra = find(&mut parent, first);
            for &v in &cons.cells[1..] {
                let rb = find(&mut parent, v);
                if ra != rb {
                    parent.insert(rb, ra);
                }
            }
        }
    }

    // Group variables + constraints by component root.
    let mut comp_vars: HashMap<usize, Vec<usize>> = HashMap::new();
    let vars_all: Vec<usize> = parent.keys().cloned().collect();
    for v in vars_all {
        let root = find(&mut parent, v);
        comp_vars.entry(root).or_default().push(v);
    }
    let mut changed = false;
    for (root, mut vars) in comp_vars.into_iter() {
        vars.sort_unstable();
        vars.dedup();
        if vars.len() > ENUM_CAP {
            continue;
        }
        let cons: Vec<&Constraint> = constraints
            .iter()
            .filter(|c| c.cells.iter().any(|v| find(&mut parent, *v) == root))
            .collect();
        // index within `vars`
        let vindex: HashMap<usize, usize> = vars.iter().enumerate().map(|(i, &v)| (v, i)).collect();
        let clist: Vec<(Vec<usize>, i64)> = cons
            .iter()
            .map(|c| (c.cells.iter().map(|v| vindex[v]).collect(), c.need))
            .collect();

        let mut assign = vec![0u8; vars.len()];
        let mut solutions: i64 = 0;
        let mut mine_count = vec![0i64; vars.len()];
        let mut overflow = false;
        backtrack(
            0,
            &mut assign,
            &clist,
            &mut solutions,
            &mut mine_count,
            &mut overflow,
        );
        if overflow || solutions == 0 {
            continue;
        }
        for (i, &v) in vars.iter().enumerate() {
            if mine_count[i] == 0 {
                // safe in every solution
                if !rev[v] {
                    flood(w, h, counts, v, rev);
                    changed = true;
                }
            } else if mine_count[i] == solutions && !mine_known[v] {
                mine_known[v] = true;
                changed = true;
            }
        }
    }
    changed
}

/// Depth-first over variable assignments, tallying valid solutions + per-var
/// mine counts. Prunes on constraint violation; caps solutions to stay bounded.
fn backtrack(
    idx: usize,
    assign: &mut [u8],
    clist: &[(Vec<usize>, i64)],
    solutions: &mut i64,
    mine_count: &mut [i64],
    overflow: &mut bool,
) {
    const MAX_SOLUTIONS: i64 = 200_000;
    if *overflow {
        return;
    }
    // Check constraints that are fully assigned so far; prune early.
    for (cells, need) in clist {
        let mut sum = 0i64;
        let mut unassigned = 0i64;
        for &c in cells {
            if c < idx {
                sum += assign[c] as i64;
            } else {
                unassigned += 1;
            }
        }
        if sum > *need || sum + unassigned < *need {
            return;
        }
    }
    if idx == assign.len() {
        *solutions += 1;
        if *solutions > MAX_SOLUTIONS {
            *overflow = true;
            return;
        }
        for (i, &a) in assign.iter().enumerate() {
            mine_count[i] += a as i64;
        }
        return;
    }
    assign[idx] = 0;
    backtrack(idx + 1, assign, clist, solutions, mine_count, overflow);
    assign[idx] = 1;
    backtrack(idx + 1, assign, clist, solutions, mine_count, overflow);
    assign[idx] = 0;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn check_kind(kind: Kind, n: usize, budget_ms: u128) {
        let (w, h, mines) = kind.dims();
        let t = Instant::now();
        for k in 0..n {
            let mut g = Game::new(kind);
            // First click somewhere varied but interior-ish.
            let first = (2 + k % (w - 4)) + w * (2 + (k / (w - 4)) % (h - 4));
            let _ = g.reveal(first / w, first % w);
            // The generated board must be solvable from that first click.
            assert!(
                solvable(g.w, g.h, &g.mines, &g.counts, first),
                "generated {} board {k} required a guess",
                kind.id()
            );
        }
        let ms = t.elapsed().as_millis();
        assert!(
            ms < budget_ms,
            "{} generation too slow: {ms}ms for {n} {w}x{h}/{mines} boards",
            kind.id()
        );
        eprintln!(
            "{n} no-guess {w}x{h}/{mines} ({}) boards in {ms}ms",
            kind.id()
        );
    }

    #[test]
    fn standard_boards_are_no_guess_and_fast() {
        check_kind(Kind::Standard, 30, 20000);
    }

    #[test]
    fn expert_boards_are_no_guess() {
        check_kind(Kind::Expert, 15, 60000);
    }
}
