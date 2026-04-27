//! Animated home banner with fluid ASCII/Braille background field.

use std::time::Instant;

// ── Animation tuning (slower, more circular) ────────────────────────

const WAVE_SPEED_MS: u64 = 45;      // Much slower wave
const SHIMMER_SPEED_MS: u64 = 60;   // Much slower shimmer
const WAVE_CYCLE_STEPS: f32 = 1200.0;
const SHIMMER_CYCLE_STEPS: f32 = 1500.0;

// ── Reveal tuning (row-by-row typing) ─────────────────────────────

const REVEAL_CHAR_STEP_MS: u64 = 6;
const REVEAL_ROW_DELAY_MS: u64 = 55;

// ── Field tuning (sparse, circular, dark gray) ─────────────────────

// Fewer, more spaced-out glyphs - sparse density (no dot-only chars)
const FIELD_GLYPHS: [char; 5] = [' ', '░', '▒', '⠂', '⠆'];

fn hash01(x: i32, y: i32, t: i32) -> f32 {
    let mut n = (x as u32).wrapping_mul(374_761_393)
        ^ (y as u32).wrapping_mul(668_265_263)
        ^ (t as u32).wrapping_mul(2_147_483_647);
    n ^= n >> 13;
    n = n.wrapping_mul(1_274_126_177);
    ((n >> 8) & 0xFF_FFFF) as f32 / 0xFF_FFFF as f32
}

#[derive(Clone, Copy)]
pub enum BannerCellKind {
    Block,
    Shade,
}

#[derive(Clone)]
pub struct BannerRow {
    pub row: usize,
    pub cells: Vec<(usize, BannerCellKind)>,
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

pub struct BannerGlitch {
    pub rows: usize,
    pub cols: usize,
    banner_base: Vec<BannerRow>,
    wave_phase: f32,
    shimmer_phase: f32,
    reveal_started: Instant,
    mouse_energy: f32,
}

impl BannerGlitch {
    pub fn new(rows: usize, cols: usize) -> Self {
        Self {
            rows,
            cols,
            banner_base: Self::make_banner_overlay(rows, cols),
            wave_phase: 0.0,
            shimmer_phase: 0.0,
            reveal_started: Instant::now(),
            mouse_energy: 0.0,
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

    pub fn notify_mouse(&mut self) {
        self.mouse_energy = (self.mouse_energy + 0.35).min(1.0);
    }

    pub fn tick(&mut self) {
        let wave_step = WAVE_SPEED_MS as f32 / WAVE_CYCLE_STEPS;
        let shimmer_step = SHIMMER_SPEED_MS as f32 / SHIMMER_CYCLE_STEPS;
        self.wave_phase = (self.wave_phase + wave_step) % 1.0;
        self.shimmer_phase = (self.shimmer_phase + shimmer_step) % 1.0;
        self.mouse_energy *= 0.9;
    }

    pub fn visible_overlay(&self) -> (Vec<BannerRow>, f32) {
        let elapsed_ms = self.reveal_started.elapsed().as_millis() as u64;
        let rows = self
            .banner_base
            .iter()
            .enumerate()
            .filter_map(|(row_idx, row)| {
                let row_start_ms = row_idx as u64 * REVEAL_ROW_DELAY_MS;
                if elapsed_ms < row_start_ms {
                    return None;
                }

                let row_elapsed_ms = elapsed_ms - row_start_ms;
                let visible_cells =
                    ((row_elapsed_ms / REVEAL_CHAR_STEP_MS) as usize + 1).min(row.cells.len());
                if visible_cells == 0 {
                    return None;
                }

                Some(BannerRow {
                    row: row.row,
                    cells: row.cells.iter().take(visible_cells).copied().collect(),
                })
            })
            .collect();

        (rows, self.wave_phase)
    }

    pub fn field_at(&self, row: usize, col: usize) -> (char, u8) {
        let x = col as f32;
        let y = row as f32;
        let t = self.wave_phase * std::f32::consts::TAU;
        let s = self.shimmer_phase * std::f32::consts::TAU;

        // Circular motion: use sin/cos with slow rotation
        let angle = t * 0.15;  // Very slow rotation
        let radius = 3.5;      // Circular radius
        
        // Circular flow pattern - rotates slowly around center
        let flow_x = angle.cos() * radius + (s * 0.2).sin() * 1.5;
        let flow_y = angle.sin() * radius + (s * 0.25).cos() * 1.2;
        let px = x + flow_x;
        let py = y + flow_y;

        // Subtle domain warp for organic feel
        let warp_x = (py * 0.08 + t * 0.4).sin() * 1.2;
        let warp_y = (px * 0.08 - t * 0.35).cos() * 1.0;
        let qx = px + warp_x;
        let qy = py + warp_y;

        // Simpler, lower frequency waves for circular feel
        let low = (qx * 0.05 + qy * 0.05 + t * 0.3).sin() * 0.4;
        let mid = (qx * 0.08 - qy * 0.06 - t * 0.4).cos() * 0.25;
        let high = ((qx * 0.12 + qy * 0.1) + s * 0.8).sin() * 0.12;

        let cell_noise = hash01(col as i32, row as i32, (self.wave_phase * 400.0) as i32);
        let noise = (cell_noise - 0.5) * 0.12;
        let mouse_swirl = ((qx * 0.2 - qy * 0.15) + t * 1.2).sin() * 0.15 * self.mouse_energy;

        let value = (low + mid + high + noise + mouse_swirl + 1.0) * 0.5;
        // Lower density threshold - fewer particles visible
        let density = value.clamp(0.0, 1.0).powf(1.8);

        let idx = (density * (FIELD_GLYPHS.len() - 1) as f32).round() as usize;
        let glyph = FIELD_GLYPHS[idx.min(FIELD_GLYPHS.len() - 1)];
        // Darker gray: 20-80 range instead of 48-168
        let gray = (20.0 + density * 60.0) as u8;
        (glyph, gray)
    }
}
