//! Pane snapshot capture and vt100 parsing into grayscale pixel buffers.
//!
//! Each terminal cell becomes one pixel in a minimap-style texture.
//! No glyphs, fonts, or colour palettes — just three brightness levels.

use vt100::Parser;

// ---------------------------------------------------------------------------
// Grayscale snapshot
// ---------------------------------------------------------------------------

/// A grayscale pixel buffer representing a pane's terminal content.
///
/// One byte per terminal cell, row-major: `pixels[y * cols + x]`.
/// Length is always `cols * rows`.
pub struct GrayscaleSnapshot {
    pub pixels: Vec<u8>,
    /// Width in cells (columns).
    pub cols: u16,
    /// Height in cells (rows).
    pub rows: u16,
}

// ---------------------------------------------------------------------------
// Brightness levels
// ---------------------------------------------------------------------------

/// Blank cells and default background.
const DARK: u8 = 8;
/// Light punctuation: `.` `,` `'` `-` etc.
const LIGHT: u8 = 128;
/// Most letters, digits, and symbols.
const MEDIUM: u8 = 200;
/// Heavy glyphs (`#` `M` `█`) and any cell with a non-default background.
const HEAVY: u8 = 255;

// ---------------------------------------------------------------------------
// Build from vt100 capture
// ---------------------------------------------------------------------------

/// Parse raw vt100 output from `capture-pane -e` into a `GrayscaleSnapshot`.
///
/// `pane_rows` and `pane_cols` should be the pane's real `#{pane_height}` and
/// `#{pane_width}` so the parser allocates the correct screen size and lines
/// don't wrap or shift.
pub fn build_grayscale_snapshot(data: &[u8], pane_rows: u16, pane_cols: u16) -> GrayscaleSnapshot {
    let rows = pane_rows.max(1);
    let cols = pane_cols.max(1);

    let mut parser = Parser::new(rows, cols, 0);

    // tmux capture-pane -p outputs bare \n (LF without CR).  The vt100
    // parser treats \n as line-feed-only, so each row continues at the
    // previous column instead of resetting to 0.  Normalise bare \n → \r\n.
    let normalized = normalize_newlines(data);
    parser.process(&normalized);

    let screen = parser.screen();

    // Use the screen's own size so the stride is always consistent with the
    // cell grid, even if the caller passed something slightly different.
    let (screen_rows, screen_cols) = screen.size();
    let rows = screen_rows.max(1);
    let cols = screen_cols.max(1);

    let total = (rows as usize) * (cols as usize);
    let mut pixels: Vec<u8> = vec![DARK; total];

    for row in 0..rows {
        let row_base = (row as usize) * (cols as usize);
        for col in 0..cols {
            let idx = row_base + (col as usize);

            let brightness = if let Some(vt_cell) = screen.cell(row, col) {
                let contents = vt_cell.contents();
                let ch = if contents.is_empty() {
                    ' '
                } else {
                    contents.chars().next().unwrap_or(' ')
                };

                if !is_default_color(vt_cell.bgcolor()) {
                    // Non-default background → bright band (status bars, selections).
                    HEAVY
                } else if ch == ' ' {
                    DARK
                } else {
                    char_brightness(ch)
                }
            } else {
                DARK
            };

            pixels[idx] = brightness;
        }
    }

    GrayscaleSnapshot { pixels, cols, rows }
}

// ---------------------------------------------------------------------------
// Line-ending normalization
// ---------------------------------------------------------------------------

/// Convert bare `\n` (LF) to `\r\n` (CRLF) so the vt100 parser resets the
/// column on every newline.  Existing `\r\n` sequences are left unchanged.
fn normalize_newlines(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + data.len() / 80);
    for &b in data {
        if b == b'\n' && out.last() != Some(&b'\r') {
            out.push(b'\r');
        }
        out.push(b);
    }
    out
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn is_default_color(c: vt100::Color) -> bool {
    matches!(c, vt100::Color::Default)
}

/// Three brightness levels by character weight.
fn char_brightness(ch: char) -> u8 {
    match ch {
        // ---- light punctuation ----
        '.' | ',' | '\'' | '`' | '´' | '-' | '_' | ':' | ';' => LIGHT,

        // ---- heavy glyphs ----
        '#' | 'M' | 'W' | 'm' | 'w' | '@' | '&' | '%' | '$' | '█' | '▓' | '▒' | '░' | '▇' | '▆'
        | '▅' | '▄' | '▃' | '▂' | '▁' | 'B' | 'D' | 'E' | 'F' | 'G' | 'H' | 'K' | 'N' | 'Q'
        | 'R' | 'X' | 'Z' => HEAVY,

        // ---- everything else (letters, digits, symbols) ----
        _ => MEDIUM,
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a small known screen (3 lines of different lengths) and assert
    /// that bright pixels land at the expected (x, y) cells.
    #[test]
    fn test_bright_pixels_at_expected_positions() {
        // Raw vt100: clear screen, then place text at known positions.
        // Row 1, col 0: "abc"
        // Row 2, col 4: "HELLO"
        // Row 4, col 10: "x"
        let data = b"\x1b[2J\x1b[H\x1b[2;1Habc\x1b[3;5HHELLO\x1b[5;11Hx";

        let snap = build_grayscale_snapshot(data, 6, 20);

        // Verify dimensions.
        assert_eq!(snap.cols, 20, "cols");
        assert_eq!(snap.rows, 6, "rows");
        assert_eq!(snap.pixels.len(), 20 * 6);

        // Helper: get pixel at (col, row) = (x, y).
        let get = |x: u16, y: u16| -> u8 {
            snap.pixels[(y as usize) * (snap.cols as usize) + (x as usize)]
        };

        // Row 1 (y=1): "abc" at cols 0,1,2 should be bright (MEDIUM for letters).
        assert!(get(0, 1) >= MEDIUM, "a at (0,1)");
        assert!(get(1, 1) >= MEDIUM, "b at (1,1)");
        assert!(get(2, 1) >= MEDIUM, "c at (2,1)");
        // Col 3 should be dark.
        assert_eq!(get(3, 1), DARK);

        // Row 2 (y=2): "HELLO" at cols 4..9 should be bright.
        for x in 4..9 {
            assert!(get(x, 2) >= MEDIUM, "HELLO char at ({},2)", x);
        }
        // Col 3 should be dark (gap before HELLO).
        assert_eq!(get(3, 2), DARK);

        // Row 4 (y=4): "x" at col 10 should be bright.
        assert!(get(10, 4) >= MEDIUM, "x at (10,4)");
        // Col 9 should be dark.
        assert_eq!(get(9, 4), DARK);

        // Row 0 should be all dark (blank line).
        for x in 0..20 {
            assert_eq!(get(x, 0), DARK, "blank row 0 col {}", x);
        }

        // Row 3 should be all dark.
        for x in 0..20 {
            assert_eq!(get(x, 3), DARK, "blank row 3 col {}", x);
        }

        // Row 5 should be all dark.
        for x in 0..20 {
            assert_eq!(get(x, 5), DARK, "blank row 5 col {}", x);
        }
    }

    /// A pane with only spaces should be all-dark.
    #[test]
    fn test_blank_pane_all_dark() {
        let data = b"\x1b[2J\x1b[H"; // clear screen, cursor home
        let snap = build_grayscale_snapshot(data, 5, 10);
        assert_eq!(snap.pixels.len(), 50);
        for &p in &snap.pixels {
            assert_eq!(p, DARK);
        }
    }

    /// Bare \n (as emitted by tmux capture-pane -p) must reset the column to
    /// 0 so each row of text starts at x=0.
    #[test]
    fn test_bare_newline_resets_column() {
        // "ab\ncdef\ng" should appear as:
        //   row 0: "ab"   at cols 0,1
        //   row 1: "cdef" at cols 0-3
        //   row 2: "g"    at col 0
        let data = b"ab\ncdef\ng";
        let snap = build_grayscale_snapshot(data, 5, 10);

        let get = |x: u16, y: u16| -> u8 {
            snap.pixels[(y as usize) * (snap.cols as usize) + (x as usize)]
        };

        // Row 0: "ab" at cols 0,1; col 2 should be dark.
        assert!(get(0, 0) >= MEDIUM, "a at (0,0)");
        assert!(get(1, 0) >= MEDIUM, "b at (1,0)");
        assert_eq!(get(2, 0), DARK, "dark at (2,0)");

        // Row 1: "cdef" at cols 0-3; col 4 should be dark.
        for x in 0..4 {
            assert!(get(x, 1) >= MEDIUM, "cdef char at ({},1)", x);
        }
        assert_eq!(get(4, 1), DARK, "dark at (4,1)");

        // Row 2: "g" at col 0; col 1 should be dark.
        assert!(get(0, 2) >= MEDIUM, "g at (0,2)");
        assert_eq!(get(1, 2), DARK, "dark at (1,2)");
    }

    /// Cells with non-default background (e.g. a coloured status bar) must be
    /// bright regardless of the character content.
    #[test]
    fn test_non_default_background_is_bright() {
        // Set background to red (ANSI 41) on row 0, then write a space.
        // \x1b[41m = red background.
        let data = b"\x1b[2J\x1b[H\x1b[41m \x1b[0m";
        let snap = build_grayscale_snapshot(data, 3, 10);
        let get = |x: u16, y: u16| -> u8 {
            snap.pixels[(y as usize) * (snap.cols as usize) + (x as usize)]
        };
        // Cell (0,0) should be bright (red background), even though it's a space.
        assert_eq!(get(0, 0), HEAVY);
        // Cell (1,0) should be dark (default background, space).
        assert_eq!(get(1, 0), DARK);
    }
}
