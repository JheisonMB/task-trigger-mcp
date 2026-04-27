//! Banner animation with gradient wave and occasional glitch.
//!
//! The banner displays with a smooth vertical gradient wave that scrolls up/down.
//! Periodically, a digital-corruption glitch effect interrupts the wave.

use std::time::Instant;

// ── Wave tuning ──────────────────────────────────────────────

const WAVE_SPEED_MS: u64 = 30; // ms per wave step
const GLITCH_INTERVAL_MIN_MS: u64 = 8000; // min time between glitches
const GLITCH_INTERVAL_MAX_MS: u64 = 15000; // max time between glitches

// ── Glitch tuning ──────────────────────────────────────────────

const MIN_GLITCH_CYCLES: usize = 2;
const MAX_GLITCH_CYCLES: usize = 4;
const DISINTEGRATE_MS: u64 = 400;

// ── Types ──────────────────────────────────────────────────────

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
    Wave,
    GlitchDisintegrating,
    GlitchBetweenCycles,
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

pub struct BannerGlitch {
    pub rows: usize,
    pub cols: usize,
    banner_base: Vec<BannerRow>,
    phase: Phase,
    phase_started: Instant,
    wave_offset: f32, // 0.0 to 1.0, cycles for wave animation
    next_glitch_at: Instant,
    glitch_cycle: usize,
    total_glitch_cycles: usize,
    corrupt_candidates: Vec<(usize, usize)>,
    corrupt_count: usize,
    peak_corrupt: usize,
    corrupt_chars: Vec<char>,
    border_noise: Vec<(usize, usize)>,
    next_between_ms: u64,
    pub vibration: (i16, i16),
}

impl BannerGlitch {
    pub fn new(rows: usize, cols: usize) -> Self {
        let overlay = Self::make_banner_overlay(rows, cols);
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
            rows,
            cols,
            banner_base: overlay,
            phase: Phase::Wave,
            phase_started: Instant::now(),
            wave_offset: 0.0,
            next_glitch_at: Instant::now()
                + std::time::Duration::from_millis(rand_between(
                    GLITCH_INTERVAL_MIN_MS,
                    GLITCH_INTERVAL_MAX_MS,
                )),
            glitch_cycle: 0,
            total_glitch_cycles: 0,
            corrupt_candidates: candidates,
            corrupt_count: 0,
            peak_corrupt: 0,
            corrupt_chars,
            border_noise: Vec::new(),
            next_between_ms: 0,
            vibration: (0, 0),
        }
    }

    fn make_banner_overlay(rows: usize, cols: usize) -> Vec<BannerRow> {
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
                    cells.push((c, BannerCellKind::Block));
                } else if ch == '░' {
                    cells.push((c, BannerCellKind::Shade));
                }
            }
            if !cells.is_empty() {
                rows_data.push(BannerRow { row: r, cells });
            }
        }

        rows_data
    }

    pub fn tick(&mut self) {
        match self.phase {
            Phase::Wave => {
                // Advance wave
                self.wave_offset += WAVE_SPEED_MS as f32 / 1000.0;
                if self.wave_offset >= 1.0 {
                    self.wave_offset -= 1.0;
                }

                // Check if it's time for glitch
                if Instant::now() >= self.next_glitch_at {
                    self.start_glitch();
                }
            }
            Phase::GlitchDisintegrating => {
                let elapsed = self.phase_started.elapsed().as_millis() as u64;
                let progress = (elapsed as f64 / DISINTEGRATE_MS as f64).min(1.0);
                self.corrupt_count =
                    ((progress * self.peak_corrupt as f64) as usize).min(self.peak_corrupt);

                let max_shake = 1i16;
                self.vibration = (
                    (rand::random::<i16>() % (max_shake * 2 + 1)) - max_shake,
                    (rand::random::<i16>() % (max_shake * 2 + 1)) - max_shake,
                );

                if elapsed % 60 < 16 {
                    self.inject_border_noise_incremental(3 + (progress * 8.0) as usize);
                }

                if elapsed >= DISINTEGRATE_MS {
                    self.corrupt_count = 0;
                    self.vibration = (0, 0);
                    self.border_noise.clear();
                    self.glitch_cycle += 1;
                    if self.glitch_cycle >= self.total_glitch_cycles {
                        self.end_glitch();
                    } else {
                        self.next_between_ms = rand_between(300, 800);
                        self.phase = Phase::GlitchBetweenCycles;
                        self.phase_started = Instant::now();
                    }
                }
            }
            Phase::GlitchBetweenCycles => {
                if self.phase_started.elapsed().as_millis() as u64 >= self.next_between_ms {
                    self.start_corruption_cycle();
                }
            }
        }
    }

    fn start_glitch(&mut self) {
        self.total_glitch_cycles = MIN_GLITCH_CYCLES
            + rand::random::<u32>() as usize % (MAX_GLITCH_CYCLES - MIN_GLITCH_CYCLES + 1);
        self.glitch_cycle = 0;
        self.start_corruption_cycle();
    }

    fn start_corruption_cycle(&mut self) {
        shuffle(&mut self.corrupt_candidates);
        for ch in self.corrupt_chars.iter_mut() {
            *ch = if rand::random::<bool>() { '1' } else { '0' };
        }
        let total = self.corrupt_candidates.len();
        let frac = 0.25 + rand::random::<f64>() * 0.25;
        self.peak_corrupt = ((total as f64 * frac) as usize).max(1);
        self.corrupt_count = 0;
        self.border_noise.clear();
        self.phase = Phase::GlitchDisintegrating;
        self.phase_started = Instant::now();
    }

    fn end_glitch(&mut self) {
        self.phase = Phase::Wave;
        self.phase_started = Instant::now();
        self.corrupt_count = 0;
        self.vibration = (0, 0);
        self.border_noise.clear();
        self.next_glitch_at = Instant::now()
            + std::time::Duration::from_millis(rand_between(
                GLITCH_INTERVAL_MIN_MS,
                GLITCH_INTERVAL_MAX_MS,
            ));
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

    /// Build the overlay for rendering. Returns banner rows with glitch corruption
    /// and the current wave offset for gradient calculation.
    pub fn visible_overlay(&self) -> (Vec<BannerRow>, f32) {
        let corrupted: std::collections::HashSet<(usize, usize)> = self.corrupt_candidates
            [..self.corrupt_count]
            .iter()
            .cloned()
            .collect();

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

        (rows, self.wave_offset)
    }
}
