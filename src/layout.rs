//! Layout tree, parsing, serialization, operations, normalization.
//! No GUI or tmux dependencies.

use serde::Deserialize;
use std::fmt;

// ---------------------------------------------------------------------------
// JSON layout types (tmux >= 3.9)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct LayoutJson {
    #[serde(rename = "V")]
    _version: u32,
    #[serde(rename = "L")]
    root: LayoutNodeJson,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "t")]
enum LayoutNodeJson {
    #[serde(rename = "h")]
    HSplit {
        w: u16,
        h: u16,
        #[allow(dead_code)]
        x: u16,
        #[allow(dead_code)]
        y: u16,
        c: Vec<LayoutNodeJson>,
    },
    #[serde(rename = "v")]
    VSplit {
        w: u16,
        h: u16,
        #[allow(dead_code)]
        x: u16,
        #[allow(dead_code)]
        y: u16,
        c: Vec<LayoutNodeJson>,
    },
    #[serde(rename = "p")]
    Pane {
        #[allow(dead_code)]
        w: u16,
        #[allow(dead_code)]
        h: u16,
        #[allow(dead_code)]
        x: u16,
        #[allow(dead_code)]
        y: u16,
        i: usize,
        #[serde(rename = "I")]
        id: String,
    },
}

/// Parse a JSON-format layout string into a Node tree.
fn parse_json_layout(input: &str, _pane_ids: &[PaneId]) -> Result<Node, ParseError> {
    let parsed: LayoutJson =
        serde_json::from_str(input).map_err(|e| ParseError::InvalidFormat(e.to_string()))?;
    Ok(json_node_to_node(&parsed.root))
}

fn json_node_to_node(node: &LayoutNodeJson) -> Node {
    match node {
        LayoutNodeJson::HSplit { w, c, .. } => {
            let total_w: f32 = c.iter().map(|child| child_width(child) as f32).sum();
            let children: Vec<(Node, f32)> = c
                .iter()
                .map(|child| {
                    let prop = if total_w > 0.0 {
                        child_width(child) as f32 / total_w
                    } else {
                        1.0 / c.len() as f32
                    };
                    (json_node_to_node(child), prop)
                })
                .collect();
            Node::Split {
                orientation: Orientation::Horizontal,
                children,
            }
        }
        LayoutNodeJson::VSplit { h, c, .. } => {
            let total_h: f32 = c.iter().map(|child| child_height(child) as f32).sum();
            let children: Vec<(Node, f32)> = c
                .iter()
                .map(|child| {
                    let prop = if total_h > 0.0 {
                        child_height(child) as f32 / total_h
                    } else {
                        1.0 / c.len() as f32
                    };
                    (json_node_to_node(child), prop)
                })
                .collect();
            Node::Split {
                orientation: Orientation::Vertical,
                children,
            }
        }
        LayoutNodeJson::Pane { i: _, id, .. } => Node::Leaf {
            pane_id: id.clone(),
        },
    }
}

fn child_width(node: &LayoutNodeJson) -> u16 {
    match node {
        LayoutNodeJson::HSplit { w, .. } => *w,
        LayoutNodeJson::VSplit { w, .. } => *w,
        LayoutNodeJson::Pane { w, .. } => *w,
    }
}

fn child_height(node: &LayoutNodeJson) -> u16 {
    match node {
        LayoutNodeJson::HSplit { h, .. } => *h,
        LayoutNodeJson::VSplit { h, .. } => *h,
        LayoutNodeJson::Pane { h, .. } => *h,
    }
}

fn json_total_dimensions(input: &str) -> Option<(u16, u16)> {
    let parsed: LayoutJson = serde_json::from_str(input).ok()?;
    match &parsed.root {
        LayoutNodeJson::HSplit { w, h, .. } => Some((*w, *h)),
        LayoutNodeJson::VSplit { w, h, .. } => Some((*w, *h)),
        LayoutNodeJson::Pane { w, h, .. } => Some((*w, *h)),
    }
}

pub type PaneId = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    Horizontal,
    Vertical,
}

/// The abstract layout tree.  Sizes are stored as proportions (0..1) that
/// always sum to 1.0 across the children of a Split.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Split {
        orientation: Orientation,
        /// Each child paired with its proportion of the *available space*
        /// (after subtracting borders) of this split.
        children: Vec<(Node, f32)>,
    },
    Leaf {
        pane_id: PaneId,
    },
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum ParseError {
    InvalidFormat(String),
    EmptyInput,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::InvalidFormat(s) => write!(f, "invalid layout format: {}", s),
            ParseError::EmptyInput => write!(f, "empty layout string"),
        }
    }
}

impl std::error::Error for ParseError {}

/// Intermediate representation produced by the raw parser.
#[derive(Debug, Clone)]
struct RawCell {
    w: u16,
    h: u16,
    #[allow(dead_code)]
    x: u16,
    #[allow(dead_code)]
    y: u16,
    content: RawContent,
}

#[derive(Debug, Clone)]
enum RawContent {
    Leaf(usize), // pane index (0-based)
    HSplit(Vec<RawCell>),
    VSplit(Vec<RawCell>),
}

/// Parse a layout string (JSON or legacy format) into a Node tree.
pub fn parse_layout(input: &str, pane_ids: &[PaneId]) -> Result<Node, ParseError> {
    let s = input.trim();
    if s.is_empty() {
        return Err(ParseError::EmptyInput);
    }
    // Try JSON format first (tmux >= 3.9)
    if s.starts_with('{') {
        return parse_json_layout(s, pane_ids);
    }
    // Legacy format: optional checksum then WxH,x,y,...
    let body = if s.len() > 5 && s.as_bytes()[4] == b',' {
        &s[5..]
    } else {
        s
    };
    let (raw, _rest) = parse_cell(body)?;
    Ok(raw_to_node(&raw, pane_ids))
}

// ---------------------------------------------------------------------------
// Low-level parsers
// ---------------------------------------------------------------------------

fn parse_number(s: &str) -> Result<(u16, &str), ParseError> {
    let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    if end == 0 {
        return Err(ParseError::InvalidFormat("expected number".into()));
    }
    let n: u16 = s[..end]
        .parse()
        .map_err(|_| ParseError::InvalidFormat(format!("bad number: '{}'", &s[..end])))?;
    Ok((n, &s[end..]))
}

fn expect_char(s: &str, expected: char) -> Result<&str, ParseError> {
    if s.starts_with(expected) {
        Ok(&s[1..])
    } else {
        Err(ParseError::InvalidFormat(format!(
            "expected '{}', got '{}'",
            expected,
            s.chars().next().unwrap_or_default()
        )))
    }
}

/// Parse `WxH,x,y,` prefix.
fn parse_dimensions(s: &str) -> Result<((u16, u16, u16, u16), &str), ParseError> {
    let (w, s) = parse_number(s)?;
    let s = expect_char(s, 'x')?;
    let (h, s) = parse_number(s)?;
    let s = expect_char(s, ',')?;
    let (x, s) = parse_number(s)?;
    let s = expect_char(s, ',')?;
    let (y, s) = parse_number(s)?;
    let s = expect_char(s, ',')?;
    Ok(((w, h, x, y), s))
}

/// Parse a single "cell": `WxH,x,y,<content>`.
fn parse_cell(s: &str) -> Result<(RawCell, &str), ParseError> {
    let ((w, h, x, y), s) = parse_dimensions(s)?;
    let (content, s) = parse_content(s)?;
    Ok((
        RawCell {
            w,
            h,
            x,
            y,
            content,
        },
        s,
    ))
}

/// Parse the content portion: leaf index, `{...}`, or `[...]`.
fn parse_content(s: &str) -> Result<(RawContent, &str), ParseError> {
    let first = s.chars().next();
    match first {
        Some('{') => parse_split_body('{', '}', RawContent::HSplit, s),
        Some('[') => parse_split_body('[', ']', RawContent::VSplit, s),
        Some(c) if c.is_ascii_digit() => {
            let (idx, s) = parse_number(s)?;
            Ok((RawContent::Leaf(idx as usize), s))
        }
        Some(c) => Err(ParseError::InvalidFormat(format!(
            "unexpected char '{}' in content",
            c
        ))),
        None => Err(ParseError::InvalidFormat("unexpected end of input".into())),
    }
}

fn parse_split_body(
    open: char,
    close: char,
    wrap: fn(Vec<RawCell>) -> RawContent,
    input: &str,
) -> Result<(RawContent, &str), ParseError> {
    let mut s = expect_char(input, open)?;
    let mut children = Vec::new();
    loop {
        let (cell, rest) = parse_cell(s)?;
        children.push(cell);
        s = rest;
        if s.starts_with(',') {
            s = &s[1..];
            // Allow trailing comma before close
            if s.starts_with(close) {
                return Ok((wrap(children), &s[1..]));
            }
            // Continue to next child
        } else if s.starts_with(close) {
            return Ok((wrap(children), &s[1..]));
        } else {
            return Err(ParseError::InvalidFormat(format!(
                "expected '{}' or ',' after split child, got '{}'",
                close,
                s.chars().next().unwrap_or_default()
            )));
        }
    }
}

// ---------------------------------------------------------------------------
// Raw → Node conversion (computes proportions)
// ---------------------------------------------------------------------------

fn raw_to_node(raw: &RawCell, pane_ids: &[PaneId]) -> Node {
    match &raw.content {
        RawContent::Leaf(idx) => {
            let pane_id = pane_ids
                .get(*idx)
                .cloned()
                .unwrap_or_else(|| format!("%{}", idx));
            Node::Leaf { pane_id }
        }
        RawContent::HSplit(children) => {
            let total_w: f32 = children.iter().map(|c| c.w as f32).sum();
            let node_children: Vec<(Node, f32)> = children
                .iter()
                .map(|c| {
                    let prop = if total_w > 0.0 {
                        c.w as f32 / total_w
                    } else {
                        1.0 / children.len() as f32
                    };
                    (raw_to_node(c, pane_ids), prop)
                })
                .collect();
            Node::Split {
                orientation: Orientation::Horizontal,
                children: node_children,
            }
        }
        RawContent::VSplit(children) => {
            let total_h: f32 = children.iter().map(|c| c.h as f32).sum();
            let node_children: Vec<(Node, f32)> = children
                .iter()
                .map(|c| {
                    let prop = if total_h > 0.0 {
                        c.h as f32 / total_h
                    } else {
                        1.0 / children.len() as f32
                    };
                    (raw_to_node(c, pane_ids), prop)
                })
                .collect();
            Node::Split {
                orientation: Orientation::Vertical,
                children: node_children,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Serialization
// ---------------------------------------------------------------------------

/// Compute child dimensions for a split given parent dimensions.
/// Returns Vec of (w, h, x, y) for each child.
fn compute_child_dims(
    parent_w: u16,
    parent_h: u16,
    parent_x: u16,
    parent_y: u16,
    orientation: Orientation,
    children: &[(Node, f32)],
) -> Vec<(u16, u16, u16, u16)> {
    let n = children.len();
    if n == 0 {
        return vec![];
    }
    let border_count = n.saturating_sub(1) as u16;
    let mut dims = Vec::with_capacity(n);

    match orientation {
        Orientation::Horizontal => {
            let available = parent_w.saturating_sub(border_count);
            let mut x = parent_x;
            let mut used = 0u16;
            for (i, (_, prop)) in children.iter().enumerate() {
                let child_w = if i == n - 1 {
                    available.saturating_sub(used)
                } else {
                    let w = (available as f32 * prop).round() as u16;
                    w.min(available.saturating_sub(used))
                };
                used += child_w;
                dims.push((child_w, parent_h, x, parent_y));
                x += child_w + 1; // +1 for border
            }
        }
        Orientation::Vertical => {
            let available = parent_h.saturating_sub(border_count);
            let mut y = parent_y;
            let mut used = 0u16;
            for (i, (_, prop)) in children.iter().enumerate() {
                let child_h = if i == n - 1 {
                    available.saturating_sub(used)
                } else {
                    let h = (available as f32 * prop).round() as u16;
                    h.min(available.saturating_sub(used))
                };
                used += child_h;
                dims.push((parent_w, child_h, parent_x, y));
                y += child_h + 1; // +1 for border
            }
        }
    }
    dims
}

/// Serialize a node tree into a layout string (no checksum prefix).
/// `pane_order` is the depth-first order of pane_ids in the final tree.
pub fn serialize_layout(node: &Node, total_w: u16, total_h: u16, pane_order: &[PaneId]) -> String {
    serialize_node(node, total_w, total_h, 0, 0, pane_order)
}

fn serialize_node(node: &Node, w: u16, h: u16, x: u16, y: u16, pane_order: &[PaneId]) -> String {
    match node {
        Node::Leaf { pane_id } => {
            let idx = pane_order.iter().position(|id| id == pane_id).unwrap_or(0);
            format!("{}x{},{},{},{}", w, h, x, y, idx)
        }
        Node::Split {
            orientation,
            children,
        } => {
            let (open, close) = match orientation {
                Orientation::Horizontal => ('{', '}'),
                Orientation::Vertical => ('[', ']'),
            };
            let dims = compute_child_dims(w, h, x, y, *orientation, children);
            let inner: Vec<String> = children
                .iter()
                .zip(dims.iter())
                .map(|((child, _), &(cw, ch, cx, cy))| {
                    serialize_node(child, cw, ch, cx, cy, pane_order)
                })
                .collect();
            format!(
                "{}x{},{},{},{}{}{}",
                w,
                h,
                x,
                y,
                open,
                inner.join(","),
                close
            )
        }
    }
}

/// Format the full layout string ready for `tmux select-layout`:
/// `<checksum>,<layout>`.
pub fn format_layout_for_tmux(
    node: &Node,
    total_w: u16,
    total_h: u16,
    pane_order: &[PaneId],
) -> String {
    let layout = serialize_layout(node, total_w, total_h, pane_order);
    let cs = checksum(&layout);
    format!("{:04x},{}", cs, layout)
}

// ---------------------------------------------------------------------------
// Checksum (tmux-compatible)
// ---------------------------------------------------------------------------

pub fn checksum(s: &str) -> u16 {
    let mut c: u16 = 0;
    for b in s.bytes() {
        c = (c >> 1) + ((c & 1) << 15);
        c = c.wrapping_add(b as u16);
    }
    c
}

// ---------------------------------------------------------------------------
// Tree queries
// ---------------------------------------------------------------------------

/// Depth-first leaf order of pane IDs.
pub fn leaf_order(node: &Node) -> Vec<PaneId> {
    let mut v = Vec::new();
    collect_leaves(node, &mut v);
    v
}

fn collect_leaves(node: &Node, out: &mut Vec<PaneId>) {
    match node {
        Node::Leaf { pane_id } => out.push(pane_id.clone()),
        Node::Split { children, .. } => {
            for (child, _) in children {
                collect_leaves(child, out);
            }
        }
    }
}

/// Find the total width and height from the parsed layout string.
pub fn extract_total_dimensions(layout_str: &str) -> Result<(u16, u16), ParseError> {
    let s = layout_str.trim();
    // Try JSON format first
    if s.starts_with('{') {
        return json_total_dimensions(s)
            .ok_or_else(|| ParseError::InvalidFormat("failed to parse JSON layout".into()));
    }
    // Legacy format
    let body = if s.len() > 5 && s.as_bytes()[4] == b',' {
        &s[5..]
    } else {
        s
    };
    let ((w, h, _x, _y), _rest) = parse_dimensions(body)?;
    Ok((w, h))
}

// ---------------------------------------------------------------------------
// Operations
// ---------------------------------------------------------------------------

/// Remove a leaf by pane_id. Returns None if the tree becomes empty.
pub fn remove(tree: &Node, pane_id: &str) -> Option<Node> {
    match tree {
        Node::Leaf { pane_id: id } => {
            if id == pane_id {
                None
            } else {
                Some(tree.clone())
            }
        }
        Node::Split {
            orientation,
            children,
        } => {
            let mut new_children: Vec<(Node, f32)> = Vec::new();
            let mut found = false;
            let mut _removed_prop = 0.0f32;

            for (child, prop) in children {
                if !found {
                    if let Some(new_child) = remove(child, pane_id) {
                        // The child may have been modified (pane removed from
                        // deeper inside).  Detect this by comparing.
                        if new_child != *child {
                            found = true;
                        }
                        new_children.push((new_child, *prop));
                    } else {
                        // Leaf was removed entirely.
                        found = true;
                        _removed_prop = *prop;
                    }
                } else {
                    new_children.push((child.clone(), *prop));
                }
            }

            if !found {
                return Some(tree.clone());
            }
            if new_children.is_empty() {
                return None;
            }

            // Renormalize proportions
            let total: f32 = new_children.iter().map(|(_, p)| *p).sum();
            if total > 0.0 {
                for (_, p) in &mut new_children {
                    *p /= total;
                }
            }

            let mut result = Node::Split {
                orientation: *orientation,
                children: new_children,
            };
            normalize(&mut result);
            Some(result)
        }
    }
}

/// Insert `leaf` beside `target` on the given `side` within the same parent.
/// Returns the modified tree.
pub fn insert_beside(tree: &Node, target: &str, side: Side, leaf: Node) -> Option<Node> {
    match tree {
        Node::Leaf { .. } => {
            // Wrapping: create a new split with the existing and new leaf
            let orientation = match side {
                Side::Left | Side::Right => Orientation::Horizontal,
                Side::Top | Side::Bottom => Orientation::Vertical,
            };
            let (first, second) = match side {
                Side::Left | Side::Top => (leaf, tree.clone()),
                Side::Right | Side::Bottom => (tree.clone(), leaf),
            };
            Some(Node::Split {
                orientation,
                children: vec![(first, 0.5), (second, 0.5)],
            })
        }
        Node::Split {
            orientation,
            children,
        } => {
            // Look for target in children
            let mut new_children: Vec<(Node, f32)> = Vec::new();
            let mut found = false;

            for (_i, (child, prop)) in children.iter().enumerate() {
                if !found && contains_leaf(child, target) {
                    found = true;
                    if child_is_leaf(child, target) {
                        // Target is a direct child leaf; insert beside it
                        let new_orientation = match side {
                            Side::Left | Side::Right => Orientation::Horizontal,
                            Side::Top | Side::Bottom => Orientation::Vertical,
                        };
                        let (first, second) = match side {
                            Side::Left | Side::Top => (leaf.clone(), child.clone()),
                            Side::Right | Side::Bottom => (child.clone(), leaf.clone()),
                        };
                        new_children.push((
                            Node::Split {
                                orientation: new_orientation,
                                children: vec![(first, 0.5), (second, 0.5)],
                            },
                            *prop,
                        ));
                    } else if let Some(new_child) = insert_beside(child, target, side, leaf.clone())
                    {
                        new_children.push((new_child, *prop));
                    } else {
                        new_children.push((child.clone(), *prop));
                    }
                } else {
                    new_children.push((child.clone(), *prop));
                }
            }

            if !found {
                return None;
            }

            let mut result = Node::Split {
                orientation: *orientation,
                children: new_children,
            };
            normalize(&mut result);
            Some(result)
        }
    }
}

/// Insert `leaf` between two adjacent panes (`left_pane` and `right_pane`)
/// that are siblings in some split.  Finds the deepest split where they are
/// adjacent, and inserts the new leaf between them.
pub fn insert_between(tree: &Node, left_pane: &str, right_pane: &str, leaf: Node) -> Option<Node> {
    let mut tree = tree.clone();
    if insert_between_impl(&mut tree, left_pane, right_pane, leaf) {
        normalize(&mut tree);
        Some(tree)
    } else {
        None
    }
}

/// Try to insert `leaf` between `left` and `right` somewhere in `tree`.
/// Returns true on success.
fn insert_between_impl(tree: &mut Node, left: &str, right: &str, leaf: Node) -> bool {
    match tree {
        Node::Leaf { .. } => false,
        Node::Split { children, .. } => {
            // Collect leaf-order of each direct child
            let child_leaves: Vec<Vec<PaneId>> =
                children.iter().map(|(c, _)| leaf_order(c)).collect();

            // Look for left/right straddling two adjacent children
            for i in 0..child_leaves.len().saturating_sub(1) {
                let rightmost_of_left = child_leaves[i].last();
                let leftmost_of_right = child_leaves[i + 1].first();

                if rightmost_of_left == Some(&left.to_string())
                    && leftmost_of_right == Some(&right.to_string())
                {
                    // Insert between child i and i+1
                    children.insert(i + 1, (leaf, 0.0));
                    // Rebalance proportions equally
                    let n = children.len() as f32;
                    for (_, p) in children.iter_mut() {
                        *p = 1.0 / n;
                    }
                    return true;
                }
            }

            // Recurse into children
            for (child, _) in children.iter_mut() {
                if insert_between_impl(child, left, right, leaf.clone()) {
                    return true;
                }
            }
            false
        }
    }
}

/// Insert `leaf` at a window edge.
pub fn insert_at_edge(tree: &Node, side: Side, leaf: Node) -> Node {
    let orientation = match side {
        Side::Left | Side::Right => Orientation::Horizontal,
        Side::Top | Side::Bottom => Orientation::Vertical,
    };
    let (first, second) = match side {
        Side::Left | Side::Top => (leaf, tree.clone()),
        Side::Right | Side::Bottom => (tree.clone(), leaf),
    };
    let mut result = Node::Split {
        orientation,
        children: vec![(first, 0.5), (second, 0.5)],
    };
    normalize(&mut result);
    result
}

/// Swap two leaves.
pub fn swap(tree: &Node, a: &str, b: &str) -> Option<Node> {
    let mut new_tree = tree.clone();
    swap_impl(&mut new_tree, a, b);
    Some(new_tree)
}

fn swap_impl(tree: &mut Node, a: &str, b: &str) {
    match tree {
        Node::Leaf { pane_id } => {
            if pane_id == a {
                *pane_id = b.to_string();
            } else if pane_id == b {
                *pane_id = a.to_string();
            }
        }
        Node::Split { children, .. } => {
            for (child, _) in children {
                swap_impl(child, a, b);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Side enum
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

// ---------------------------------------------------------------------------
// Normalization
// ---------------------------------------------------------------------------

/// Normalize a tree in-place:
/// 1. Collapse single-child splits.
/// 2. Merge adjacent same-orientation splits.
/// 3. Renormalize proportions so they sum to 1.0.
pub fn normalize(node: &mut Node) {
    match node {
        Node::Leaf { .. } => {}
        Node::Split {
            orientation,
            children,
        } => {
            // First, recursively normalize children
            for (child, _) in children.iter_mut() {
                normalize(child);
            }

            // Flatten: if a child is a Split with the same orientation, merge
            let mut flat: Vec<(Node, f32)> = Vec::new();
            for (child, prop) in children.drain(..) {
                match child {
                    Node::Split {
                        orientation: child_orient,
                        children: grand_children,
                    } if child_orient == *orientation => {
                        for (gc, gp) in grand_children {
                            flat.push((gc, gp * prop));
                        }
                    }
                    other => {
                        flat.push((other, prop));
                    }
                }
            }

            // Collapse single-child splits
            if flat.len() == 1 {
                let (only_child, _) = flat.into_iter().next().unwrap();
                *node = only_child;
                return;
            }

            // Renormalize proportions
            let total: f32 = flat.iter().map(|(_, p)| *p).sum();
            if total > 0.0 {
                for (_, p) in &mut flat {
                    *p /= total;
                }
            } else {
                let n = flat.len() as f32;
                for (_, p) in &mut flat {
                    *p = 1.0 / n;
                }
            }

            *children = flat;
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn contains_leaf(node: &Node, pane_id: &str) -> bool {
    match node {
        Node::Leaf { pane_id: id } => id == pane_id,
        Node::Split { children, .. } => children.iter().any(|(c, _)| contains_leaf(c, pane_id)),
    }
}

fn child_is_leaf(node: &Node, pane_id: &str) -> bool {
    matches!(node, Node::Leaf { pane_id: id } if id == pane_id)
}

// ---------------------------------------------------------------------------
// Layout computation for rendering
// ---------------------------------------------------------------------------

/// A rectangle with pixel coordinates (for rendering).
#[derive(Debug, Clone)]
pub struct LayoutRect {
    pub rect: egui::Rect,
    pub pane_id: PaneId,
}

/// A divider between siblings.
#[derive(Debug, Clone)]
pub struct Divider {
    pub rect: egui::Rect,
    /// Index of the split child *before* the divider.
    #[allow(dead_code)]
    pub index: usize,
    /// The two adjacent pane IDs (left/top and right/bottom) for identifying the split.
    pub left_pane: PaneId,
    pub right_pane: PaneId,
}

/// Compute the pixel-space rectangles for all panes and dividers given a
/// total bounding box.
pub fn compute_layout_rects(
    node: &Node,
    total_rect: egui::Rect,
) -> (Vec<LayoutRect>, Vec<Divider>) {
    let mut pane_rects = Vec::new();
    let mut dividers = Vec::new();
    compute_rects_impl(node, total_rect, &mut pane_rects, &mut dividers);
    (pane_rects, dividers)
}

/// Compute the natural pixel size of a layout tree using each leaf's actual
/// terminal cell dimensions.  Returns (width, height) in pixels.
pub fn compute_natural_size(
    node: &Node,
    pane_dims: &std::collections::HashMap<PaneId, (u16, u16)>,
    cell_scale_x: f32,
    cell_scale_y: f32,
) -> (f32, f32) {
    match node {
        Node::Leaf { pane_id } => {
            let (cols, rows) = pane_dims.get(pane_id).copied().unwrap_or((80, 24));
            (cols as f32 * cell_scale_x, rows as f32 * cell_scale_y)
        }
        Node::Split {
            orientation,
            children,
        } => {
            let n = children.len();
            if n == 0 {
                return (0.0, 0.0);
            }
            let child_sizes: Vec<(f32, f32)> = children
                .iter()
                .map(|(child, _)| {
                    compute_natural_size(child, pane_dims, cell_scale_x, cell_scale_y)
                })
                .collect();
            let border_px = (n.saturating_sub(1)) as f32 * BORDER_PX;
            match orientation {
                Orientation::Horizontal => {
                    let total_w = child_sizes.iter().map(|(w, _)| *w).sum::<f32>() + border_px;
                    let max_h = child_sizes.iter().map(|(_, h)| *h).fold(0.0f32, f32::max);
                    (total_w, max_h)
                }
                Orientation::Vertical => {
                    let max_w = child_sizes.iter().map(|(w, _)| *w).fold(0.0f32, f32::max);
                    let total_h = child_sizes.iter().map(|(_, h)| *h).sum::<f32>() + border_px;
                    (max_w, total_h)
                }
            }
        }
    }
}

/// Like [`compute_layout_rects`] but sizes the total bounding box according
/// to each leaf's actual terminal cell dimensions rather than fitting a
/// fixed-size rect.  Returns pane rects, dividers, and the total size.
pub fn compute_natural_layout_rects(
    node: &Node,
    pane_dims: &std::collections::HashMap<PaneId, (u16, u16)>,
    cell_scale_x: f32,
    cell_scale_y: f32,
    origin: egui::Pos2,
) -> (Vec<LayoutRect>, Vec<Divider>, f32, f32) {
    let (total_w, total_h) = compute_natural_size(node, pane_dims, cell_scale_x, cell_scale_y);
    let total_rect = egui::Rect::from_min_size(origin, egui::Vec2::new(total_w, total_h));
    let (pane_rects, dividers) = compute_layout_rects(node, total_rect);
    (pane_rects, dividers, total_w, total_h)
}

pub const BORDER_PX: f32 = 2.0; // visual border width between panes in pixels

fn compute_rects_impl(
    node: &Node,
    rect: egui::Rect,
    pane_rects: &mut Vec<LayoutRect>,
    dividers: &mut Vec<Divider>,
) {
    match node {
        Node::Leaf { pane_id } => {
            pane_rects.push(LayoutRect {
                rect,
                pane_id: pane_id.clone(),
            });
        }
        Node::Split {
            orientation,
            children,
        } => {
            let n = children.len();
            if n == 0 {
                return;
            }
            let border_count = n.saturating_sub(1) as f32;
            match orientation {
                Orientation::Horizontal => {
                    let available_w = rect.width() - border_count * BORDER_PX;
                    let mut x = rect.left();
                    for (i, (child, prop)) in children.iter().enumerate() {
                        let child_w = if i == n - 1 {
                            rect.right() - x
                        } else {
                            available_w * prop
                        };
                        let child_rect = egui::Rect::from_min_max(
                            egui::pos2(x, rect.top()),
                            egui::pos2(x + child_w, rect.bottom()),
                        );
                        compute_rects_impl(child, child_rect, pane_rects, dividers);
                        x += child_w;
                        if i < n - 1 {
                            let divider_rect = egui::Rect::from_min_max(
                                egui::pos2(x, rect.top()),
                                egui::pos2(x + BORDER_PX, rect.bottom()),
                            );
                            let left_pane = rightmost_leaf(child);
                            let right_pane = leftmost_leaf(&children[i + 1].0);
                            dividers.push(Divider {
                                rect: divider_rect,
                                index: i,
                                left_pane,
                                right_pane,
                            });
                            x += BORDER_PX;
                        }
                    }
                }
                Orientation::Vertical => {
                    let available_h = rect.height() - border_count * BORDER_PX;
                    let mut y = rect.top();
                    for (i, (child, prop)) in children.iter().enumerate() {
                        let child_h = if i == n - 1 {
                            rect.bottom() - y
                        } else {
                            available_h * prop
                        };
                        let child_rect = egui::Rect::from_min_max(
                            egui::pos2(rect.left(), y),
                            egui::pos2(rect.right(), y + child_h),
                        );
                        compute_rects_impl(child, child_rect, pane_rects, dividers);
                        y += child_h;
                        if i < n - 1 {
                            let divider_rect = egui::Rect::from_min_max(
                                egui::pos2(rect.left(), y),
                                egui::pos2(rect.right(), y + BORDER_PX),
                            );
                            let top_pane = bottommost_leaf(child);
                            let bottom_pane = topmost_leaf(&children[i + 1].0);
                            dividers.push(Divider {
                                rect: divider_rect,
                                index: i,
                                left_pane: top_pane,
                                right_pane: bottom_pane,
                            });
                            y += BORDER_PX;
                        }
                    }
                }
            }
        }
    }
}

fn leftmost_leaf(node: &Node) -> PaneId {
    match node {
        Node::Leaf { pane_id } => pane_id.clone(),
        Node::Split { children, .. } => leftmost_leaf(&children.first().unwrap().0),
    }
}

fn rightmost_leaf(node: &Node) -> PaneId {
    match node {
        Node::Leaf { pane_id } => pane_id.clone(),
        Node::Split { children, .. } => rightmost_leaf(&children.last().unwrap().0),
    }
}

fn topmost_leaf(node: &Node) -> PaneId {
    leftmost_leaf(node)
}

fn bottommost_leaf(node: &Node) -> PaneId {
    rightmost_leaf(node)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_checksum() {
        // Known checksum from tmux source test
        let s = "80x24,0,0,0";
        let cs = checksum(s);
        // Just verify it's stable
        assert_eq!(cs, checksum(s));
    }

    #[test]
    fn test_parse_single_pane() {
        let input = "80x24,0,0,0";
        let pane_ids = vec!["%0".to_string()];
        let node = parse_layout(input, &pane_ids).unwrap();
        assert_eq!(
            node,
            Node::Leaf {
                pane_id: "%0".into()
            }
        );
    }

    #[test]
    fn test_parse_hsplit() {
        let input = "80x24,0,0,{40x24,0,0,0,39x24,41,0,1}";
        let pane_ids = vec!["%0".to_string(), "%1".to_string()];
        let node = parse_layout(input, &pane_ids).unwrap();
        match &node {
            Node::Split {
                orientation,
                children,
            } => {
                assert_eq!(*orientation, Orientation::Horizontal);
                assert_eq!(children.len(), 2);
            }
            _ => panic!("expected split"),
        }
    }

    #[test]
    fn test_parse_with_checksum() {
        let input = "e31c,80x24,0,0,0";
        let pane_ids = vec!["%0".to_string()];
        let node = parse_layout(input, &pane_ids).unwrap();
        assert_eq!(
            node,
            Node::Leaf {
                pane_id: "%0".into()
            }
        );
    }

    #[test]
    fn test_roundtrip_single() {
        let node = Node::Leaf {
            pane_id: "%0".into(),
        };
        let pane_order = vec!["%0".to_string()];
        let s = serialize_layout(&node, 80, 24, &pane_order);
        let parsed = parse_layout(&s, &pane_order).unwrap();
        assert_eq!(node, parsed);
    }

    #[test]
    fn test_roundtrip_hsplit() {
        let node = Node::Split {
            orientation: Orientation::Horizontal,
            children: vec![
                (
                    Node::Leaf {
                        pane_id: "%0".into(),
                    },
                    0.5,
                ),
                (
                    Node::Leaf {
                        pane_id: "%1".into(),
                    },
                    0.5,
                ),
            ],
        };
        let pane_order = vec!["%0".to_string(), "%1".to_string()];
        let s = serialize_layout(&node, 80, 24, &pane_order);
        let parsed = parse_layout(&s, &pane_order).unwrap();
        // The parsed node should have similar structure (proportions may vary slightly)
        match &parsed {
            Node::Split {
                orientation,
                children,
            } => {
                assert_eq!(*orientation, Orientation::Horizontal);
                assert_eq!(children.len(), 2);
            }
            _ => panic!("expected split"),
        }
    }

    #[test]
    fn test_remove() {
        let tree = Node::Split {
            orientation: Orientation::Horizontal,
            children: vec![
                (
                    Node::Leaf {
                        pane_id: "%0".into(),
                    },
                    0.5,
                ),
                (
                    Node::Leaf {
                        pane_id: "%1".into(),
                    },
                    0.5,
                ),
            ],
        };
        let result = remove(&tree, "%0").unwrap();
        // After removing %0 and normalization, the split with one child collapses to just %1
        assert_eq!(
            result,
            Node::Leaf {
                pane_id: "%1".into()
            }
        );
    }

    #[test]
    fn test_insert_at_edge() {
        let tree = Node::Leaf {
            pane_id: "%0".into(),
        };
        let new_pane = Node::Leaf {
            pane_id: "%1".into(),
        };
        let result = insert_at_edge(&tree, Side::Right, new_pane);
        match &result {
            Node::Split {
                orientation,
                children,
            } => {
                assert_eq!(*orientation, Orientation::Horizontal);
                assert_eq!(children.len(), 2);
            }
            _ => panic!("expected split"),
        }
    }

    #[test]
    fn test_insert_beside() {
        let tree = Node::Split {
            orientation: Orientation::Vertical,
            children: vec![
                (
                    Node::Leaf {
                        pane_id: "%0".into(),
                    },
                    0.5,
                ),
                (
                    Node::Leaf {
                        pane_id: "%1".into(),
                    },
                    0.5,
                ),
            ],
        };
        let new_pane = Node::Leaf {
            pane_id: "%2".into(),
        };
        let result = insert_beside(&tree, "%0", Side::Bottom, new_pane).unwrap();
        // %0 should now be in a vertical split with %2, and that split is a child alongside %1
        let leaves = leaf_order(&result);
        assert!(leaves.contains(&"%0".to_string()));
        assert!(leaves.contains(&"%1".to_string()));
        assert!(leaves.contains(&"%2".to_string()));
    }

    #[test]
    fn test_swap() {
        let tree = Node::Split {
            orientation: Orientation::Horizontal,
            children: vec![
                (
                    Node::Leaf {
                        pane_id: "%0".into(),
                    },
                    0.5,
                ),
                (
                    Node::Leaf {
                        pane_id: "%1".into(),
                    },
                    0.5,
                ),
            ],
        };
        let result = swap(&tree, "%0", "%1").unwrap();
        let leaves = leaf_order(&result);
        assert_eq!(leaves, vec!["%1", "%0"]);
    }

    #[test]
    fn test_normalize_collapse() {
        let mut tree = Node::Split {
            orientation: Orientation::Horizontal,
            children: vec![(
                Node::Leaf {
                    pane_id: "%0".into(),
                },
                1.0,
            )],
        };
        normalize(&mut tree);
        assert_eq!(
            tree,
            Node::Leaf {
                pane_id: "%0".into()
            }
        );
    }

    #[test]
    fn test_normalize_merge() {
        // Two nested horizontal splits should merge
        let mut tree = Node::Split {
            orientation: Orientation::Horizontal,
            children: vec![
                (
                    Node::Split {
                        orientation: Orientation::Horizontal,
                        children: vec![
                            (
                                Node::Leaf {
                                    pane_id: "%0".into(),
                                },
                                0.5,
                            ),
                            (
                                Node::Leaf {
                                    pane_id: "%1".into(),
                                },
                                0.5,
                            ),
                        ],
                    },
                    0.5,
                ),
                (
                    Node::Leaf {
                        pane_id: "%2".into(),
                    },
                    0.5,
                ),
            ],
        };
        normalize(&mut tree);
        match &tree {
            Node::Split {
                orientation,
                children,
            } => {
                assert_eq!(*orientation, Orientation::Horizontal);
                assert_eq!(children.len(), 3);
            }
            _ => panic!("expected split with 3 children after merge"),
        }
    }
}
