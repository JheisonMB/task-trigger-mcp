//! Brian's Brain cellular automaton for the home screen.
//!
//! 3-state automaton: On → Dying → Off → On
//! Rule: Off cell turns On if exactly 2 neighbors are On.
//! Uses toroidal wrapping so patterns flow across edges.
//!
//! The grid is seeded from the CANOPY banner text so the automaton
//! looks like the banner "exploding" when it activates.

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
        Self {
            grid: Self::make_banner_grid(rows, cols),
            rows,
            cols,
            home_since: Instant::now(),
            active: false,
        }
    }

    /// Seed the grid from the CANOPY banner text.
    /// Only the solid block characters (`█`) become On cells — these are the
    /// most prominent characters in the banner.  The automaton rules alone
    /// create a natural explosion wave radiating outward from the banner shape.
    fn make_banner_grid(rows: usize, cols: usize) -> Vec<Vec<CellState>> {
        let mut grid = vec![vec![CellState::Off; cols]; rows];

        let banner_h = BANNER.len();
        let banner_w = BANNER.iter().map(|l| l.chars().count()).max().unwrap_or(0);

        let top = rows.saturating_sub(banner_h) / 2;
        let left = cols.saturating_sub(banner_w) / 2;

        for (br, line) in BANNER.iter().enumerate() {
            let r = top + br;
            if r >= rows {
                break;
            }
            for (bc, ch) in line.chars().enumerate() {
                let c = left + bc;
                if c >= cols {
                    break;
                }
                // Only full-block characters seed the automaton
                if ch == '█' {
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
        self.active = false;
        self.home_since = Instant::now();
        self.grid = Self::make_banner_grid(self.rows, self.cols);
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
