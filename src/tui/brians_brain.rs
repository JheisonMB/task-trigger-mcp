//! Brian's Brain cellular automaton for the home screen.
//!
//! 3-state automaton: On → Dying → Off → On
//! Rule: Off cell turns On if exactly 2 neighbors are On.
//! Uses toroidal wrapping so patterns flow across edges.
//!
//! The grid is seeded from the CANOPY banner text so the automaton
//! looks like the banner "exploding" when it activates.
//!
//! Before activation, a digital-corruption glitch replaces banner characters
//! with 0s and 1s (Matrix-style) while vibrating, then snaps back to normal.
//! This repeats with progressive intensity until the banner finally explodes.
//!
//! Includes automatic particle count validation and noise injection to prevent
//! the automaton from stabilizing with too few particles.

use std::time::Instant;

// ── Automaton tuning ────────────────────────────────────────────

const MIN_PARTICLE_THRESHOLD: f64 = 0.010;
const LOW_ACTIVITY_THRESHOLD: f64 = 0.024;
const EDGE_NOISE_PROBABILITY: f64 = 0.22;
const EDGE_PULSE_PROBABILITY: f64 = 0.08;
const NOISE_PULSE_PROBABILITY: f64 = 0.12;
const EDGE_PULSE_BURST_MIN: usize = 4;
const EDGE_PULSE_BURST_MAX: usize = 14;

// ── Glitch tuning ──────────────────────────────────────────────

/// Fixed initial delay (ms) before glitch starts.
const INITIAL_DELAY_MS: u64 = 1000;
const MIN_GLITCH_CYCLES: usize = 3;
const MAX_GLITCH_CYCLES: usize = 5;
/// How long (ms) the disintegration builds up.
const DISINTEGRATE_MS: u64 = 200;
/// Pause (ms) between consecutive glitch cycles (random within range).
const BETWEEN_CYCLES_MIN_MS: u64 = 200;
const BETWEEN_CYCLES_MAX_MS: u64 = 1000;
/// Max fraction of banner cells corrupted at peak.
const MAX_CORRUPT_FRACTION: f64 = 0.65;

/// Base green channel for the banner seeded cells.
const BANNER_GREEN: u8 = 175;
/// Green channel for edge-noise injected cells.
const NOISE_GREEN: u8 = 220;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CellState {
    Off,
    On,
    Dying,
}

/// Appearance of a single cell in the banner overlay.
#[derive(Clone, Copy)]
pub enum BannerCellKind {
    /// Full block `█`.
    Block,
    /// Light shade `░`.
    Shade,
    /// Corrupted — shows a `0` or `1`.
    Glitch(char),
}

/// Banner row data for the overlay.
#[derive(Clone)]
pub struct BannerRow {
    pub row: usize,
    pub cells: Vec<(usize, BannerCellKind)>,
}

#[derive(Clone, PartialEq, Eq)]
enum Phase {
    Waiting,
    Disintegrating,
    BetweenCycles,
    Done,
}

pub struct BriansBrain {
    pub grid: Vec<Vec<CellState>>,
    /// Per-cell green channel (0-255) for automaton color variation.
    pub green_grid: Vec<Vec<u8>>,
    pub rows: usize,
    pub cols: usize,
    pub home_since: Instant,
    pub active: bool,
    /// Original banner (never mutated).
    banner_base: Vec<BannerRow>,

    phase: Phase,
    phase_started: Instant,
    glitch_cycle: usize,
    total_glitch_cycles: usize,
    /// Shuffled banner cell coordinates — corruption order.
    corrupt_candidates: Vec<(usize, usize)>,
    /// How many cells from front of candidates are corrupted right now.
    corrupt_count: usize,
    /// Peak corruption target for the current cycle.
    peak_corrupt: usize,
    /// Assigned glitch char per candidate (0 or 1).
    corrupt_chars: Vec<char>,
    /// Border noise cells (row, col) shown during disintegration.
    border_noise: Vec<(usize, usize)>,
    /// Random pause for current BetweenCycles.
    next_between_ms: u64,
    /// Vibration offset applied during Disintegrating.
    pub vibration: (i16, i16),
}

const BANNER: &[&str] = &[
    r"  ██████   ██████   ████████    ██████  ████████  █████ ████",
    r" ███░░███ ░░░░░███ ░░███░░███  ███░░███░░███░░███░░███ ░███",
    r"░███ ░░░   ███████  ░███ ░███ ░███ ░███ ░███ ░███ ░███ ░███",
    r"░███  ███ ███░░███  ░███ ░███ ░███ ░███ ░███ ░███ ░███ ░███",
    r"░░██████ ░░████████ ████ █████░░██████  ░███████  ░░███████",
    r" ░░░░░░   ░░░░░░░░ ░░░░ ░░░░░  ░░░░░░   ░███░░░    ░░░░░███",
    r"                                        ░███       ███ ░███",
    r"                                        █████     ░░██████",
    r"                                       ░░░░░       ░░░░░░",
];

fn shuffle<T>(v: &mut [T]) {
    let n = v.len();
    for i in (1..n).rev() {
        let j = rand::random::<u32>() as usize % (i + 1);
        v.swap(i, j);
    }
}

fn rand_between(lo: u64, hi: u64) -> u64 {
    lo + rand::random::<u32>() as u64 % (hi - lo + 1)
}

impl BriansBrain {
    pub fn new(rows: usize, cols: usize) -> Self {
        let (grid, overlay, green_grid) = Self::make_banner_grid(rows, cols);

        let total_glitch_cycles = MIN_GLITCH_CYCLES
            + rand::random::<u32>() as usize % (MAX_GLITCH_CYCLES - MIN_GLITCH_CYCLES + 1);

        let mut candidates: Vec<(usize, usize)> = overlay
            .iter()
            .flat_map(|row| row.cells.iter().map(move |&(col, _)| (row.row, col)))
            .collect();
        shuffle(&mut candidates);
        let corrupt_chars = candidates
            .iter()
            .map(|_| if rand::random::<bool>() { '1' } else { '0' })
            .collect();

        Self {
            grid,
            green_grid,
            rows,
            cols,
            home_since: Instant::now(),
            active: false,
            banner_base: overlay,
            phase: Phase::Waiting,
            phase_started: Instant::now(),
            glitch_cycle: 0,
            total_glitch_cycles,
            corrupt_candidates: candidates,
            corrupt_count: 0,
            peak_corrupt: 0,
            corrupt_chars,
            border_noise: Vec::new(),
            next_between_ms: 0,
            vibration: (0, 0),
        }
    }

    fn make_banner_grid(
        rows: usize,
        cols: usize,
    ) -> (Vec<Vec<CellState>>, Vec<BannerRow>, Vec<Vec<u8>>) {
        let mut grid = vec![vec![CellState::Off; cols]; rows];
        let mut green_grid = vec![vec![0u8; cols]; rows];
        let mut rows_data: Vec<BannerRow> = Vec::new();

        let banner_h = BANNER.len();
        let banner_w = BANNER.iter().map(|l| l.chars().count()).max().unwrap_or(0);
        let top = rows.saturating_sub(banner_h) / 2;
        let left = cols.saturating_sub(banner_w) / 2;

        for (br, line) in BANNER.iter().enumerate() {
            let r = top + br;
            if r >= rows {
                break;
            }
            let mut cells = Vec::new();
            for (bc, ch) in line.chars().enumerate() {
                let c = left + bc;
                if c >= cols {
                    break;
                }
                if ch == '█' {
                    grid[r][c] = CellState::On;
                    // Slight per-cell variation around BANNER_GREEN
                    green_grid[r][c] =
                        BANNER_GREEN.saturating_add((rand::random::<u8>() % 30).wrapping_sub(15));
                    cells.push((c, BannerCellKind::Block));
                } else if ch == '░' {
                    cells.push((c, BannerCellKind::Shade));
                }
            }
            if !cells.is_empty() {
                rows_data.push(BannerRow { row: r, cells });
            }
        }

        (grid, rows_data, green_grid)
    }

    pub fn should_activate(&self) -> bool {
        self.phase == Phase::Done && !self.active
    }

    pub fn tick(&mut self) -> bool {
        if self.active {
            return false;
        }

        match self.phase.clone() {
            Phase::Waiting => {
                if self.home_since.elapsed().as_millis() as u64 >= INITIAL_DELAY_MS {
                    if self.total_glitch_cycles == 0 {
                        self.phase = Phase::Done;
                    } else {
                        self.start_corruption_cycle();
                    }
                }
            }
            Phase::Disintegrating => {
                let elapsed = self.phase_started.elapsed().as_millis() as u64;
                let progress = (elapsed as f64 / DISINTEGRATE_MS as f64).min(1.0);
                self.corrupt_count =
                    ((progress * self.peak_corrupt as f64) as usize).min(self.peak_corrupt);

                // Vibrate: ±1 early, ±2 past 50%
                let max_shake = if progress > 0.5 { 2i16 } else { 1i16 };
                self.vibration = (
                    (rand::random::<i16>() % (max_shake * 2 + 1)) - max_shake,
                    (rand::random::<i16>() % (max_shake * 2 + 1)) - max_shake,
                );

                // Scatter border noise progressively
                if elapsed % 60 < 16 {
                    self.inject_border_noise_incremental(3 + (progress * 8.0) as usize);
                }

                if elapsed >= DISINTEGRATE_MS {
                    // SNAP: instantly reset everything
                    self.corrupt_count = 0;
                    self.vibration = (0, 0);
                    self.border_noise.clear();
                    self.glitch_cycle += 1;
                    if self.glitch_cycle >= self.total_glitch_cycles {
                        self.phase = Phase::Done;
                    } else {
                        self.next_between_ms =
                            rand_between(BETWEEN_CYCLES_MIN_MS, BETWEEN_CYCLES_MAX_MS);
                        self.phase = Phase::BetweenCycles;
                        self.phase_started = Instant::now();
                    }
                }
            }
            Phase::BetweenCycles => {
                if self.phase_started.elapsed().as_millis() as u64 >= self.next_between_ms {
                    self.start_corruption_cycle();
                }
            }
            Phase::Done => {}
        }

        self.should_activate()
    }

    fn start_corruption_cycle(&mut self) {
        shuffle(&mut self.corrupt_candidates);
        // Regenerate glitch chars each cycle
        for ch in self.corrupt_chars.iter_mut() {
            *ch = if rand::random::<bool>() { '1' } else { '0' };
        }
        let total = self.corrupt_candidates.len();
        let cycle_progress = if self.total_glitch_cycles > 1 {
            self.glitch_cycle as f64 / (self.total_glitch_cycles - 1) as f64
        } else {
            1.0
        };
        let min_frac = 0.15 + 0.25 * cycle_progress;
        let max_frac = (min_frac + 0.20).min(MAX_CORRUPT_FRACTION);
        let frac = min_frac + rand::random::<f64>() * (max_frac - min_frac);
        self.peak_corrupt = ((total as f64 * frac) as usize).max(1);
        self.corrupt_count = 0;
        self.border_noise.clear();
        self.phase = Phase::Disintegrating;
        self.phase_started = Instant::now();
    }

    fn inject_border_noise_incremental(&mut self, count: usize) {
        if self.banner_base.is_empty() {
            return;
        }
        let min_row = self.banner_base.iter().map(|r| r.row).min().unwrap_or(0);
        let max_row = self.banner_base.iter().map(|r| r.row).max().unwrap_or(0);
        let min_col = self
            .banner_base
            .iter()
            .flat_map(|r| r.cells.iter().map(|&(c, _)| c))
            .min()
            .unwrap_or(0);
        let max_col = self
            .banner_base
            .iter()
            .flat_map(|r| r.cells.iter().map(|&(c, _)| c))
            .max()
            .unwrap_or(0);

        let margin = 3usize;
        for _ in 0..count {
            let side = rand::random::<u32>() % 4;
            let (r, c) = match side {
                0 => {
                    let r = min_row.saturating_sub(margin)
                        + rand::random::<u32>() as usize % (margin + 1);
                    let span = max_col.saturating_sub(min_col) + 2 * margin + 1;
                    let c = min_col.saturating_sub(margin)
                        + rand::random::<u32>() as usize % span.max(1);
                    (r, c)
                }
                1 => {
                    let r = max_row + 1 + rand::random::<u32>() as usize % margin.max(1);
                    let span = max_col.saturating_sub(min_col) + 2 * margin + 1;
                    let c = min_col.saturating_sub(margin)
                        + rand::random::<u32>() as usize % span.max(1);
                    (r, c)
                }
                2 => {
                    let span = max_row.saturating_sub(min_row) + 2 * margin + 1;
                    let r = min_row.saturating_sub(margin)
                        + rand::random::<u32>() as usize % span.max(1);
                    let c = min_col.saturating_sub(margin)
                        + rand::random::<u32>() as usize % (margin + 1);
                    (r, c)
                }
                _ => {
                    let span = max_row.saturating_sub(min_row) + 2 * margin + 1;
                    let r = min_row.saturating_sub(margin)
                        + rand::random::<u32>() as usize % span.max(1);
                    let c = max_col + 1 + rand::random::<u32>() as usize % margin.max(1);
                    (r, c)
                }
            };
            if r < self.rows && c < self.cols {
                self.border_noise.push((r, c));
            }
        }
    }

    /// Build the overlay for rendering. During disintegration, corrupted cells
    /// become `0`/`1` glitch chars; border noise is appended.
    pub fn visible_overlay(&self) -> Vec<BannerRow> {
        let corrupted: std::collections::HashSet<(usize, usize)> = self.corrupt_candidates
            [..self.corrupt_count]
            .iter()
            .cloned()
            .collect();

        // Map corruption index → char for fast lookup
        let corrupt_map: std::collections::HashMap<(usize, usize), char> = self.corrupt_candidates
            [..self.corrupt_count]
            .iter()
            .enumerate()
            .map(|(i, &pos)| (pos, self.corrupt_chars[i]))
            .collect();

        let mut rows: Vec<BannerRow> = self
            .banner_base
            .iter()
            .map(|row| {
                let cells = row
                    .cells
                    .iter()
                    .map(|&(col, kind)| {
                        if corrupted.contains(&(row.row, col)) {
                            let ch = corrupt_map.get(&(row.row, col)).copied().unwrap_or('0');
                            (col, BannerCellKind::Glitch(ch))
                        } else {
                            (col, kind)
                        }
                    })
                    .collect();
                BannerRow {
                    row: row.row,
                    cells,
                }
            })
            .collect();

        // Border noise as glitch 0/1 cells
        if !self.border_noise.is_empty() {
            let mut by_row: std::collections::HashMap<usize, Vec<(usize, BannerCellKind)>> =
                std::collections::HashMap::new();
            for &(r, c) in &self.border_noise {
                let ch = if rand::random::<bool>() { '1' } else { '0' };
                by_row
                    .entry(r)
                    .or_default()
                    .push((c, BannerCellKind::Glitch(ch)));
            }
            for (r, cells) in by_row {
                rows.push(BannerRow { row: r, cells });
            }
        }

        rows
    }

    pub fn activate(&mut self) {
        self.active = true;
    }

    pub fn reset(&mut self) {
        self.active = false;
        self.home_since = Instant::now();
        let (grid, overlay, green_grid) = Self::make_banner_grid(self.rows, self.cols);
        self.grid = grid;
        self.green_grid = green_grid;
        let mut candidates: Vec<(usize, usize)> = overlay
            .iter()
            .flat_map(|row| row.cells.iter().map(move |&(col, _)| (row.row, col)))
            .collect();
        shuffle(&mut candidates);
        let corrupt_chars = candidates
            .iter()
            .map(|_| if rand::random::<bool>() { '1' } else { '0' })
            .collect();
        self.banner_base = overlay;
        self.corrupt_candidates = candidates;
        self.corrupt_chars = corrupt_chars;
        self.phase = Phase::Waiting;
        self.phase_started = Instant::now();
        self.total_glitch_cycles = MIN_GLITCH_CYCLES
            + rand::random::<u32>() as usize % (MAX_GLITCH_CYCLES - MIN_GLITCH_CYCLES + 1);
        self.glitch_cycle = 0;
        self.corrupt_count = 0;
        self.peak_corrupt = 0;
        self.border_noise.clear();
        self.next_between_ms = 0;
        self.vibration = (0, 0);
    }

    pub fn step(&mut self) {
        let mut next = vec![vec![CellState::Off; self.cols]; self.rows];
        let mut next_green = vec![vec![0u8; self.cols]; self.rows];

        for (r, row) in next.iter_mut().enumerate().take(self.rows) {
            for (c, cell) in row.iter_mut().enumerate().take(self.cols) {
                *cell = match self.grid[r][c] {
                    CellState::On => {
                        // Dying cells inherit their parent's green
                        next_green[r][c] = self.green_grid[r][c];
                        CellState::Dying
                    }
                    CellState::Dying => CellState::Off,
                    CellState::Off if self.count_on_neighbors(r, c) == 2 => {
                        // Newborn: average green of the 2 On neighbors
                        next_green[r][c] = self.avg_neighbor_green(r, c);
                        CellState::On
                    }
                    CellState::Off => CellState::Off,
                };
            }
        }
        self.grid = next;
        self.green_grid = next_green;
        self.validate_and_inject_noise();
    }

    /// Average green channel of On neighbors (used for newborn inheritance).
    fn avg_neighbor_green(&self, row: usize, col: usize) -> u8 {
        let mut sum = 0u32;
        let mut count = 0u32;
        for dr in [-1i32, 0, 1] {
            for dc in [-1i32, 0, 1] {
                if dr == 0 && dc == 0 {
                    continue;
                }
                let r = (row as i32 + dr).rem_euclid(self.rows as i32) as usize;
                let c = (col as i32 + dc).rem_euclid(self.cols as i32) as usize;
                if self.grid[r][c] == CellState::On {
                    sum += self.green_grid[r][c] as u32;
                    count += 1;
                }
            }
        }
        if let Some(avg) = sum.checked_div(count).map(|value| value as i16) {
            // Small random drift ±5 to create gradual color variation
            let drift = (rand::random::<i16>() % 11) - 5;
            (avg + drift).clamp(100, 255) as u8
        } else {
            BANNER_GREEN
        }
    }

    fn count_particles(&self) -> usize {
        self.grid
            .iter()
            .flatten()
            .filter(|&&cell| cell == CellState::On)
            .count()
    }

    fn is_edge_cell(&self, row: usize, col: usize) -> bool {
        row == 0 || row == self.rows - 1 || col == 0 || col == self.cols - 1
    }

    fn validate_and_inject_noise(&mut self) {
        let total_cells = self.rows * self.cols;
        let particle_ratio = self.count_particles() as f64 / total_cells as f64;
        if particle_ratio < MIN_PARTICLE_THRESHOLD {
            self.inject_edge_noise(EDGE_NOISE_PROBABILITY);
        } else if particle_ratio < LOW_ACTIVITY_THRESHOLD {
            self.inject_noise_pulse(EDGE_NOISE_PROBABILITY * 0.65);
        } else if rand::random::<f64>() < NOISE_PULSE_PROBABILITY {
            self.inject_noise_pulse(EDGE_PULSE_PROBABILITY);
        }
    }

    fn inject_noise_pulse(&mut self, probability: f64) {
        let burst_span = EDGE_PULSE_BURST_MAX - EDGE_PULSE_BURST_MIN + 1;
        let burst_limit = EDGE_PULSE_BURST_MIN + rand::random::<u32>() as usize % burst_span.max(1);
        let mut injected = 0usize;
        for _ in 0..2 {
            injected += self.inject_edge_noise_until(probability, burst_limit - injected);
            if injected >= burst_limit {
                break;
            }
        }
    }

    fn inject_edge_noise(&mut self, probability: f64) {
        let _ = self.inject_edge_noise_until(probability, usize::MAX);
    }

    fn inject_edge_noise_until(&mut self, probability: f64, max_injections: usize) -> usize {
        let mut injected = 0usize;
        for r in 0..self.rows {
            for c in 0..self.cols {
                if self.is_edge_cell(r, c)
                    && self.grid[r][c] == CellState::Off
                    && rand::random::<f64>() < probability
                {
                    self.grid[r][c] = CellState::On;
                    self.green_grid[r][c] = NOISE_GREEN;
                    injected += 1;
                    if injected >= max_injections {
                        return injected;
                    }
                }
            }
        }
        injected
    }

    fn count_on_neighbors(&self, row: usize, col: usize) -> usize {
        let mut count = 0;
        for dr in [-1i32, 0, 1] {
            for dc in [-1i32, 0, 1] {
                if dr == 0 && dc == 0 {
                    continue;
                }
                let r = (row as i32 + dr).rem_euclid(self.rows as i32) as usize;
                let c = (col as i32 + dc).rem_euclid(self.cols as i32) as usize;
                if self.grid[r][c] == CellState::On {
                    count += 1;
                }
            }
        }
        count
    }
}
