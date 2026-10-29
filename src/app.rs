//! eframe App: drag-and-drop pane rearrangement UI.

use crate::layout::{self, compute_layout_rects, leaf_order, LayoutRect, Node, Side};
use crate::snapshot::GrayscaleSnapshot;
use crate::tmux;
use egui::{Color32, Pos2, Rect, Vec2};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Drop zone types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum DropZone {
    /// Window-edge margin band.
    Edge(Side),
    /// Divider gutter between two siblings.
    Between {
        left_pane: layout::PaneId,
        right_pane: layout::PaneId,
    },
    /// Outer band of a pane.
    Beside { pane_id: layout::PaneId, side: Side },
    /// Center of a pane (swap).
    Swap { pane_id: layout::PaneId },
}

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

pub struct TmuxRearrangeApp {
    /// The current working tree.
    tree: Node,
    /// Total dimensions of the tmux window (cells).
    total_w: u16,
    total_h: u16,
    /// Pane metadata keyed by pane_id.
    pane_info: HashMap<layout::PaneId, tmux::PaneInfo>,
    /// Active pane ID.
    active_pane_id: Option<layout::PaneId>,
    /// Pane snapshots (grayscale pixel buffers) keyed by pane_id.
    snapshots: HashMap<layout::PaneId, GrayscaleSnapshot>,
    /// Uploaded egui textures, one per pane.
    texture_handles: HashMap<layout::PaneId, egui::TextureHandle>,
    /// Whether textures have been uploaded (done once on first frame).
    textures_loaded: bool,
    /// The window target for tmux commands.
    window: String,
    /// Pane border status ("" or "top" or "bottom").
    #[allow(dead_code)]
    pane_border_status: String,

    // Drag state
    drag: Option<DragState>,

    // Computed each frame
    pane_rects: Vec<LayoutRect>,
    dividers: Vec<layout::Divider>,
    /// The rect of the layout canvas (where panes are drawn).
    canvas_rect: Rect,
    /// Scale factor: cells → pixels.
    scale_x: f32,
    scale_y: f32,
}

struct DragState {
    /// The pane being dragged.
    pane_id: layout::PaneId,
    /// Original tree before drag started (for cancel).
    original_tree: Node,
    /// Tree with the dragged pane removed (preview).
    preview_tree: Node,
    /// Original pane rect (for dimming the source slot).
    original_rect: Rect,
    /// Current drop zone under cursor.
    drop_zone: Option<DropZone>,
}

impl TmuxRearrangeApp {
    pub fn new(
        window: String,
        tree: Node,
        total_w: u16,
        total_h: u16,
        pane_info: Vec<tmux::PaneInfo>,
        snapshots: HashMap<layout::PaneId, GrayscaleSnapshot>,
        active_pane_id: Option<layout::PaneId>,
        pane_border_status: String,
    ) -> Self {
        let pane_info_map: HashMap<_, _> = pane_info
            .into_iter()
            .map(|p| (p.pane_id.clone(), p))
            .collect();

        TmuxRearrangeApp {
            tree,
            total_w,
            total_h,
            pane_info: pane_info_map,
            active_pane_id,
            snapshots,
            texture_handles: HashMap::new(),
            textures_loaded: false,
            window,
            pane_border_status,
            drag: None,
            pane_rects: Vec::new(),
            dividers: Vec::new(),
            canvas_rect: Rect::NOTHING,
            scale_x: 1.0,
            scale_y: 1.0,
        }
    }
}

// ---------------------------------------------------------------------------
// eframe App impl
// ---------------------------------------------------------------------------

impl eframe::App for TmuxRearrangeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Bottom bar
        egui::TopBottomPanel::bottom("bottom_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    self.save();
                }
                if ui.button("Cancel").clicked() {
                    std::process::exit(1);
                }
            });
        });

        // Lazy texture upload (once, on first frame)
        if !self.textures_loaded {
            self.upload_textures(ctx);
            self.textures_loaded = true;
        }

        // Central canvas
        egui::CentralPanel::default().show(ctx, |ui| {
            self.canvas_rect = ui.available_rect_before_wrap();
            let available = ui.available_size();

            // Compute scale to fit the tmux window aspect ratio
            let aspect = self.total_w as f32 / (self.total_h as f32).max(1.0);
            let available_aspect = available.x / available.y.max(1.0);

            let (canvas_w, canvas_h) = if available_aspect > aspect {
                // Height-constrained
                let h = available.y;
                let w = h * aspect;
                (w, h)
            } else {
                // Width-constrained
                let w = available.x;
                let h = w / aspect;
                (w, h)
            };

            self.scale_x = canvas_w / self.total_w as f32;
            self.scale_y = canvas_h / self.total_h as f32;

            // Center the canvas
            let canvas_left = self.canvas_rect.left() + (available.x - canvas_w) * 0.5;
            let canvas_top = self.canvas_rect.top() + (available.y - canvas_h) * 0.5;
            let canvas_rect = Rect::from_min_size(
                Pos2::new(canvas_left, canvas_top),
                Vec2::new(canvas_w, canvas_h),
            );
            self.canvas_rect = canvas_rect;

            // Compute layout rects first (needed for hit-testing in on_press)
            let display_tree = if let Some(ref drag) = self.drag {
                &drag.preview_tree
            } else {
                &self.tree
            };
            let (pane_rects, dividers) = compute_layout_rects(display_tree, canvas_rect);
            self.pane_rects = pane_rects;
            self.dividers = dividers;

            // Handle input
            let pointer_pos = ui.input(|i| i.pointer.hover_pos());
            let pressed = ui.input(|i| i.pointer.button_pressed(egui::PointerButton::Primary));
            let released = ui.input(|i| i.pointer.button_released(egui::PointerButton::Primary));
            let down = ui.input(|i| i.pointer.button_down(egui::PointerButton::Primary));

            // Process press before release so a click in one frame
            // (both pressed + released) starts and finishes the drag correctly.
            if let Some(pos) = pointer_pos {
                if pressed {
                    self.on_press(pos);
                    // Recompute layout after starting drag so the preview tree
                    // (without dragged pane) is shown immediately.
                    if let Some(ref drag) = self.drag {
                        let (pr, div) = compute_layout_rects(&drag.preview_tree, canvas_rect);
                        self.pane_rects = pr;
                        self.dividers = div;
                    }
                }
            }

            // Always process release to clean up drag state, even if pointer
            // is outside the window (pointer_pos is None).
            if released {
                if let Some(release_pos) = pointer_pos {
                    self.on_release(release_pos);
                } else {
                    // Pointer outside window: cancel the drag.
                    self.cancel_drag();
                }
                // Recompute layout after drop so rendering shows the result
                // immediately instead of one frame later.
                let display_tree = if let Some(ref drag) = self.drag {
                    &drag.preview_tree
                } else {
                    &self.tree
                };
                let (pr, div) = compute_layout_rects(display_tree, canvas_rect);
                self.pane_rects = pr;
                self.dividers = div;
            }

            if let Some(pos) = pointer_pos {
                if !released && down {
                    self.on_drag(pos);
                }
            }

            // Render
            let painter = ui.painter();
            self.render(&painter, pointer_pos);
        });

        ctx.request_repaint(); // ensure smooth drag updates
    }
}

// ---------------------------------------------------------------------------
// Input handling
// ---------------------------------------------------------------------------

impl TmuxRearrangeApp {
    fn on_press(&mut self, pos: Pos2) {
        // Guard against double-press (shouldn't happen, but be safe)
        if self.drag.is_some() {
            return;
        }

        // Find which pane was clicked
        let clicked_pane = self
            .pane_rects
            .iter()
            .find(|lr| lr.rect.contains(pos))
            .map(|lr| lr.pane_id.clone());

        if let Some(pane_id) = clicked_pane {
            // Only allow dragging if there's more than one pane, or always allow
            let original_tree = self.tree.clone();
            let preview_tree = match layout::remove(&self.tree, &pane_id) {
                Some(t) => t,
                None => return, // can't remove the only pane
            };

            // Find original rect of the dragged pane in the original tree
            let (orig_rects, _) = compute_layout_rects(&original_tree, self.canvas_rect);
            let original_rect = orig_rects
                .iter()
                .find(|lr| lr.pane_id == pane_id)
                .map(|lr| lr.rect)
                .unwrap_or(Rect::NOTHING);

            self.drag = Some(DragState {
                pane_id: pane_id.clone(),
                original_tree,
                preview_tree,
                original_rect,
                drop_zone: None,
            });

            // Update tree to preview
            if let Some(ref drag) = self.drag {
                self.tree = drag.preview_tree.clone();
            }
        }
    }

    /// Cancel the current drag, restoring the original tree.
    fn cancel_drag(&mut self) {
        if let Some(drag) = self.drag.take() {
            self.tree = drag.original_tree;
        }
    }

    fn on_drag(&mut self, pos: Pos2) {
        if let Some(ref mut drag) = self.drag {
            // Hit test against the preview tree to find drop zone
            let (preview_rects, preview_dividers) =
                compute_layout_rects(&drag.preview_tree, self.canvas_rect);
            drag.drop_zone = hit_test(
                pos,
                self.canvas_rect,
                &preview_rects,
                &preview_dividers,
                &drag.pane_id,
            );
        }
    }

    fn on_release(&mut self, pos: Pos2) {
        if let Some(drag) = self.drag.take() {
            // Final hit test
            let (preview_rects, preview_dividers) =
                compute_layout_rects(&drag.preview_tree, self.canvas_rect);
            let drop_zone = hit_test(
                pos,
                self.canvas_rect,
                &preview_rects,
                &preview_dividers,
                &drag.pane_id,
            );

            let dragged_leaf = Node::Leaf {
                pane_id: drag.pane_id.clone(),
            };

            let mut applied = false;
            if let Some(zone) = drop_zone {
                // Swap operates on the original tree (both panes present),
                // other operations insert the dragged leaf into the preview tree.
                let new_tree = if matches!(zone, DropZone::Swap { .. }) {
                    apply_drop(&drag.original_tree, &zone, dragged_leaf)
                } else {
                    apply_drop(&drag.preview_tree, &zone, dragged_leaf)
                };

                // Count occurrences of the dragged pane — must be exactly 1.
                let leaves = leaf_order(&new_tree);
                let count = leaves.iter().filter(|id| *id == &drag.pane_id).count();
                if count == 1 {
                    self.tree = new_tree;
                    applied = true;
                }
                // If count != 1 (missing or duplicate), fall back to original tree.
            }

            if !applied {
                // Cancel: restore original tree
                self.tree = drag.original_tree;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Hit testing
// ---------------------------------------------------------------------------

/// Margin band width as a fraction of the window dimension.
const EDGE_MARGIN: f32 = 0.08;
/// Outer band for insert-beside as a fraction of pane dimension.
const BESIDE_BAND: f32 = 0.25;
/// Gutter half-width in pixels for divider hit testing.
const GUTTER_HALF: f32 = 6.0;

fn hit_test(
    pos: Pos2,
    canvas_rect: Rect,
    pane_rects: &[LayoutRect],
    dividers: &[layout::Divider],
    dragged_pane: &str,
) -> Option<DropZone> {
    // 1. Window-edge margin band
    if canvas_rect.contains(pos) {
        let dist_left = pos.x - canvas_rect.left();
        let dist_right = canvas_rect.right() - pos.x;
        let dist_top = pos.y - canvas_rect.top();
        let dist_bottom = canvas_rect.bottom() - pos.y;

        let edge_w = canvas_rect.width() * EDGE_MARGIN;
        let edge_h = canvas_rect.height() * EDGE_MARGIN;

        let near_left = dist_left < edge_w;
        let near_right = dist_right < edge_w;
        let near_top = dist_top < edge_h;
        let near_bottom = dist_bottom < edge_h;

        if near_left || near_right || near_top || near_bottom {
            // Pick the closest edge
            let mut best_side = if near_left {
                Side::Left
            } else if near_right {
                Side::Right
            } else if near_top {
                Side::Top
            } else {
                Side::Bottom
            };
            let mut best_dist = match best_side {
                Side::Left => dist_left,
                Side::Right => dist_right,
                Side::Top => dist_top,
                Side::Bottom => dist_bottom,
            };

            // Check all edges
            for (side, dist) in &[
                (Side::Left, dist_left),
                (Side::Right, dist_right),
                (Side::Top, dist_top),
                (Side::Bottom, dist_bottom),
            ] {
                let edge = match side {
                    Side::Left | Side::Right => edge_w,
                    Side::Top | Side::Bottom => edge_h,
                };
                if *dist < edge && *dist < best_dist {
                    best_side = *side;
                    best_dist = *dist;
                }
            }
            return Some(DropZone::Edge(best_side));
        }
    }

    // 2. Divider gutters
    for div in dividers {
        let gutter = div.rect.expand2(Vec2::new(GUTTER_HALF, GUTTER_HALF));
        if gutter.contains(pos) {
            return Some(DropZone::Between {
                left_pane: div.left_pane.clone(),
                right_pane: div.right_pane.clone(),
            });
        }
    }

    // 3 & 4. Pane zones (beside bands, then center swap)
    for lr in pane_rects {
        if lr.pane_id == dragged_pane {
            continue;
        }
        if !lr.rect.contains(pos) {
            continue;
        }

        let r = lr.rect;
        let band_w = r.width() * BESIDE_BAND;
        let band_h = r.height() * BESIDE_BAND;

        let dist_left = pos.x - r.left();
        let dist_right = r.right() - pos.x;
        let dist_top = pos.y - r.top();
        let dist_bottom = r.bottom() - pos.y;

        // Check if in any outer band
        let in_left = dist_left < band_w;
        let in_right = dist_right < band_w;
        let in_top = dist_top < band_h;
        let in_bottom = dist_bottom < band_h;

        if in_left || in_right || in_top || in_bottom {
            // Pick the closest side
            let candidates = [
                (in_left, Side::Left, dist_left),
                (in_right, Side::Right, dist_right),
                (in_top, Side::Top, dist_top),
                (in_bottom, Side::Bottom, dist_bottom),
            ];
            let best = candidates
                .iter()
                .filter(|(in_band, _, _)| *in_band)
                .min_by(|a, b| a.2.partial_cmp(&b.2).unwrap())
                .map(|(_, side, _)| *side);

            if let Some(side) = best {
                return Some(DropZone::Beside {
                    pane_id: lr.pane_id.clone(),
                    side,
                });
            }
        }

        // 4. Center → swap
        return Some(DropZone::Swap {
            pane_id: lr.pane_id.clone(),
        });
    }

    None
}

// ---------------------------------------------------------------------------
// Apply drop operation
// ---------------------------------------------------------------------------

fn apply_drop(tree: &Node, zone: &DropZone, leaf: Node) -> Node {
    match zone {
        DropZone::Edge(side) => layout::insert_at_edge(tree, *side, leaf),
        DropZone::Between {
            left_pane,
            right_pane,
        } => layout::insert_between(tree, left_pane, right_pane, leaf)
            .unwrap_or_else(|| tree.clone()),
        DropZone::Beside { pane_id, side } => {
            layout::insert_beside(tree, pane_id, *side, leaf).unwrap_or_else(|| tree.clone())
        }
        DropZone::Swap { pane_id } => {
            // Swap: we need the dragged pane_id from the leaf
            if let Node::Leaf {
                pane_id: dragged_id,
            } = &leaf
            {
                layout::swap(tree, dragged_id, pane_id).unwrap_or_else(|| tree.clone())
            } else {
                tree.clone()
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Texture management
// ---------------------------------------------------------------------------

impl TmuxRearrangeApp {
    /// Upload each pane's grayscale buffer as an egui texture (once at launch).
    fn upload_textures(&mut self, ctx: &egui::Context) {
        for (pane_id, snap) in &self.snapshots {
            let pixels: Vec<egui::Color32> = snap
                .pixels
                .iter()
                .map(|&v| egui::Color32::from_gray(v))
                .collect();
            // size is [width, height] = [cols, rows]; pixels.len() must equal cols * rows.
            assert_eq!(pixels.len(), snap.cols as usize * snap.rows as usize);
            let image = egui::ColorImage {
                size: [snap.cols as usize, snap.rows as usize],
                pixels,
            };
            let handle = ctx.load_texture(
                format!("snap-{}", pane_id),
                image,
                egui::TextureOptions {
                    magnification: egui::TextureFilter::Linear,
                    minification: egui::TextureFilter::Linear,
                    ..Default::default()
                },
            );
            self.texture_handles.insert(pane_id.clone(), handle);
        }
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

impl TmuxRearrangeApp {
    fn render(&mut self, painter: &egui::Painter, cursor_pos: Option<Pos2>) {
        let canvas_rect = self.canvas_rect;

        // Draw canvas background
        painter.rect_filled(canvas_rect, 2.0, Color32::from_rgb(30, 30, 30));

        // Draw panes
        for lr in &self.pane_rects {
            let is_active = self
                .active_pane_id
                .as_ref()
                .map(|id| id == &lr.pane_id)
                .unwrap_or(false);

            // During a drag the pane is removed from the preview tree, so
            // it won't appear in pane_rects.  We draw a dimmed placeholder
            // separately (see below).  Normal alpha for all visible panes.
            let alpha = 1.0;

            // Pane background
            let bg = if is_active {
                Color32::from_rgb(50, 50, 60)
            } else {
                Color32::from_rgb(40, 40, 45)
            };
            painter.rect_filled(lr.rect, 2.0, bg.gamma_multiply(alpha));

            // Pane border
            let border_color = if is_active {
                Color32::from_rgb(100, 180, 255)
            } else {
                Color32::from_rgb(80, 80, 80)
            };
            painter.rect_stroke(
                lr.rect,
                2.0,
                egui::Stroke::new(1.5_f32, border_color.gamma_multiply(alpha)),
            );

            // Pane ID label (minimal, overlaid on minimap)
            let pane_id_str = &lr.pane_id;
            let font_id = egui::FontId::monospace(10.0);
            let label_pos = lr.rect.left_top() + Vec2::new(4.0, 12.0);
            painter.text(
                label_pos,
                egui::Align2::LEFT_TOP,
                pane_id_str,
                font_id,
                Color32::from_rgba_premultiplied(255, 255, 255, 180),
            );

            // Draw grayscale minimap texture if available
            if let Some(handle) = self.texture_handles.get(&lr.pane_id) {
                if let Some(snap) = self.snapshots.get(&lr.pane_id) {
                    let margin = 2.0;
                    let label_h = 16.0;
                    let avail = Rect::from_min_max(
                        Pos2::new(lr.rect.left() + margin, lr.rect.top() + label_h),
                        Pos2::new(lr.rect.right() - margin, lr.rect.bottom() - margin),
                    );

                    if avail.width() > 0.0 && avail.height() > 0.0 {
                        // Terminal cells are ~2× taller than wide (cell aspect ≈ 0.5).
                        // Letter-box the texture into `avail` preserving this aspect.
                        let cell_aspect: f32 = 0.5;
                        let tex_w = snap.cols as f32;
                        let tex_h = snap.rows as f32;
                        let tex_display_aspect = (tex_w * cell_aspect) / tex_h;
                        let avail_aspect = avail.width() / avail.height();

                        let draw_rect = if avail_aspect > tex_display_aspect {
                            // Height-constrained: centre horizontally.
                            let h = avail.height();
                            let w = h * tex_display_aspect;
                            Rect::from_center_size(avail.center(), Vec2::new(w, h))
                        } else {
                            // Width-constrained: centre vertically.
                            let w = avail.width();
                            let h = w / tex_display_aspect;
                            Rect::from_center_size(avail.center(), Vec2::new(w, h))
                        };

                        let uv = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
                        let mut mesh = egui::epaint::Mesh::with_texture(handle.id());
                        mesh.add_rect_with_uv(draw_rect, uv, Color32::WHITE);
                        painter.add(egui::Shape::Mesh(mesh));
                    }
                }
            }
        }

        // Draw dividers
        for div in &self.dividers {
            painter.rect_filled(div.rect, 0.0, Color32::from_rgb(60, 60, 60));
        }

        // Draw drop zone highlight
        if let Some(ref drag) = self.drag {
            if let Some(ref zone) = drag.drop_zone {
                let highlight_rect =
                    compute_drop_highlight(zone, &drag.preview_tree, canvas_rect, &drag.pane_id);
                if highlight_rect.is_positive() {
                    painter.rect_filled(
                        highlight_rect,
                        0.0,
                        Color32::from_rgba_premultiplied(100, 200, 255, 80),
                    );
                    painter.rect_stroke(
                        highlight_rect,
                        0.0,
                        egui::Stroke::new(2.0_f32, Color32::from_rgb(100, 200, 255)),
                    );
                }
            }

            // Draw dimmed original position of the dragged pane
            if drag.original_rect.is_positive() {
                painter.rect_filled(
                    drag.original_rect,
                    2.0,
                    Color32::from_rgba_premultiplied(40, 40, 45, 77), // dimmed
                );
                painter.rect_stroke(
                    drag.original_rect,
                    2.0,
                    egui::Stroke::new(1.0_f32, Color32::from_rgba_premultiplied(80, 80, 80, 77)),
                );
            }

            // Draw dragged pane under cursor
            if let Some(pos) = cursor_pos {
                let drag_size = drag.original_rect.size() * 0.6;
                let drag_rect = Rect::from_center_size(pos, drag_size);
                painter.rect_filled(
                    drag_rect,
                    4.0,
                    Color32::from_rgba_premultiplied(80, 80, 120, 180),
                );
                painter.rect_stroke(drag_rect, 4.0, egui::Stroke::new(1.0_f32, Color32::WHITE));

                // Label on dragged preview
                let font_id = egui::FontId::monospace(10.0);
                painter.text(
                    drag_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    &drag.pane_id,
                    font_id,
                    Color32::WHITE,
                );
            }
        }
    }
}

/// Compute the highlight rect for a drop zone.
fn compute_drop_highlight(
    zone: &DropZone,
    preview_tree: &Node,
    canvas_rect: Rect,
    _dragged_pane_id: &str,
) -> Rect {
    match zone {
        DropZone::Edge(side) => {
            let band_w = canvas_rect.width() * EDGE_MARGIN;
            let band_h = canvas_rect.height() * EDGE_MARGIN;
            match side {
                Side::Left => Rect::from_min_max(
                    canvas_rect.left_top(),
                    Pos2::new(canvas_rect.left() + band_w, canvas_rect.bottom()),
                ),
                Side::Right => Rect::from_min_max(
                    Pos2::new(canvas_rect.right() - band_w, canvas_rect.top()),
                    canvas_rect.right_bottom(),
                ),
                Side::Top => Rect::from_min_max(
                    canvas_rect.left_top(),
                    Pos2::new(canvas_rect.right(), canvas_rect.top() + band_h),
                ),
                Side::Bottom => Rect::from_min_max(
                    Pos2::new(canvas_rect.left(), canvas_rect.bottom() - band_h),
                    canvas_rect.right_bottom(),
                ),
            }
        }
        DropZone::Between {
            left_pane,
            right_pane,
        } => {
            let (_preview_rects, preview_dividers) =
                compute_layout_rects(preview_tree, canvas_rect);
            for div in &preview_dividers {
                if (div.left_pane == *left_pane && div.right_pane == *right_pane)
                    || (div.left_pane == *right_pane && div.right_pane == *left_pane)
                {
                    return div.rect.expand2(Vec2::new(GUTTER_HALF, GUTTER_HALF));
                }
            }
            Rect::NOTHING
        }
        DropZone::Beside { pane_id, side } => {
            let (preview_rects, _) = compute_layout_rects(preview_tree, canvas_rect);
            if let Some(lr) = preview_rects.iter().find(|lr| &lr.pane_id == pane_id) {
                let r = lr.rect;
                let band_w = r.width() * BESIDE_BAND;
                let band_h = r.height() * BESIDE_BAND;
                match side {
                    Side::Left => {
                        Rect::from_min_max(r.left_top(), Pos2::new(r.left() + band_w, r.bottom()))
                    }
                    Side::Right => {
                        Rect::from_min_max(Pos2::new(r.right() - band_w, r.top()), r.right_bottom())
                    }
                    Side::Top => {
                        Rect::from_min_max(r.left_top(), Pos2::new(r.right(), r.top() + band_h))
                    }
                    Side::Bottom => Rect::from_min_max(
                        Pos2::new(r.left(), r.bottom() - band_h),
                        r.right_bottom(),
                    ),
                }
            } else {
                Rect::NOTHING
            }
        }
        DropZone::Swap { pane_id } => {
            let (preview_rects, _) = compute_layout_rects(preview_tree, canvas_rect);
            preview_rects
                .iter()
                .find(|lr| &lr.pane_id == pane_id)
                .map(|lr| lr.rect)
                .unwrap_or(Rect::NOTHING)
        }
    }
}

// ---------------------------------------------------------------------------
// Save
// ---------------------------------------------------------------------------

impl TmuxRearrangeApp {
    fn save(&mut self) {
        let pane_order = leaf_order(&self.tree);
        let layout_str =
            layout::format_layout_for_tmux(&self.tree, self.total_w, self.total_h, &pane_order);

        // Reorder panes to match leaf order
        if let Err(e) = tmux::reorder_panes(&self.window, &pane_order) {
            eprintln!("Error reordering panes: {}", e);
            std::process::exit(3);
        }

        // Apply layout
        if let Err(e) = tmux::apply_layout(&self.window, &layout_str) {
            eprintln!("Error applying layout: {}", e);
            std::process::exit(3);
        }

        std::process::exit(0);
    }
}
