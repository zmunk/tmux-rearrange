# tmux-rearrange

A GUI tool to rearrange tmux panes by mouse drag-and-drop. Launched from a tmux keybinding, it shows the pane layout of the current window and lets you reorganize panes visually.

## Demo

```
┌──────────────────────────────────────────────────┐
│  ┌──────────────┐  ┌──────────┐  ┌───────────┐  │
│  │              │  │          │  │           │  │
│  │   %0 (vim)   │  │  %1 (htop)│  │ %2 (logs) │  │
│  │              │  │          │  │           │  │
│  └──────────────┘  └──────────┘  └───────────┘  │
│                                                   │
│  ┌─────────────────────────────────────────────┐  │
│  │  %3 (shell)                                  │  │
│  │                                              │  │
│  └─────────────────────────────────────────────┘  │
│                                                   │
│              [ Save ]    [ Cancel ]               │
└──────────────────────────────────────────────────┘
```

Drag any pane onto another to swap, onto a divider to insert between, or to a window edge to move it there.

## Requirements

- **tmux** latest (3.9+ with JSON layout support)
- **Rust** stable (1.75+)
- **Display**: macOS (Metal), Linux (X11/Wayland), or WSL2 with WSLg

## Install

```bash
cargo build --release
cp target/release/tmux-rearrange /usr/local/bin/
```

## tmux Configuration

Add a keybinding to `~/.tmux.conf`:

```
bind g run-shell -b "tmux-rearrange --window '#{window_id}'"
```

Then reload tmux: `tmux source-file ~/.tmux.conf`

Press `Ctrl+b g` in any window to launch the rearranger.

## Usage

| Action | How |
|--------|-----|
| **Swap** | Drag a pane onto the **center** of another pane |
| **Insert beside** | Drag a pane onto the **outer 25%** of another pane |
| **Insert between** | Drag a pane onto the **divider** between two panes |
| **Move to edge** | Drag a pane to the **window edge margin** |
| **Save** | Click **Save** — applies the new layout to tmux |
| **Cancel** | Click **Cancel** or close the window — no changes |

### Exit codes

| Code | Meaning |
|------|---------|
| 0 | Layout saved and applied |
| 1 | Cancelled (no changes) |
| 2 | Window is zoomed (refused) |
| 3 | Environment error (no display, tmux not found, etc.) |

## How It Works

### Data flow

```
tmux display-message -p '#{window_visible_layout}'
        │
        ▼
   JSON layout tree          tmux list-panes         tmux capture-pane -e
        │                         │                        │
        ▼                         ▼                        ▼
   layout.rs                  pane metadata           vt100 snapshot
   parse → Node tree          (ids, sizes)            → CellGrid
        │                         │                        │
        └─────────┬───────────────┘                        │
                  ▼                                        │
            app.rs (eframe/egui)                           │
            drag-and-drop UI ◄─────────────────────────────┘
                  │
                  ▼  [Save clicked]
            layout.rs serialize → checksum+layout string
                  │
                  ▼
            tmux swap-pane (reorder) + select-layout
```

### Architecture

```
src/
  main.rs       CLI parsing, orchestration, exit codes
  tmux.rs       All tmux CLI interaction (single module)
  layout.rs     Tree model, JSON/legacy parse, serialize,
                checksum, normalize, insert/remove/swap ops
  snapshot.rs   vt100 → cell grid, ANSI→RGB color, render cache
  app.rs        eframe App, drag state, hit-testing, rendering
```

- **`layout.rs`** has no GUI or tmux dependencies. It works with a pure `Node` tree where splits store children with proportional sizes.
- **`tmux.rs`** is the only module that spawns `tmux` commands.
- **`app.rs`** handles all egui rendering and mouse input.

### Tree model

```rust
enum Node {
    Split {
        orientation: Horizontal | Vertical,
        children: Vec<(Node, f32)>,  // (child, proportion)
    },
    Leaf {
        pane_id: String,  // e.g. "%42"
    },
}
```

Proportions always sum to 1.0 across siblings. The tree is normalized after every operation (single-child splits collapsed, adjacent same-orientation splits merged).

### Drag and drop

Every drag is a `remove` followed by one insert, applied to a copy of the tree for preview:

1. **Press** — clone tree, remove dragged pane → preview tree
2. **Drag** — hit-test cursor against preview tree to find drop zone
3. **Release** — apply insert operation to preview tree → new working tree

Drop zones are tested in priority order:

| Priority | Zone | Operation |
|----------|------|-----------|
| 1 | Window-edge margin (~8%) | `insert_at_edge` |
| 2 | Divider gutter between siblings | `insert_between` |
| 3 | Outer ~25% band of a pane | `insert_beside` |
| 4 | Pane center | `swap` |

### Applying to tmux

On Save:
1. Collect depth-first leaf order from the tree
2. Reorder tmux panes with `swap-pane` so the window's pane order matches
3. Serialize the tree to a layout string with checksum
4. Run `tmux select-layout` to apply the new geometry

## Platform Notes

| Platform | Backend | Notes |
|----------|---------|-------|
| **macOS** | wgpu (Metal) | DISPLAY not required |
| **Linux** | wgpu/glow | Requires `DISPLAY` or `WAYLAND_DISPLAY` |
| **WSL2** | wgpu/glow | Requires WSLg; ensure `update-environment` includes `DISPLAY WAYLAND_DISPLAY XDG_RUNTIME_DIR` |

## Non-Goals (for now)

- Keyboard controls
- Live-updating pane snapshots
- Resizing panes by dragging dividers
- Rearranging zoomed windows
- Multi-window operations
