//! Pane snapshot capture and vt100 parsing into a cell grid for rendering.

use vt100::Parser;

// ---------------------------------------------------------------------------
// Cell grid
// ---------------------------------------------------------------------------

/// A single terminal cell with foreground and background colors.
#[derive(Debug, Clone)]
pub struct Cell {
    pub ch: char,
    pub fg: egui::Color32,
    pub bg: egui::Color32,
}

/// Parsed terminal screen grid.
pub struct CellGrid {
    cells: Vec<Vec<Option<Cell>>>,
    rows: u16,
    cols: u16,
}

impl CellGrid {
    pub fn rows(&self) -> u16 {
        self.rows
    }

    pub fn cols(&self) -> u16 {
        self.cols
    }

    pub fn cell(&self, row: u16, col: u16) -> Option<&Cell> {
        self.cells
            .get(row as usize)
            .and_then(|r| r.get(col as usize))
            .and_then(|c| c.as_ref())
    }
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Parse raw vt100 output from `capture-pane -e` into a `CellGrid`.
pub fn parse_snapshot(data: &[u8], rows: u16, cols: u16) -> CellGrid {
    // vt100 parser needs screen dimensions; we use the pane dimensions
    let rows = rows.max(1);
    let cols = cols.max(1);

    let mut parser = Parser::new(rows, cols, 0);
    parser.process(data);

    let screen = parser.screen();
    let mut cells: Vec<Vec<Option<Cell>>> = vec![vec![None; cols as usize]; rows as usize];

    for row in 0..rows {
        for col in 0..cols {
            if let Some(vt_cell) = screen.cell(row, col) {
                // Only render cells that have content or non-default background
                let contents = vt_cell.contents();
                let ch = if contents.is_empty() {
                    ' '
                } else {
                    contents.chars().next().unwrap_or(' ')
                };

                let fg = convert_fg_color(vt_cell.fgcolor());
                let bg = convert_bg_color(vt_cell.bgcolor());

                // Skip empty cells with default background for performance
                if ch == ' ' && bg == DEFAULT_BG {
                    continue;
                }

                cells[row as usize][col as usize] = Some(Cell { ch, fg, bg });
            }
        }
    }

    CellGrid { cells, rows, cols }
}

// ---------------------------------------------------------------------------
// Color conversion
// ---------------------------------------------------------------------------

const DEFAULT_FG: egui::Color32 = egui::Color32::from_rgb(192, 192, 192);
const DEFAULT_BG: egui::Color32 = egui::Color32::from_rgb(0, 0, 0);

fn convert_fg_color(c: vt100::Color) -> egui::Color32 {
    match c {
        vt100::Color::Default => DEFAULT_FG,
        vt100::Color::Rgb(r, g, b) => egui::Color32::from_rgb(r, g, b),
        vt100::Color::Idx(i) => ansi_color(i),
    }
}

fn convert_bg_color(c: vt100::Color) -> egui::Color32 {
    match c {
        vt100::Color::Default => DEFAULT_BG,
        vt100::Color::Rgb(r, g, b) => egui::Color32::from_rgb(r, g, b),
        vt100::Color::Idx(i) => ansi_color(i),
    }
}

/// Convert an ANSI 256-color index to egui::Color32.
fn ansi_color(i: u8) -> egui::Color32 {
    match i {
        // Standard 16 ANSI colors
        0 => egui::Color32::from_rgb(0, 0, 0),
        1 => egui::Color32::from_rgb(205, 0, 0),
        2 => egui::Color32::from_rgb(0, 205, 0),
        3 => egui::Color32::from_rgb(205, 205, 0),
        4 => egui::Color32::from_rgb(0, 0, 205),
        5 => egui::Color32::from_rgb(205, 0, 205),
        6 => egui::Color32::from_rgb(0, 205, 205),
        7 => egui::Color32::from_rgb(229, 229, 229),
        8 => egui::Color32::from_rgb(127, 127, 127),
        9 => egui::Color32::from_rgb(255, 0, 0),
        10 => egui::Color32::from_rgb(0, 255, 0),
        11 => egui::Color32::from_rgb(255, 255, 0),
        12 => egui::Color32::from_rgb(0, 0, 255),
        13 => egui::Color32::from_rgb(255, 0, 255),
        14 => egui::Color32::from_rgb(0, 255, 255),
        15 => egui::Color32::from_rgb(255, 255, 255),
        // 216 color cube (16..231)
        16..=231 => {
            let n = i - 16;
            let r = cube_level((n / 36) % 6);
            let g = cube_level((n / 6) % 6);
            let b = cube_level(n % 6);
            egui::Color32::from_rgb(r, g, b)
        }
        // Grayscale (232..255)
        232..=255 => {
            let level = (i - 232) * 10 + 8;
            egui::Color32::from_rgb(level, level, level)
        }
    }
}

fn cube_level(idx: u8) -> u8 {
    match idx {
        0 => 0,
        1 => 95,
        2 => 135,
        3 => 175,
        4 => 215,
        5 => 255,
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// Snapshot cache for rendering
// ---------------------------------------------------------------------------

/// Pre-computed snapshot ready for rendering.  Rebuilt only when pixel size changes.
pub struct SnapshotCache {
    /// Pre-computed shapes in local coordinates (origin at pane top-left).
    cells: Vec<CachedCell>,
    /// Pixel dimensions this cache was built for.
    pixel_w: f32,
    pixel_h: f32,
}

struct CachedCell {
    rect: egui::Rect,
    bg: egui::Color32,
    /// If present, a foreground block should be drawn at this relative rect
    /// (inset from `rect`, only for non-space characters).
    fg_rect: Option<egui::Rect>,
    fg: egui::Color32,
}

impl SnapshotCache {
    pub fn new() -> Self {
        SnapshotCache {
            cells: Vec::new(),
            pixel_w: 0.0,
            pixel_h: 0.0,
        }
    }

    /// Rebuild the cache if the pixel size changed.  Returns true if rebuilt.
    pub fn update(
        &mut self,
        grid: &CellGrid,
        pixel_w: f32,
        pixel_h: f32,
        default_bg: egui::Color32,
    ) -> bool {
        let pixel_w = pixel_w.max(1.0);
        let pixel_h = pixel_h.max(1.0);

        if (self.pixel_w - pixel_w).abs() < 0.5 && (self.pixel_h - pixel_h).abs() < 0.5 {
            return false; // no change
        }

        self.pixel_w = pixel_w;
        self.pixel_h = pixel_h;

        let rows = grid.rows();
        let cols = grid.cols();
        let cell_w = pixel_w / cols as f32;
        let cell_h = pixel_h / rows as f32;

        // Inset for the foreground block inside each cell.
        let inset = cell_w.min(cell_h) * 0.15;

        let mut cells = Vec::new();

        for row in 0..rows {
            for col in 0..cols {
                let (bg, fg, has_char) = if let Some(cell) = grid.cell(row, col) {
                    let ch = cell.ch;
                    (cell.bg, cell.fg, ch != ' ' && ch != '\0')
                } else {
                    (default_bg, DEFAULT_FG, false)
                };

                let x = col as f32 * cell_w;
                let y = row as f32 * cell_h;
                let rect =
                    egui::Rect::from_min_max(egui::pos2(x, y), egui::pos2(x + cell_w, y + cell_h));

                let fg_rect = if has_char {
                    Some(egui::Rect::from_min_max(
                        egui::pos2(x + inset, y + inset),
                        egui::pos2(x + cell_w - inset, y + cell_h - inset),
                    ))
                } else {
                    None
                };

                cells.push(CachedCell {
                    rect,
                    bg,
                    fg_rect,
                    fg,
                });
            }
        }

        self.cells = cells;
        true
    }

    /// Render the snapshot into the given painter at `offset` position, clipped to `clip_rect`.
    pub fn paint(&self, painter: &egui::Painter, offset: egui::Pos2, clip_rect: egui::Rect) {
        for cell in &self.cells {
            let rect = cell.rect.translate(offset.to_vec2());

            // Quick reject: skip if outside clip rect
            if !clip_rect.intersects(rect) {
                continue;
            }

            // Background
            painter.rect_filled(rect, 0.0, cell.bg);

            // Foreground block (solid color, no text)
            if let Some(fg_rect) = cell.fg_rect {
                let fg_rect = fg_rect.translate(offset.to_vec2());
                painter.rect_filled(fg_rect, 0.0, cell.fg);
            }
        }
    }
}

impl Default for SnapshotCache {
    fn default() -> Self {
        Self::new()
    }
}
