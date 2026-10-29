//! tmux-rearrange: GUI tool to rearrange tmux panes by drag-and-drop.
//!
//! Usage: tmux-rearrange --window <window_id>

mod app;
mod layout;
mod snapshot;
mod tmux;

use std::collections::HashMap;
use std::env;
use std::process;

fn main() {
    // Parse arguments
    let args: Vec<String> = env::args().collect();
    let mut window: Option<String> = None;
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--window" && i + 1 < args.len() {
            window = Some(args[i + 1].clone());
            i += 2;
        } else if args[i] == "--help" || args[i] == "-h" {
            print_usage();
            process::exit(0);
        } else {
            eprintln!("Unknown argument: {}", args[i]);
            print_usage();
            process::exit(3);
        }
    }

    let window = match window {
        Some(w) => w,
        None => {
            eprintln!("Error: --window is required");
            print_usage();
            process::exit(3);
        }
    };

    // Check for display (Linux only; macOS uses Metal, Windows uses native)
    #[cfg(target_os = "linux")]
    {
        if env::var("DISPLAY").is_err() && env::var("WAYLAND_DISPLAY").is_err() {
            eprintln!("Error: no display available (set DISPLAY or WAYLAND_DISPLAY)");
            process::exit(3);
        }
    }

    // Run main logic
    match run(&window) {
        Ok(code) => process::exit(code),
        Err(e) => {
            eprintln!("Error: {}", e);
            process::exit(3);
        }
    }
}

fn run(window: &str) -> Result<i32, Box<dyn std::error::Error>> {
    // 1. Check if zoomed
    if tmux::is_window_zoomed(window)? {
        eprintln!("Window is zoomed; cannot rearrange");
        return Ok(2);
    }

    // 2. Get visible layout
    let layout_str = tmux::get_visible_layout(window)?;

    // 3. Extract total dimensions
    let (total_w, total_h) = layout::extract_total_dimensions(&layout_str)
        .map_err(|e| format!("failed to parse layout dimensions: {}", e))?;

    // 4. List panes
    let pane_info = tmux::list_panes(window)?;
    let pane_ids: Vec<String> = pane_info.iter().map(|p| p.pane_id.clone()).collect();

    // 5. Parse layout into tree
    let tree = layout::parse_layout(&layout_str, &pane_ids)
        .map_err(|e| format!("failed to parse layout: {}", e))?;

    // 6. Capture pane snapshots (sequentially for simplicity; could be parallel)
    let mut snapshots: HashMap<String, snapshot::CellGrid> = HashMap::new();
    for info in &pane_info {
        match tmux::capture_pane(&info.pane_id) {
            Ok(data) => {
                let grid = snapshot::parse_snapshot(&data, info.height, info.width);
                snapshots.insert(info.pane_id.clone(), grid);
            }
            Err(e) => {
                eprintln!("Warning: failed to capture pane {}: {}", info.pane_id, e);
            }
        }
    }

    // 7. Get active pane
    let active_pane_id = tmux::get_active_pane(window).ok();

    // 8. Get pane-border-status
    let pane_border_status = tmux::get_pane_border_status(window).unwrap_or_default();

    // 9. Launch GUI
    let app = app::TmuxRearrangeApp::new(
        window.to_string(),
        tree,
        total_w,
        total_h,
        pane_info,
        snapshots,
        active_pane_id,
        pane_border_status,
    );

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([800.0, 600.0])
            .with_title("tmux-rearrange"),
        ..Default::default()
    };

    eframe::run_native(
        "tmux-rearrange",
        native_options,
        Box::new(|_cc| Box::new(app)),
    )
    .map_err(|e| format!("failed to run GUI: {}", e))?;

    // Window closed without Save → cancel
    Ok(1)
}

fn print_usage() {
    eprintln!("Usage: tmux-rearrange --window <window_id>");
    eprintln!();
    eprintln!("Example tmux binding:");
    eprintln!("  bind g run-shell -b \"tmux-rearrange --window '#{{window_id}}'\"");
}
