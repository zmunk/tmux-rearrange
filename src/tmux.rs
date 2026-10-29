//! All tmux CLI interaction.  This is the only module that spawns `tmux`.

use std::process::{Command, Output};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum TmuxError {
    CommandFailed { cmd: String, stderr: String },
    TmuxNotFound,
    ParseError(String),
}

impl std::fmt::Display for TmuxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TmuxError::CommandFailed { cmd, stderr } => {
                write!(f, "tmux command failed: {}\nstderr: {}", cmd, stderr)
            }
            TmuxError::TmuxNotFound => write!(f, "tmux not found in PATH"),
            TmuxError::ParseError(s) => write!(f, "parse error: {}", s),
        }
    }
}

impl std::error::Error for TmuxError {}

pub type Result<T> = std::result::Result<T, TmuxError>;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn run_tmux(args: &[&str]) -> Result<Output> {
    Command::new("tmux").args(args).output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            TmuxError::TmuxNotFound
        } else {
            TmuxError::CommandFailed {
                cmd: format!("tmux {}", args.join(" ")),
                stderr: e.to_string(),
            }
        }
    })
}

fn run_tmux_stdout(args: &[&str]) -> Result<String> {
    let output = run_tmux(args)?;
    if !output.status.success() {
        return Err(TmuxError::CommandFailed {
            cmd: format!("tmux {}", args.join(" ")),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Check whether the given window is zoomed.  Returns `true` if zoomed.
pub fn is_window_zoomed(window: &str) -> Result<bool> {
    let out = run_tmux_stdout(&[
        "display-message",
        "-p",
        "-t",
        window,
        "#{window_zoomed_flag}",
    ])?;
    Ok(out.trim() == "1")
}

/// Get the visible layout string for a window.
pub fn get_visible_layout(window: &str) -> Result<String> {
    run_tmux_stdout(&[
        "display-message",
        "-p",
        "-t",
        window,
        "#{window_visible_layout}",
    ])
}

/// List panes in a window.  Returns Vec of (pane_id, width, height, is_active).
pub fn list_panes(window: &str) -> Result<Vec<PaneInfo>> {
    let out = run_tmux_stdout(&[
        "list-panes",
        "-t",
        window,
        "-F",
        "#{pane_id} #{pane_width} #{pane_height} #{?pane_active,1,0}",
    ])?;

    let mut panes = Vec::new();
    for line in out.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.splitn(4, ' ').collect();
        if parts.len() < 4 {
            continue;
        }
        panes.push(PaneInfo {
            pane_id: parts[0].to_string(),
            width: parts[1]
                .parse()
                .map_err(|_| TmuxError::ParseError(format!("bad width: {}", parts[1])))?,
            height: parts[2]
                .parse()
                .map_err(|_| TmuxError::ParseError(format!("bad height: {}", parts[2])))?,
            active: parts[3] == "1",
        });
    }
    Ok(panes)
}

/// Capture the contents of a pane (with escape sequences).
pub fn capture_pane(pane_id: &str) -> Result<Vec<u8>> {
    let output = run_tmux(&["capture-pane", "-p", "-e", "-t", pane_id])?;
    if !output.status.success() {
        return Err(TmuxError::CommandFailed {
            cmd: format!("tmux capture-pane -p -e -t {}", pane_id),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        });
    }
    Ok(output.stdout)
}

/// Get the pane-border-status option for a window.
pub fn get_pane_border_status(window: &str) -> Result<String> {
    run_tmux_stdout(&[
        "display-message",
        "-p",
        "-t",
        window,
        "#{pane-border-status}",
    ])
}

/// Get the active pane ID for a window.
pub fn get_active_pane(window: &str) -> Result<String> {
    run_tmux_stdout(&["display-message", "-p", "-t", window, "#{pane_id}"])
}

/// Reorder panes using swap-pane so that the window's pane order matches the
/// given depth-first leaf order.
pub fn reorder_panes(window: &str, desired_order: &[String]) -> Result<()> {
    // Get current pane order in the window
    let current_order = list_panes(window)?
        .into_iter()
        .map(|p| p.pane_id)
        .collect::<Vec<_>>();

    // We need to make the window's pane order match desired_order.
    // We do this by swapping panes.  Strategy: for each position,
    // if the pane at that position is not the desired one, swap it with
    // the pane that should be there.
    let mut current = current_order.clone();
    for i in 0..desired_order.len().min(current.len()) {
        if current[i] != desired_order[i] {
            // Find where the desired pane currently is
            if let Some(j) = current.iter().position(|id| id == &desired_order[i]) {
                // Swap pane at position i with position j
                // tmux swap-pane -s src -t dst -d (don't change focus)
                let src = &current[j];
                let dst = &current[i];
                run_tmux_stdout(&["swap-pane", "-s", src, "-t", dst, "-d"])?;
                current.swap(i, j);
            }
        }
    }
    Ok(())
}

/// Apply a layout string to a window.
pub fn apply_layout(window: &str, layout_str: &str) -> Result<()> {
    run_tmux_stdout(&["select-layout", "-t", window, layout_str])?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct PaneInfo {
    pub pane_id: String,
    pub width: u16,
    pub height: u16,
    #[allow(dead_code)]
    pub active: bool,
}
