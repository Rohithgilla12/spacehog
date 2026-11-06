//! Squarified treemaps on a half-block canvas. Each terminal cell holds two
//! square-ish pixels (`▀` with fg = top, bg = bottom), so layouts run in a
//! pixel space twice as tall as the cell grid. The Treemap view and the
//! Explorer preview both draw through `paint`.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use spacehog_core::human;
use spacehog_core::model::{NodeId, NodeInfo, NodeKind};
use spacehog_core::report::Item;

use crate::data::{self, Ctx, Toggles};
use crate::theme::{self, Rgb, Theme};

pub const ZOOM: Duration = Duration::from_millis(150);
const MAX_TILES: usize = 60;
const REST: Rgb = (0x55, 0x58, 0x6e);

/// Squarified layout (Bruls, Huizing, van Wijk): rows grow while adding a
/// tile improves the worst aspect ratio. `weights` must be sorted largest
/// first. Edges are snapped from shared float boundaries, so tiles tile
/// `area` exactly with no gaps or overlap.
pub fn squarify(weights: &[u64], area: Rect) -> Vec<Rect> {
    let total: f64 = weights.iter().map(|w| *w as f64).sum();
    if weights.is_empty() || total <= 0.0 || area.is_empty() {
        return vec![Rect::default(); weights.len()];
    }
    let k = f64::from(area.width) * f64::from(area.height) / total;
    let areas: Vec<f64> = weights.iter().map(|w| (*w as f64 * k).max(1e-9)).collect();
    let (mut x, mut y) = (f64::from(area.x), f64::from(area.y));
    let (mut w, mut h) = (f64::from(area.width), f64::from(area.height));
    let mut out = Vec::with_capacity(areas.len());
    let mut start = 0;
    while start < areas.len() {
        let side = w.min(h);
        let mut end = start + 1;
        while end < areas.len()
            && worst(&areas[start..=end], side) <= worst(&areas[start..end], side)
        {
            end += 1;
        }
        let row = &areas[start..end];
        let sum: f64 = row.iter().sum();
        let last = end == areas.len();
        if w >= h {
            let cw = if last { w } else { sum / h };
            let mut cy = y;
            for (i, a) in row.iter().enumerate() {
                let ch = if i + 1 == row.len() {
                    y + h - cy
                } else {
                    a / cw
                };
                out.push(snap(x, cy, cw, ch));
                cy += ch;
            }
            x += cw;
            w -= cw;
        } else {
            let rh = if last { h } else { sum / w };
            let mut cx = x;
            for (i, a) in row.iter().enumerate() {
                let cw = if i + 1 == row.len() {
                    x + w - cx
                } else {
                    a / rh
                };
                out.push(snap(cx, y, cw, rh));
                cx += cw;
            }
            y += rh;
            h -= rh;
        }
        start = end;
    }
    out
}

fn worst(row: &[f64], side: f64) -> f64 {
    let sum: f64 = row.iter().sum();
    let max = row.iter().copied().fold(f64::MIN, f64::max);
    let min = row.iter().copied().fold(f64::MAX, f64::min);
    let (s2, sum2) = (side * side, sum * sum);
    (s2 * max / sum2).max(sum2 / (s2 * min))
}

fn snap(x: f64, y: f64, w: f64, h: f64) -> Rect {
    let (x0, y0) = (x.round(), y.round());
    let (x1, y1) = ((x + w).round(), (y + h).round());
    Rect::new(
        x0 as u16,
        y0 as u16,
        (x1 - x0).max(0.0) as u16,
        (y1 - y0).max(0.0) as u16,
    )
}

/// One painted rectangle, in pixels relative to the map's top-left.
#[derive(Debug, Clone)]
pub struct Tile {
    pub rect: Rect,
    pub rgb: Rgb,
    pub label: String,
    pub detail: String,
    /// The top-level tile this one sits inside; selection and hover act on it.
    pub top: usize,
    /// Has nested tiles drawn inside it below a one-row header.
    pub header: bool,
}

/// What a top-level tile stands for: a node, or the small items folded together.
#[derive(Debug, Clone)]
pub struct Top {
    pub rect: Rect,
    pub node: Option<NodeInfo>,
}

/// Where the layout's full extent lands on screen, in pixels. Identity is
/// `(0, 0, w, h)`; zoom transitions interpolate it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Viewport {
    fn identity(w: u16, h: u16) -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            w: f64::from(w),
            h: f64::from(h),
        }
    }

    fn of(r: Rect) -> Self {
        Self {
            x: f64::from(r.x),
            y: f64::from(r.y),
            w: f64::from(r.width).max(1.0),
            h: f64::from(r.height).max(1.0),
        }
    }

    fn lerp(self, to: Self, t: f64) -> Self {
        let m = |a: f64, b: f64| a + (b - a) * t;
        Self {
            x: m(self.x, to.x),
            y: m(self.y, to.y),
            w: m(self.w, to.w),
            h: m(self.h, to.h),
        }
    }
}

pub struct Paint<'a> {
    pub theme: &'a Theme,
    pub selected: Option<usize>,
    pub hover: Option<usize>,
    /// None draws the layout as is, with labels.
    pub view: Option<Viewport>,
    /// The selected tile's border.
    pub outline: Rgb,
}

/// Lays out one folder's children as leaf tiles, folding everything past
/// `MAX_TILES` into a single "+N more" tile so area stays honest.
pub fn level(
    nodes: &[NodeInfo],
    rect: Rect,
    t: Toggles,
    ctx: &Ctx,
    theme: &Theme,
) -> Vec<(Rect, Option<NodeInfo>, Tile)> {
    let (shown, rest) = nodes.split_at(nodes.len().min(MAX_TILES));
    let rest_bytes: u64 = rest.iter().map(|n| n.size(t.size)).sum();
    let mut weights: Vec<u64> = shown.iter().map(|n| n.size(t.size)).collect();
    if rest_bytes > 0 {
        weights.push(rest_bytes);
    }
    let rects = squarify(&weights, rect);
    let mut out: Vec<(Rect, Option<NodeInfo>, Tile)> = shown
        .iter()
        .zip(&rects)
        .map(|(n, r)| {
            let tile = Tile {
                rect: *r,
                rgb: theme.bucket_of(&ctx.rules, n.bucket.as_deref()),
                label: if n.kind == NodeKind::Dir {
                    format!("{}/", n.name)
                } else {
                    n.name.clone()
                },
                detail: human::bytes(n.size(t.size)),
                top: 0,
                header: false,
            };
            (*r, Some(n.clone()), tile)
        })
        .collect();
    if rest_bytes > 0 {
        let r = rects[rects.len() - 1];
        out.push((
            r,
            None,
            Tile {
                rect: r,
                rgb: REST,
                label: format!("+{} more", rest.len()),
                detail: human::bytes(rest_bytes),
                top: 0,
                header: false,
            },
        ));
    }
    out
}

/// Rasterises `tiles` into `area`. Later tiles paint over earlier ones, so
/// nested tiles follow their header tile.
pub fn paint(buf: &mut Buffer, area: Rect, tiles: &[Tile], tops: &[Rect], p: &Paint) {
    let (w, h) = (area.width, area.height.saturating_mul(2));
    if w == 0 || h == 0 {
        return;
    }
    let mut grid = vec![u32::MAX; usize::from(w) * usize::from(h)];
    for (i, t) in tiles.iter().enumerate() {
        let r = t.rect.intersection(Rect::new(0, 0, w, h));
        for y in r.top()..r.bottom() {
            let row = usize::from(y) * usize::from(w);
            grid[row + usize::from(r.left())..row + usize::from(r.right())].fill(i as u32);
        }
    }
    let at = |x: u16, y: u16| -> Option<usize> {
        let v = grid[usize::from(y) * usize::from(w) + usize::from(x)];
        (v != u32::MAX).then_some(v as usize)
    };
    let id = Viewport::identity(w, h);
    let view = p.view.unwrap_or(id);
    let sample = |ox: u16, oy: u16| -> Option<(usize, u16, u16)> {
        let lx = (f64::from(ox) + 0.5 - view.x) * id.w / view.w;
        let ly = (f64::from(oy) + 0.5 - view.y) * id.h / view.h;
        if lx < 0.0 || ly < 0.0 || lx >= id.w || ly >= id.h {
            return None;
        }
        let (lx, ly) = (lx as u16, ly as u16);
        at(lx, ly).map(|i| (i, lx, ly))
    };
    if p.theme.mono() {
        paint_mono(buf, area, tiles, p, &sample);
    } else {
        let colour = |hit: Option<(usize, u16, u16)>| {
            hit.map(|(i, x, y)| p.theme.rgb(pixel(&tiles[i], tops, x, y, p)))
        };
        for cy in 0..area.height {
            for cx in 0..w {
                let top = colour(sample(cx, cy * 2));
                let bottom = colour(sample(cx, cy * 2 + 1));
                let cell = &mut buf[(area.x + cx, area.y + cy)];
                match (top, bottom) {
                    (None, None) => {}
                    (t, b) => {
                        cell.set_symbol("▀");
                        cell.set_style(
                            Style::new()
                                .fg(t.unwrap_or(ratatui::style::Color::Reset))
                                .bg(b.unwrap_or(ratatui::style::Color::Reset)),
                        );
                    }
                }
            }
        }
    }
    if p.view.is_none() {
        for (i, t) in tiles.iter().enumerate() {
            label(buf, area, i, t, &at, p);
        }
    }
}

/// A tile's colour at one of its pixels: darker edges separate neighbours,
/// and the selected top-level tile gets a bright outline.
fn pixel(t: &Tile, tops: &[Rect], x: u16, y: u16, p: &Paint) -> Rgb {
    let lit = p.selected == Some(t.top);
    let hot = p.hover == Some(t.top);
    if lit && let Some(r) = tops.get(t.top) {
        let edge = x == r.left() || x + 1 == r.right() || y == r.top() || y + 1 == r.bottom();
        if edge {
            return p.outline;
        }
    }
    let r = t.rect;
    if x + 1 == r.right() || y + 1 == r.bottom() {
        return shade(p, t.rgb, 0.3);
    }
    let k = match (t.header, lit || hot) {
        (true, false) => 0.42,
        (true, true) => 0.55,
        (false, false) => 0.68,
        (false, true) => 0.95,
    };
    shade(p, t.rgb, k)
}

/// Quieter tiles are darker on the dark page and paler on the light one,
/// so depth and hover read the right way round in both themes.
fn shade(p: &Paint, rgb: Rgb, k: f32) -> Rgb {
    match p.theme.tone {
        theme::Tone::Light => {
            let (r, g, b) = rgb;
            let w = (1.0 - k) * 0.85;
            let m = |v: u8| (f32::from(v) + (255.0 - f32::from(v)) * w) as u8;
            (m(r), m(g), m(b))
        }
        _ => theme::scale(rgb, k),
    }
}

/// The fill a label sits on, matching `pixel` for the label's cell.
fn label_bg(t: &Tile, p: &Paint) -> Rgb {
    let lit = p.selected == Some(t.top) || p.hover == Some(t.top);
    let k = match (t.header, lit) {
        (true, false) => 0.42,
        (true, true) => 0.55,
        (false, false) => 0.68,
        (false, true) => 0.95,
    };
    shade(p, t.rgb, k)
}

/// The cells fully inside a tile, excluding its right and bottom edge pixels.
fn inner_cells(r: Rect) -> Rect {
    let x0 = r.left();
    let x1 = r.right().saturating_sub(1);
    let y0 = r.top().div_ceil(2);
    let y1 = r.bottom().saturating_sub(1) / 2;
    Rect::new(x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0))
}

fn label(
    buf: &mut Buffer,
    area: Rect,
    i: usize,
    t: &Tile,
    at: &dyn Fn(u16, u16) -> Option<usize>,
    p: &Paint,
) {
    let cells = inner_cells(t.rect);
    if cells.width < 8 || cells.height < 1 || (!t.header && cells.height < 2) {
        return;
    }
    if at(cells.x, cells.y * 2) != Some(i) {
        return;
    }
    let style = if p.theme.mono() {
        if p.selected == Some(t.top) {
            Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD)
        } else {
            Style::new().add_modifier(Modifier::BOLD)
        }
    } else {
        let bg = label_bg(t, p);
        Style::new()
            .bg(p.theme.rgb(bg))
            .fg(p.theme.rgb(theme::ink_on(bg)))
    };
    let width = usize::from(cells.width);
    let mut lines = vec![(data::truncate_middle(&t.label, width), true)];
    if t.header {
        let both = format!("{} {}", t.label, t.detail);
        if both.chars().count() <= width {
            lines[0] = (both, true);
        }
    } else {
        lines.push((data::truncate_middle(&t.detail, width), false));
    }
    for (row, (text, bold)) in lines.into_iter().enumerate() {
        let y = area.y + cells.y + row as u16;
        let x = area.x + cells.x;
        let s = if bold {
            style.add_modifier(Modifier::BOLD)
        } else {
            style
        };
        buf.set_stringn(x, y, &text, width, s);
    }
}

/// Without colour, tiles are outlined with box-drawing edges and the
/// selected tile is shaded.
fn paint_mono(
    buf: &mut Buffer,
    area: Rect,
    tiles: &[Tile],
    p: &Paint,
    sample: &dyn Fn(u16, u16) -> Option<(usize, u16, u16)>,
) {
    for cy in 0..area.height {
        for cx in 0..area.width {
            let Some((i, _, _)) = sample(cx, cy * 2) else {
                continue;
            };
            let right = cx + 1 >= area.width || sample(cx + 1, cy * 2).map(|h| h.0) != Some(i);
            let below = cy + 1 >= area.height || sample(cx, cy * 2 + 2).map(|h| h.0) != Some(i);
            let fill = if p.selected == Some(tiles[i].top) {
                "░"
            } else {
                " "
            };
            let symbol = match (right, below) {
                (true, true) => "┘",
                (true, false) => "│",
                (false, true) => "─",
                (false, false) => fill,
            };
            buf[(area.x + cx, area.y + cy)].set_symbol(symbol);
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum ZoomFrom {
    /// Zooming in: the new level grows out of the tile that was clicked.
    Tile(Rect),
    /// Zooming out: the parent level shrinks until this child is a tile again.
    Child(NodeId),
}

#[derive(Debug, Clone, Copy)]
struct Zoom {
    from: ZoomFrom,
    started: Instant,
    /// Eased progress, advanced by `tick` so drawing never reads the clock.
    t: f64,
}

/// The Treemap view: a trail from the scan root to the folder on screen,
/// cached children, and the last layout for navigation and mouse hits.
pub struct TreemapState {
    pub trail: Vec<NodeInfo>,
    kids: HashMap<NodeId, Vec<NodeInfo>>,
    pub depth: u8,
    pub cursor: usize,
    pub hover: Option<usize>,
    zoom: Option<Zoom>,
    focus: Option<NodeId>,
    pub tops: Vec<Top>,
    /// The map's cell area from the last draw, for mouse hits.
    pub area: Rect,
}

pub const MAX_DEPTH: u8 = 4;

impl TreemapState {
    pub fn new(root: NodeInfo) -> Self {
        Self {
            trail: vec![root],
            kids: HashMap::new(),
            depth: 2,
            cursor: 0,
            hover: None,
            zoom: None,
            focus: None,
            tops: Vec::new(),
            area: Rect::default(),
        }
    }

    pub fn current(&self) -> &NodeInfo {
        self.trail.last().expect("trail starts at the root")
    }

    fn kids_of(&mut self, ctx: &Ctx, id: NodeId, t: Toggles) -> Vec<NodeInfo> {
        self.kids
            .entry(id)
            .or_insert_with(|| data::children(ctx, id, t.size).unwrap_or_default())
            .iter()
            .filter(|n| n.size(t.size) > 0 && (t.hidden || !n.name.starts_with('.')))
            .cloned()
            .collect()
    }

    /// Drops cached children after a size-mode change or a cleanup.
    pub fn invalidate(&mut self) {
        self.kids.clear();
    }

    pub fn selected(&self) -> Option<&NodeInfo> {
        self.tops.get(self.cursor).and_then(|t| t.node.as_ref())
    }

    pub fn breadcrumb(&self) -> Vec<String> {
        self.trail.iter().skip(1).map(|n| n.name.clone()).collect()
    }

    pub fn animating(&self) -> bool {
        self.zoom.is_some()
    }

    pub fn tick(&mut self) {
        if let Some(z) = self.zoom.as_mut() {
            let raw = z.started.elapsed().as_secs_f64() / ZOOM.as_secs_f64();
            if raw >= 1.0 {
                self.zoom = None;
            } else {
                z.t = 1.0 - (1.0 - raw).powi(3);
            }
        }
    }

    pub fn zoom_in(&mut self, ctx: &Ctx, t: Toggles) -> bool {
        let Some(top) = self.tops.get(self.cursor) else {
            return false;
        };
        let rect = top.rect;
        let Some(node) = top.node.clone() else {
            return false;
        };
        if node.kind != NodeKind::Dir || self.kids_of(ctx, node.id, t).is_empty() {
            return false;
        }
        self.zoom = Some(Zoom {
            from: ZoomFrom::Tile(rect),
            started: Instant::now(),
            t: 0.0,
        });
        self.trail.push(node);
        self.cursor = 0;
        self.hover = None;
        true
    }

    pub fn zoom_out(&mut self) -> bool {
        if self.trail.len() < 2 {
            return false;
        }
        let child = self.trail.pop().expect("checked length");
        self.zoom = Some(Zoom {
            from: ZoomFrom::Child(child.id),
            started: Instant::now(),
            t: 0.0,
        });
        self.focus = Some(child.id);
        self.hover = None;
        true
    }

    /// Opens the map on `node`'s folder, keeping the trail from the root.
    pub fn show(&mut self, ctx: &Ctx, id: NodeId) -> Result<()> {
        let chain = data::lineage(ctx, id)?;
        let root = self.trail[0].clone();
        let Some(pos) = chain.iter().position(|x| *x == root.id) else {
            return Ok(());
        };
        let mut trail = vec![root];
        for id in &chain[pos + 1..] {
            if let Some(n) = ctx.index.node(*id)?
                && n.kind == NodeKind::Dir
            {
                trail.push(n);
            }
        }
        self.trail = trail;
        self.cursor = 0;
        Ok(())
    }

    pub fn set_depth(&mut self, delta: i8) {
        self.depth = self.depth.saturating_add_signed(delta).clamp(1, MAX_DEPTH);
    }

    /// Moves to the nearest top-level tile whose centre lies in the direction.
    pub fn step(&mut self, dx: i32, dy: i32) {
        let Some(cur) = self.tops.get(self.cursor) else {
            return;
        };
        let centre = |r: Rect| {
            (
                i32::from(r.x) * 2 + i32::from(r.width),
                i32::from(r.y) * 2 + i32::from(r.height),
            )
        };
        let (cx, cy) = centre(cur.rect);
        let best = self
            .tops
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != self.cursor)
            .filter_map(|(i, t)| {
                let (x, y) = centre(t.rect);
                let (along, across) = if dx != 0 {
                    ((x - cx) * dx, (y - cy).abs())
                } else {
                    ((y - cy) * dy, (x - cx).abs())
                };
                (along > 0).then_some((along + 2 * across, i))
            })
            .min();
        if let Some((_, i)) = best {
            self.cursor = i;
        }
    }

    /// The top-level tile under a terminal cell, if any.
    pub fn hit(&self, col: u16, row: u16) -> Option<usize> {
        if !self.area.contains(Position::new(col, row)) {
            return None;
        }
        let p = Position::new(col - self.area.x, (row - self.area.y) * 2);
        self.tops.iter().position(|t| t.rect.contains(p))
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        &mut self,
        ctx: &Ctx,
        theme: &Theme,
        t: Toggles,
        dir: NodeId,
        rect: Rect,
        depth: u8,
        top: Option<usize>,
        out: &mut Vec<Tile>,
    ) {
        let kids = self.kids_of(ctx, dir, t);
        for (r, node, mut tile) in level(&kids, rect, t, ctx, theme) {
            let top = top.unwrap_or_else(|| {
                self.tops.push(Top {
                    rect: r,
                    node: node.clone(),
                });
                self.tops.len() - 1
            });
            tile.top = top;
            let nest = depth < self.depth
                && r.width >= 10
                && r.height >= 6
                && node
                    .as_ref()
                    .is_some_and(|n| n.kind == NodeKind::Dir && n.child_count > 0);
            tile.header = nest;
            out.push(tile);
            if let (true, Some(n)) = (nest, node) {
                let header_end = (r.top().div_ceil(2) + 1) * 2;
                let inner = Rect::new(
                    r.x,
                    header_end,
                    r.width - 1,
                    r.bottom().saturating_sub(1).saturating_sub(header_end),
                );
                if !inner.is_empty() {
                    self.build(ctx, theme, t, n.id, inner, depth + 1, Some(top), out);
                }
            }
        }
    }

    /// The tiles for `size` pixels, and whether any were laid out.
    fn layout(&mut self, ctx: &Ctx, theme: &Theme, t: Toggles, size: Rect) -> Vec<Tile> {
        self.tops.clear();
        let mut tiles = Vec::new();
        let dir = self.current().id;
        self.build(ctx, theme, t, dir, size, 1, None, &mut tiles);
        if let Some(id) = self.focus.take()
            && let Some(i) = self
                .tops
                .iter()
                .position(|x| x.node.as_ref().is_some_and(|n| n.id == id))
        {
            self.cursor = i;
        }
        self.cursor = self.cursor.min(self.tops.len().saturating_sub(1));
        tiles
    }

    fn viewport(&self, size: Rect) -> Option<Viewport> {
        let z = self.zoom?;
        let id = Viewport::identity(size.width, size.height);
        let start = match z.from {
            ZoomFrom::Tile(r) => Viewport::of(r),
            ZoomFrom::Child(child) => {
                let r = self
                    .tops
                    .iter()
                    .find(|x| x.node.as_ref().is_some_and(|n| n.id == child))?
                    .rect;
                let v = Viewport::of(r);
                let (sx, sy) = (id.w / v.w, id.h / v.h);
                Viewport {
                    x: -v.x * sx,
                    y: -v.y * sy,
                    w: id.w * sx,
                    h: id.h * sy,
                }
            }
        };
        Some(start.lerp(id, z.t))
    }
}

pub fn render(
    frame: &mut Frame,
    area: Rect,
    s: &mut TreemapState,
    ctx: &Ctx,
    theme: &Theme,
    t: Toggles,
) {
    let [crumb_area, map, info] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(area);
    let mut crumbs = vec![Span::styled(ctx.tilde(&s.trail[0].path), theme.dim())];
    for name in s.breadcrumb() {
        crumbs.push(Span::styled(" › ", theme.dim()));
        crumbs.push(Span::styled(name, theme.text()));
    }
    let depth = format!("depth {} ", s.depth);
    let [l, r] = Layout::horizontal([Constraint::Min(1), Constraint::Length(depth.len() as u16)])
        .areas(crumb_area);
    frame.render_widget(Paragraph::new(Line::from(crumbs)), l);
    frame.render_widget(Paragraph::new(Span::styled(depth, theme.dim())), r);

    s.area = map;
    let size = Rect::new(0, 0, map.width, map.height.saturating_mul(2));
    let tiles = s.layout(ctx, theme, t, size);
    if tiles.is_empty() {
        frame.render_widget(
            Paragraph::new("This folder is empty.").style(theme.dim()),
            map,
        );
    }
    let tops: Vec<Rect> = s.tops.iter().map(|x| x.rect).collect();
    let view = s.viewport(size);
    paint(
        frame.buffer_mut(),
        map,
        &tiles,
        &tops,
        &Paint {
            theme,
            selected: Some(s.cursor),
            hover: s.hover,
            view,
            outline: theme.accent_rgb(),
        },
    );
    frame.render_widget(Paragraph::new(info_line(s, ctx, theme, t)), info);
}

fn info_line(s: &TreemapState, ctx: &Ctx, theme: &Theme, t: Toggles) -> Line<'static> {
    let total = s.current().size(t.size).max(1);
    let Some(top) = s.tops.get(s.cursor) else {
        return Line::default();
    };
    let Some(n) = &top.node else {
        return Line::styled("small items folded together", theme.dim());
    };
    let size = n.size(t.size);
    let mut spans = vec![
        Span::styled(n.name.clone(), theme.text().add_modifier(Modifier::BOLD)),
        Span::styled(format!("  {}", human::bytes(size)), theme.text()),
        Span::styled(
            format!("  {:.1}%", size as f64 / total as f64 * 100.0),
            theme.dim(),
        ),
    ];
    if n.bucket.is_some() {
        let item = Item::from_node(&ctx.rules, n);
        spans.push(Span::styled(
            format!("  {}", item.bucket_label),
            Style::new().fg(theme.bucket(&ctx.rules, n.bucket.as_deref())),
        ));
        spans.push(Span::styled(
            format!("  {} {}", item.safety.glyph(), item.safety.as_str()),
            theme.dim(),
        ));
    }
    if let Some(d) = n.stale_days.filter(|d| *d >= ctx.config.stale_days) {
        spans.push(Span::styled(
            format!("  idle {}", human::days(d)),
            Style::new().fg(theme.warn()),
        ));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn covers_exactly(area: Rect, tiles: &[Rect]) {
        let cells: u32 = tiles.iter().map(|r| r.area()).sum();
        assert_eq!(cells, area.area(), "{tiles:?}");
        for (i, a) in tiles.iter().enumerate() {
            assert!(area.contains(a.as_position()) || a.is_empty(), "{a:?}");
            for b in &tiles[i + 1..] {
                assert!(!a.intersects(*b), "{a:?} overlaps {b:?}");
            }
        }
    }

    #[test]
    fn squarify_tiles_the_area_without_gaps_or_overlap() {
        for area in [
            Rect::new(0, 0, 40, 24),
            Rect::new(3, 5, 117, 61),
            Rect::new(0, 0, 9, 3),
        ] {
            let tiles = squarify(&[500, 250, 150, 100, 60, 30, 9, 3, 1], area);
            covers_exactly(area, &tiles);
            assert!(tiles[0].area() > tiles[3].area());
        }
    }

    #[test]
    fn squarify_keeps_tiles_close_to_square() {
        let tiles = squarify(&[1; 16], Rect::new(0, 0, 40, 40));
        for r in &tiles {
            let ratio = f64::from(r.width.max(r.height)) / f64::from(r.width.min(r.height));
            assert!(ratio <= 1.5, "{r:?}");
        }
    }
}
