# tmux Pane Rearranger: Specification

## 1. Overview
A GUI launched from a tmux key binding that shows the visible pane layout of the originating window, lets the user rearrange panes by mouse drag-and-drop, and on save applies the new layout to the real tmux window.

## 2. Platforms and Requirements
- Linux (X11 and Wayland), macOS, WSL2 (WSLg).
- No tiling WM assumed; the window is a normal floating window.
- tmux: latest, including patched `next-3.9`. No backward-compatibility goal.
- Rust, `eframe`/`egui`. Backend: `wgpu` on macOS; `glow` or `wgpu` elsewhere.

## 3. Launch and CLI
```
tmux-rearrange --window <window_id>
```
Example binding:
```
bind g run-shell -b "tmux-rearrange --window '#{window_id}'"
```
- All tmux calls target `--window` explicitly.
- If no display is available, print a clear error and exit with code 3.

### Exit codes
| Code | Meaning |
|---|---|
| 0 | Saved; layout applied |
| 1 | Cancelled; no changes |
| 2 | Window is zoomed; refused |
| 3 | Environment/tmux error (no display, tmux not found, command failed) |

## 4. Data Acquisition
1. `tmux display-message -p -t <window> '#{window_zoomed_flag}'`; if `1`, exit 2.
2. `tmux display-message -p -t <window> '#{window_visible_layout}'` is parsed into the layout tree.
3. `tmux list-panes -t <window> -F '#{pane_id} #{pane_width} #{pane_height} #{pane_active}'` supplies pane metadata.
4. `tmux capture-pane -p -e -t <pane_id>` per pane (in parallel) for snapshots.

## 5. Model
```
enum Node {
    Split { orientation: H | V, children: Vec<(Node, f32 /*proportion*/)> },
    Leaf  { pane_id: PaneId },
}
```
- Sizes are stored as proportions and converted to cells on save.
- After every operation, normalize: collapse single-child splits, merge adjacent same-orientation splits, renormalize proportions.

### Operations
- `remove(leaf)`
- `insert_beside(target, side: L|R|T|B, leaf)`
- `insert_between(parent, index, leaf)`
- `insert_at_edge(side, leaf)`: wraps the root, or adds a sibling if the root already has that orientation.
- `swap(a, b)` (center drop)

Every drag is `remove` followed by one insert, applied to a copy of the tree for preview.

## 6. UI
- One window with a layout canvas scaled to the tmux window's aspect ratio, plus a bottom bar with **Save** and **Cancel** buttons.
- Mouse only. No keyboard shortcuts for now.
- Each pane is drawn as a rectangle with its pane ID/title and a text snapshot (see §8).
- The active pane is outlined.

### Drag and drop
- Press and drag on a pane to start; the pane renders semi-transparent under the cursor, and its original slot is dimmed.
- While dragging, a highlight shows the rectangle the dragged pane would occupy, computed from the preview tree.
- Release applies the operation to the working tree; release outside any valid zone cancels the drag.

### Drop zones (tested in this order)
| Priority | Zone | Operation |
|---|---|---|
| 1 | Window-edge margin band | `insert_at_edge` |
| 2 | Divider gutter between siblings | `insert_between` |
| 3 | Outer ~25% band of a pane (L/R/T/B) | `insert_beside` |
| 4 | Pane center | `swap` |

### Buttons
- **Save:** apply the layout (§7), then exit 0.
- **Cancel:** exit 1 without touching tmux. Closing the window also counts as cancel.

## 7. Applying the Layout
1. Convert the tree to cell sizes, accounting for 1-cell borders between panes and any `pane-border-status` row.
2. Reorder panes with `swap-pane` so the window's pane order matches the tree's depth-first leaf order (`select-layout` assigns panes to leaves by order, not by ID).
3. Build the layout string: `WxH,x,y{...}` for left/right splits, `WxH,x,y[...]` for top/bottom splits, leaves as `WxH,x,y,<pane_num>`. Prefix with the checksum:
```rust
fn checksum(s: &str) -> u16 {
    let mut c: u16 = 0;
    for b in s.bytes() {
        c = (c >> 1) + ((c & 1) << 15);
        c = c.wrapping_add(b as u16);
    }
    c
}
```
4. Run `tmux select-layout -t <window> '<%04x checksum>,<layout>'`.
5. On any tmux failure, print stderr and exit 3.

If the window's layout changed between launch and save, still apply; tmux rejects an invalid layout string and the app exits 3.

## 8. Pane Snapshots
- Captured once at launch, not live.
- `capture-pane -p -e` output is parsed with `vt100` into a cell grid.
- Drawn with egui's painter in a small monospace font, with per-cell foreground/background colors, clipped to the pane rectangle.
- Re-rendered only when the pane rectangle's size changes.

## 9. Platform Notes
- **macOS:** `wgpu` (Metal) backend.
- **WSL2:** requires WSLg; ensure `DISPLAY`/`WAYLAND_DISPLAY` are present. Add to `tmux.conf`:
  `set -g update-environment "DISPLAY WAYLAND_DISPLAY XDG_RUNTIME_DIR"`
- **Linux:** winit handles X11 and Wayland; request a floating, non-maximized window with a fixed initial size.

## 10. Module Layout
```
src/
  main.rs       // arg parsing, exit codes
  tmux.rs       // all tmux CLI interaction (single module)
  layout.rs     // parse layout string, tree, operations, serialize, checksum
  snapshot.rs   // capture + vt100 -> cell grid
  app.rs        // eframe App, drag state, hit-testing, rendering
```
- `layout.rs` has no GUI or tmux dependencies and is unit tested (parse/serialize round-trip, each operation, normalization, checksum).
- `tmux.rs` is the only code that spawns `tmux`.

## 11. Non-Goals (for now)
- Keyboard controls.
- Live-updating snapshots.
- Resizing panes by dragging dividers.
- Rearranging zoomed windows.
- Multi-window operations.
