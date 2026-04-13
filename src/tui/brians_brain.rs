//! Brian's Brain cellular automaton for the home screen.
//!
//! 3-state automaton: On → Dying → Off → On
//! Rule: Off cell turns On if exactly 2 neighbors are On.
//! Uses toroidal wrapping so patterns flow across edges.
//!
//! The grid is seeded from the CANOPY banner text so the automaton
//! looks like the banner "exploding" when it activates.
//!
//! Includes automatic particle count validation and noise injection to prevent
//! the automaton from stabilizing with too few particles.

use std::time::Instant;

/// Minimum percentage of particles (relative to total cells) to maintain activity.
/// Below this threshold, edge noise is injected to keep the automaton fluid.
const MIN_PARTICLE_THRESHOLD: f64 = 0.005; // 0.5% of cells

/// Probability of injecting noise at edge cells when below threshold.
const EDGE_NOISE_PROBABILITY: f64 = 0.15; // 15% chance per edge cell

/// Number of banner rows to reveal per side per tick during the unfold animation.
const REVEAL_RATE: usize = 1;

/// Minimum seconds before the unfold animation completes (at least this long).
const UNFOLD_SECONDS: u64 = 1;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CellState {
    Off,
    On,
    Dying,
}

/// Banner row data for the overlay.
#[derive(Clone)]
pub struct BannerRow {
    /// Grid row index.
    pub row: usize,
    /// Characters in this row: (col_index, is_shade).
    pub cells: Vec<(usize, bool)>,
}

pub struct BriansBrain {
    pub grid: Vec<Vec<CellState>>,
    pub rows: usize,
    pub cols: usize,
    pub home_since: Instant,
    pub active: bool,
    /// Full banner overlay grouped by row for progressive reveal.
    banner_overlay: Vec<BannerRow>,
    /// Center row index in the overlay.
    overlay_center: usize,
    /// Number of rows revealed from center during unfold animation.
    reveal_radius: usize,
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

impl BriansBrain {
    pub fn new(rows: usize, cols: usize) -> Self {
        let (grid, overlay, center_idx) = Self::make_banner_grid(rows, cols);
        Self {
            grid,
            rows,
            cols,
            home_since: Instant::now(),
            active: false,
            banner_overlay: overlay,
            overlay_center: center_idx,
            reveal_radius: 0,
        }
    }

    /// Seed the grid from the CANOPY banner text.
    /// Only full block characters (`█`) become On cells — they drive the explosion.
    /// Light shade characters (`░`) are recorded in the overlay for pre-activation
    /// rendering but do NOT participate in the automaton (they fade away).
    fn make_banner_grid(rows: usize, cols: usize) -> (Vec<Vec<CellState>>, Vec<BannerRow>, usize) {
        let mut grid = vec![vec![CellState::Off; cols]; rows];
        let mut rows_data: Vec<BannerRow> = Vec::new();

        let banner_h = BANNER.len();
        let banner_w = BANNER.iter().map(|l| l.chars().count()).max().unwrap_or(0);

        let top = rows.saturating_sub(banner_h) / 2;
        let left = cols.saturating_sub(banner_w) / 2;

        // Center row of the banner relative to the overlay
        let center = banner_h / 2;

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
                    cells.push((c, false));
                } else if ch == '░' {
                    cells.push((c, true));
                }
            }
            if !cells.is_empty() {
                rows_data.push(BannerRow { row: r, cells });
            }
        }

        // Find the center index in rows_data (closest to the actual center row of the banner)
        let center_idx = rows_data
            .iter()
            .position(|rd| rd.row >= top + center)
            .unwrap_or(rows_data.len().saturating_sub(1));

        (grid, rows_data, center_idx)
    }

    pub fn should_activate(&self) -> bool {
        // Wait for unfold animation to complete before activating
        self.home_since.elapsed().as_secs() >= UNFOLD_SECONDS
            && self.reveal_radius >= self.overlay_center.max(self.banner_overlay.len().saturating_sub(1) - self.overlay_center)
            && !self.active
    }

    /// Advance the unfold animation by one step. Returns true if the animation just completed.
    pub fn tick(&mut self) -> bool {
        if self.active {
            return false;
        }
        let max_dist = self.overlay_center.max(self.banner_overlay.len().saturating_sub(1) - self.overlay_center);
        if self.reveal_radius < max_dist {
            self.reveal_radius = (self.reveal_radius + REVEAL_RATE).min(max_dist);
        }
        self.reveal_radius >= max_dist
    }

    /// Get the currently visible banner rows based on the reveal radius.
    /// Returns rows sorted by distance from center (innermost first).
    pub fn visible_overlay(&self) -> Vec<&BannerRow> {
        if self.reveal_radius == 0 {
            return vec![];
        }
        // Collect rows within reveal distance from center, sorted by distance
        let mut visible: Vec<_> = self
            .banner_overlay
            .iter()
            .enumerate()
            .filter(|(i, _)| (*i as i64 - self.overlay_center as i64).unsigned_abs() <= self.reveal_radius as u64)
            .collect();
        visible.sort_by_key(|(i, _)| (*i as i64 - self.overlay_center as i64).unsigned_abs());
        visible.into_iter().map(|(_, r)| r).collect()
    }

    pub fn activate(&mut self) {
        self.active = true;
    }

    pub fn reset(&mut self) {
        self.active = false;
        self.home_since = Instant::now();
        let (grid, overlay, center_idx) = Self::make_banner_grid(self.rows, self.cols);
        self.grid = grid;
        self.banner_overlay = overlay;
        self.overlay_center = center_idx;
        self.reveal_radius = 0;
    }

    pub fn step(&mut self) {
        let mut next = vec![vec![CellState::Off; self.cols]; self.rows];
        for (r, row) in next.iter_mut().enumerate().take(self.rows) {
            for (c, cell) in row.iter_mut().enumerate().take(self.cols) {
                *cell = match self.grid[r][c] {
                    CellState::On => CellState::Dying,
                    CellState::Dying => CellState::Off,
                    CellState::Off if self.count_on_neighbors(r, c) == 2 => CellState::On,
                    CellState::Off => CellState::Off,
                };
            }
        }
        self.grid = next;

        // Validate particle count and inject noise if too low
        self.validate_and_inject_noise();
    }

    /// Count the total number of On particles in the grid.
    fn count_particles(&self) -> usize {
        self.grid
            .iter()
            .flatten()
            .filter(|&&cell| cell == CellState::On)
            .count()
    }

    /// Check if a cell is on the edge of the grid.
    fn is_edge_cell(&self, row: usize, col: usize) -> bool {
        row == 0 || row == self.rows - 1 || col == 0 || col == self.cols - 1
    }

    /// Validate particle count and inject random noise at edges if below threshold.
    fn validate_and_inject_noise(&mut self) {
        let total_cells = self.rows * self.cols;
        let particle_count = self.count_particles();
        let particle_ratio = particle_count as f64 / total_cells as f64;

        // If particles drop below threshold, inject noise at edges
        if particle_ratio < MIN_PARTICLE_THRESHOLD {
            self.inject_edge_noise();
        }
    }

    /// Inject random noise at edge cells to reinvigorate the automaton.
    fn inject_edge_noise(&mut self) {
        use rand::Rng;
        let mut rng = rand::thread_rng();

        for r in 0..self.rows {
            for c in 0..self.cols {
                // Only inject noise at edge cells
                if self.is_edge_cell(r, c)
                    && self.grid[r][c] == CellState::Off
                    && rng.gen_bool(EDGE_NOISE_PROBABILITY)
                {
                    self.grid[r][c] = CellState::On;
                }
            }
        }
    }

    /// Count On neighbours with toroidal (wrap-around) boundaries.
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
