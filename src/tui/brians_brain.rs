//! Brian's Brain cellular automaton for the home screen.
//!
//! 3-state automaton: On → Dying → Off → On
//! Rule: Off cell turns On if exactly 2 neighbors are On.
//! Uses toroidal wrapping so patterns flow across edges.

use std::time::Instant;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CellState {
    Off,
    On,
    Dying,
}

pub struct BriansBrain {
    pub grid: Vec<Vec<CellState>>,
    pub rows: usize,
    pub cols: usize,
    pub home_since: Instant,
    pub active: bool,
}

impl BriansBrain {
    pub fn new(rows: usize, cols: usize) -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos() as usize;
        Self {
            grid: Self::make_grid(rows, cols, seed),
            rows,
            cols,
            home_since: Instant::now(),
            active: false,
        }
    }

    /// Pseudo-random grid with ~12% On density. Time-based seed gives
    /// a unique pattern each time the screensaver activates.
    fn make_grid(rows: usize, cols: usize, seed: usize) -> Vec<Vec<CellState>> {
        let mut grid = vec![vec![CellState::Off; cols]; rows];
        for r in 0..rows {
            for c in 0..cols {
                let mut h = r
                    .wrapping_mul(2_654_435_761)
                    .wrapping_add(c.wrapping_mul(2_246_822_519))
                    ^ seed;
                h = h.wrapping_mul(1_013_904_223).wrapping_add(1_664_525);
                h ^= h >> 16;
                if h % 8 == 0 {
                    grid[r][c] = CellState::On;
                }
            }
        }
        grid
    }

    pub fn should_activate(&self) -> bool {
        self.home_since.elapsed().as_secs() >= 2 && !self.active
    }

    pub fn activate(&mut self) {
        self.active = true;
    }

    pub fn reset(&mut self) {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos() as usize;
        self.active = false;
        self.home_since = Instant::now();
        self.grid = Self::make_grid(self.rows, self.cols, seed);
    }

    pub fn step(&mut self) {
        let mut next = vec![vec![CellState::Off; self.cols]; self.rows];
        for r in 0..self.rows {
            for c in 0..self.cols {
                next[r][c] = match self.grid[r][c] {
                    CellState::On => CellState::Dying,
                    CellState::Dying => CellState::Off,
                    CellState::Off if self.count_on_neighbors(r, c) == 2 => CellState::On,
                    CellState::Off => CellState::Off,
                };
            }
        }
        self.grid = next;
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
