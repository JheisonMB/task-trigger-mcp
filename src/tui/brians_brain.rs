//! Brian's Brain cellular automaton for the home screen.
//!
//! 3-state automaton: On → Dying → Off → On
//! Rule: Off cell turns On if exactly 2 neighbors are On.

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
        let mut grid = vec![vec![CellState::Off; cols]; rows];
        // Seed initial pattern in center
        let mid_r = rows / 2;
        let mid_c = cols / 2;
        // Simple seed: a few On cells
        let seed = [(0, 0), (0, 1), (1, 0), (-1, 0), (0, -1)];
        for (dr, dc) in seed {
            let r = (mid_r as isize + dr) as usize;
            let c = (mid_c as isize + dc) as usize;
            if r < rows && c < cols {
                grid[r][c] = CellState::On;
            }
        }

        Self {
            grid,
            rows,
            cols,
            home_since: Instant::now(),
            active: false,
        }
    }

    pub fn should_activate(&self) -> bool {
        self.home_since.elapsed().as_secs() >= 2 && !self.active
    }

    pub fn activate(&mut self) {
        self.active = true;
        self.home_since = Instant::now();
    }

    #[allow(dead_code)]
    pub fn reset(&mut self) {
        self.active = false;
        self.grid = vec![vec![CellState::Off; self.cols]; self.rows];
    }

    pub fn step(&mut self) {
        let mut next = vec![vec![CellState::Off; self.cols]; self.rows];
        for (r, row) in self.grid.iter().enumerate() {
            for (c, _) in row.iter().enumerate() {
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

    fn count_on_neighbors(&self, row: usize, col: usize) -> usize {
        let mut count = 0;
        for dr in -1..=1 {
            for dc in -1..=1 {
                if dr == 0 && dc == 0 {
                    continue;
                }
                let r = row as isize + dr;
                let c = col as isize + dc;
                if r >= 0
                    && r < self.rows as isize
                    && c >= 0
                    && c < self.cols as isize
                    && self.grid[r as usize][c as usize] == CellState::On
                {
                    count += 1;
                }
            }
        }
        count
    }
}
