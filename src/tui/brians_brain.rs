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

/// Minimum seconds before starting glitch effects.
const INITIAL_DELAY_SECONDS: u64 = 1;

/// Probability of character corruption during glitch phase.
const GLITCH_CORRUPTION_PROBABILITY: f64 = 0.3;

/// Minimum glitch iterations before automaton activation.
const MIN_GLITCH_ITERATIONS: usize = 1;

/// Maximum glitch iterations before automaton activation.
const MAX_GLITCH_ITERATIONS: usize = 4;

/// Minimum milliseconds between glitch effects.
const MIN_GLITCH_INTERVAL_MS: u64 = 200;

/// Maximum milliseconds between glitch effects.
const MAX_GLITCH_INTERVAL_MS: u64 = 1000;

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
    /// Characters in this row: (`col_index`, `is_shade`).
    pub cells: Vec<(usize, bool)>,
}

pub struct BriansBrain {
    pub grid: Vec<Vec<CellState>>,
    pub rows: usize,
    pub cols: usize,
    pub home_since: Instant,
    pub active: bool,
    /// Full banner overlay grouped by row.
    banner_overlay: Vec<BannerRow>,
    /// Current glitch iteration count.
    glitch_iteration: usize,
    /// Total glitch iterations for this session.
    total_glitch_iterations: usize,
    /// Whether we're in glitch phase (before activation).
    glitch_phase: bool,
    /// Timestamp of last glitch effect.
    last_glitch_time: Instant,
    /// Random interval between glitch effects.
    glitch_interval_ms: u64,
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
        let (grid, overlay) = Self::make_banner_grid(rows, cols);
        let total_glitch_iterations = rand::random::<u32>() as usize % (MAX_GLITCH_ITERATIONS - MIN_GLITCH_ITERATIONS + 1) + MIN_GLITCH_ITERATIONS;
        let glitch_interval_ms = rand::random::<u32>() as u64 % (MAX_GLITCH_INTERVAL_MS - MIN_GLITCH_INTERVAL_MS + 1) + MIN_GLITCH_INTERVAL_MS;
        
        Self {
            grid,
            rows,
            cols,
            home_since: Instant::now(),
            active: false,
            banner_overlay: overlay,
            glitch_iteration: 0,
            total_glitch_iterations,
            glitch_phase: false,
            last_glitch_time: Instant::now(),
            glitch_interval_ms,
        }
    }

    /// Seed the grid from the CANOPY banner text.
    /// Only full block characters (`█`) become On cells — they drive the explosion.
    /// Light shade characters (`░`) are recorded in the overlay for pre-activation
    /// rendering but do NOT participate in the automaton (they fade away).
    fn make_banner_grid(rows: usize, cols: usize) -> (Vec<Vec<CellState>>, Vec<BannerRow>) {
        let mut grid = vec![vec![CellState::Off; cols]; rows];
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
                    cells.push((c, false));
                } else if ch == '░' {
                    cells.push((c, true));
                }
            }
            if !cells.is_empty() {
                rows_data.push(BannerRow { row: r, cells });
            }
        }

        (grid, rows_data)
    }

    pub fn should_activate(&self) -> bool {
        // Activate after all glitch iterations complete
        self.glitch_iteration >= self.total_glitch_iterations && !self.active
    }

    /// Advance the animation/glitch state. Returns true if automaton just activated.
    pub fn tick(&mut self) -> bool {
        if self.active {
            return false;
        }

        // Start glitch phase after initial delay
        if !self.glitch_phase && self.home_since.elapsed().as_secs() >= INITIAL_DELAY_SECONDS {
            self.glitch_phase = true;
            self.last_glitch_time = Instant::now();
        }

        // During glitch phase, apply glitch effects at random intervals
        if self.glitch_phase 
            && self.glitch_iteration < self.total_glitch_iterations
            && self.last_glitch_time.elapsed().as_millis() >= self.glitch_interval_ms as u128 {
            self.apply_glitch_effects();
            self.glitch_iteration += 1;
            self.last_glitch_time = Instant::now();
            // Randomize interval for next glitch
            self.glitch_interval_ms = rand::random::<u32>() as u64 % (MAX_GLITCH_INTERVAL_MS - MIN_GLITCH_INTERVAL_MS + 1) + MIN_GLITCH_INTERVAL_MS;
        }

        // Check if we should activate
        self.should_activate()
    }

    /// Get all banner rows (now shown complete from start).
    pub fn visible_overlay(&self) -> Vec<&BannerRow> {
        self.banner_overlay.iter().collect()
    }

    /// Apply random glitch effects to the banner grid.
    fn apply_glitch_effects(&mut self) {
        // Randomly corrupt some characters in the banner
        for row_data in &self.banner_overlay {
            for &(col, _is_shade) in &row_data.cells {
                if rand::random::<f64>() < GLITCH_CORRUPTION_PROBABILITY {
                    // Corrupt the cell state
                    self.grid[row_data.row][col] = match self.grid[row_data.row][col] {
                        CellState::On => CellState::Off,
                        CellState::Off => CellState::On,
                        CellState::Dying => CellState::On,
                    };
                }
            }
        }

        // Occasionally add random noise
        if rand::random::<f64>() < 0.5 {
            for _ in 0..10 {
                let row = rand::random::<u32>() as usize % self.rows;
                let col = rand::random::<u32>() as usize % self.cols;
                self.grid[row][col] = CellState::On;
            }
        }

        // Final iteration: dramatic explosion effect
        if self.glitch_iteration + 1 == self.total_glitch_iterations {
            self.apply_explosion_effect();
        }
    }

    /// Apply dramatic explosion effect for final activation.
    fn apply_explosion_effect(&mut self) {
        // Create explosion pattern from banner center
        let center_row = self.rows / 2;
        let center_col = self.cols / 2;

        // Explode outward in concentric circles
        let max_radius = (self.rows.min(self.cols) / 2) as i32;
        for radius in 1..=max_radius {
            for angle in 0..360 {
                if rand::random::<f64>() < 0.7 { // 70% chance to place cell
                    let rad = angle as f64 * std::f64::consts::PI / 180.0;
                    let row = center_row as i32 + (rad.sin() * radius as f64) as i32;
                    let col = center_col as i32 + (rad.cos() * radius as f64) as i32;
                    
                    if row >= 0 && row < self.rows as i32 && col >= 0 && col < self.cols as i32 {
                        self.grid[row as usize][col as usize] = CellState::On;
                    }
                }
            }
        }

        // Add some random sparks
        for _ in 0..50 {
            let row = rand::random::<u32>() as usize % self.rows;
            let col = rand::random::<u32>() as usize % self.cols;
            self.grid[row][col] = CellState::On;
        }
    }

    pub fn activate(&mut self) {
        self.active = true;
    }

    pub fn reset(&mut self) {
        self.active = false;
        self.home_since = Instant::now();
        let (grid, overlay) = Self::make_banner_grid(self.rows, self.cols);
        self.grid = grid;
        self.banner_overlay = overlay;
        self.glitch_iteration = 0;
        self.total_glitch_iterations = rand::random::<u32>() as usize % (MAX_GLITCH_ITERATIONS - MIN_GLITCH_ITERATIONS + 1) + MIN_GLITCH_ITERATIONS;
        self.glitch_phase = false;
        self.last_glitch_time = Instant::now();
        self.glitch_interval_ms = rand::random::<u32>() as u64 % (MAX_GLITCH_INTERVAL_MS - MIN_GLITCH_INTERVAL_MS + 1) + MIN_GLITCH_INTERVAL_MS;
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
        for r in 0..self.rows {
            for c in 0..self.cols {
                // Only inject noise at edge cells
                if self.is_edge_cell(r, c)
                    && self.grid[r][c] == CellState::Off
                    && rand::random::<f64>() < EDGE_NOISE_PROBABILITY
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
