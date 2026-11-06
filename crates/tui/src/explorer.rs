use std::collections::{HashMap, HashSet};

use anyhow::Result;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use spacehog_core::human;
use spacehog_core::model::{NodeId, NodeInfo, NodeKind};
use tui_tree_widget::{Tree, TreeItem, TreeState};

use crate::data::{self, Ctx, Toggles};
use crate::theme::Theme;
use crate::treemap;
use crate::widgets::{self, Bar};

/// A lazily loaded tree over one scan. `TreeState` owns which folders are
/// open and which row is selected; `children` caches index reads.
pub struct ExplorerState {
    pub root: NodeInfo,
    children: HashMap<NodeId, Vec<NodeInfo>>,
    pub tree: TreeState<NodeId>,
    pub preview: bool,
}

/// What a row filter needs, bundled so navigation and rendering agree.
#[derive(Clone, Copy)]
pub struct RowFilter<'a> {
    pub toggles: Toggles,
    pub text: &'a str,
    pub stale_days: i64,
}

impl ExplorerState {
    pub fn new(ctx: &Ctx, root: NodeInfo, f: RowFilter) -> Result<Self> {
        let mut s = Self {
            children: HashMap::new(),
            tree: TreeState::default(),
            preview: false,
            root,
        };
        s.load(ctx, s.root.id, f.toggles)?;
        s.select_index(f, 0);
        Ok(s)
    }

    fn load(&mut self, ctx: &Ctx, id: NodeId, t: Toggles) -> Result<()> {
        if let std::collections::hash_map::Entry::Vacant(e) = self.children.entry(id) {
            e.insert(data::children(ctx, id, t.size)?);
        }
        Ok(())
    }

    /// Re-sorts cached children after a size-mode change.
    pub fn resort(&mut self, t: Toggles) {
        for kids in self.children.values_mut() {
            kids.sort_by_key(|n| std::cmp::Reverse(n.size(t.size)));
        }
    }

    fn opened(&self) -> &HashSet<Vec<NodeId>> {
        self.tree.opened()
    }

    fn kids<'s>(
        &'s self,
        path: &[NodeId],
        open: &HashSet<Vec<NodeId>>,
        f: RowFilter,
    ) -> Vec<&'s NodeInfo> {
        let id = *path.last().unwrap_or(&self.root.id);
        let Some(kids) = self.children.get(&id) else {
            return Vec::new();
        };
        kids.iter()
            .filter(|n| {
                let mut p = path.to_vec();
                p.push(n.id);
                data::row_visible(n, f.toggles, f.text, f.stale_days, open.contains(&p))
            })
            .collect()
    }

    /// Identifier paths of every visible row, in display order.
    pub fn visible(&self, f: RowFilter) -> Vec<Vec<NodeId>> {
        let open = self.opened();
        let mut out = Vec::new();
        self.walk(&[], open, f, &mut out);
        out
    }

    fn walk(
        &self,
        path: &[NodeId],
        open: &HashSet<Vec<NodeId>>,
        f: RowFilter,
        out: &mut Vec<Vec<NodeId>>,
    ) {
        for n in self.kids(path, open, f) {
            let mut p = path.to_vec();
            p.push(n.id);
            out.push(p.clone());
            if open.contains(&p) {
                self.walk(&p, open, f, out);
            }
        }
    }

    pub fn selected(&self) -> Option<&NodeInfo> {
        let path = self.tree.selected();
        let (&id, parents) = path.split_last()?;
        let parent = parents.last().copied().unwrap_or(self.root.id);
        self.children.get(&parent)?.iter().find(|n| n.id == id)
    }

    /// The folder whose contents fill the preview: the selection if it is
    /// one, otherwise the folder holding it.
    fn preview_dir(&self) -> Option<NodeId> {
        let path = self.tree.selected();
        match self.selected() {
            Some(n) if n.kind == NodeKind::Dir => Some(n.id),
            Some(_) if path.len() >= 2 => Some(path[path.len() - 2]),
            _ => Some(self.root.id),
        }
    }

    pub fn sync_preview(&mut self, ctx: &Ctx, t: Toggles) -> Result<()> {
        if self.preview
            && let Some(id) = self.preview_dir()
        {
            self.load(ctx, id, t)?;
        }
        Ok(())
    }

    fn select_index(&mut self, f: RowFilter, i: usize) {
        let rows = self.visible(f);
        if let Some(p) = rows.get(i.min(rows.len().saturating_sub(1))) {
            self.tree.select(p.clone());
        }
    }

    pub fn step(&mut self, f: RowFilter, delta: isize) {
        let rows = self.visible(f);
        let cur = rows.iter().position(|p| p == self.tree.selected());
        let next = match cur {
            Some(i) => i
                .saturating_add_signed(delta)
                .min(rows.len().saturating_sub(1)),
            None => 0,
        };
        if let Some(p) = rows.get(next) {
            self.tree.select(p.clone());
        }
    }

    pub fn first(&mut self, f: RowFilter) {
        self.select_index(f, 0);
    }

    pub fn last(&mut self, f: RowFilter) {
        self.select_index(f, usize::MAX);
    }

    /// Opens the selected folder and moves onto its largest child.
    pub fn descend(&mut self, ctx: &Ctx, f: RowFilter) -> Result<bool> {
        let Some(n) = self.selected() else {
            return Ok(false);
        };
        if n.kind != NodeKind::Dir || n.child_count == 0 {
            return Ok(false);
        }
        let path = self.tree.selected().to_vec();
        self.load(ctx, n.id, f.toggles)?;
        self.tree.open(path.clone());
        let first = self.kids(&path, self.opened(), f).first().map(|c| c.id);
        if let Some(id) = first {
            let mut p = path;
            p.push(id);
            self.tree.select(p);
        }
        Ok(true)
    }

    /// Moves to the parent folder and closes it.
    pub fn ascend(&mut self) -> bool {
        let path = self.tree.selected().to_vec();
        if path.len() < 2 {
            return false;
        }
        let parent = path[..path.len() - 1].to_vec();
        self.tree.close(&parent);
        self.tree.select(parent);
        true
    }

    /// Opens every ancestor of `id` and selects it.
    pub fn reveal(&mut self, ctx: &Ctx, id: NodeId, t: Toggles) -> Result<bool> {
        let chain = data::lineage(ctx, id)?;
        let Some(pos) = chain.iter().position(|x| *x == self.root.id) else {
            return Ok(false);
        };
        let below = &chain[pos + 1..];
        if below.is_empty() {
            return Ok(false);
        }
        let mut path = Vec::new();
        for (i, node) in below.iter().enumerate() {
            let parent = path.last().copied().unwrap_or(self.root.id);
            self.load(ctx, parent, t)?;
            path.push(*node);
            if i + 1 < below.len() {
                self.tree.open(path.clone());
            }
        }
        self.tree.select(path);
        Ok(true)
    }

    /// Re-anchors the selection after a filter or toggle hides it.
    pub fn clamp(&mut self, f: RowFilter) {
        let rows = self.visible(f);
        if !rows.iter().any(|p| p == self.tree.selected()) {
            self.select_index(f, 0);
        }
    }

    /// Names from the scan root down to the selection.
    pub fn breadcrumb(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut parent = self.root.id;
        for id in self.tree.selected() {
            let Some(n) = self
                .children
                .get(&parent)
                .and_then(|k| k.iter().find(|n| n.id == *id))
            else {
                break;
            };
            out.push(n.name.clone());
            parent = n.id;
        }
        out
    }

    fn items(
        &self,
        path: &[NodeId],
        parent_size: u64,
        open: &HashSet<Vec<NodeId>>,
        r: &RowStyle,
    ) -> Vec<TreeItem<'static, NodeId>> {
        let kids = self.kids(path, open, r.filter);
        let size = |n: &NodeInfo| n.size(r.filter.toggles.size);
        let biggest = kids.iter().map(|n| size(n)).max().unwrap_or(1).max(1);
        kids.into_iter()
            .map(|n| {
                let mut p = path.to_vec();
                p.push(n.id);
                let is_open = open.contains(&p);
                let line = row(n, parent_size, biggest, path.len(), is_open, r);
                let kids = if is_open {
                    self.items(&p, size(n), open, r)
                } else {
                    Vec::new()
                };
                TreeItem::new(n.id, line, kids).expect("node ids are unique among siblings")
            })
            .collect()
    }
}

struct RowStyle<'a> {
    ctx: &'a Ctx,
    theme: &'a Theme,
    filter: RowFilter<'a>,
    /// Row width, so every column lines up whatever the tree indent.
    width: usize,
}

/// Unclaimed folders still get a visible bar, in a neutral grey.
const NEUTRAL: crate::theme::Rgb = (0x8a, 0x8f, 0xa8);

fn pct(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 / whole as f64
    }
}

/// Name first (the tree indents it), then columns at fixed offsets: a bar
/// against the largest sibling, size, share of the parent, bucket, idle
/// time, and child count.
fn row(
    n: &NodeInfo,
    parent_size: u64,
    biggest: u64,
    depth: usize,
    open: bool,
    r: &RowStyle,
) -> Line<'static> {
    let t = r.theme;
    let size = n.size(r.filter.toggles.size);
    let is_dir = n.kind == NodeKind::Dir;
    let arrow = match (is_dir && n.child_count > 0, open) {
        (true, true) => t.icon("\u{f115} ", "▾ "),
        (true, false) => t.icon("\u{f114} ", "▸ "),
        (false, _) if is_dir => t.icon("\u{f114} ", "  "),
        _ => t.icon("\u{f15b} ", "  "),
    };
    let wide = r.width >= 100;
    let bar_w = (r.width * 28 / 100).clamp(8, 48);
    let bucket_w = if wide { 16 } else { 0 };
    let tail = bar_w + 1 + 10 + 7 + bucket_w + 6 + 7;
    let name_w = r.width.saturating_sub(tail + 2 + depth * 2).max(6);
    let name = format!("{}{}", n.name, if is_dir { "/" } else { "" });
    let colour = if n.bucket.is_some() {
        t.bucket_of(&r.ctx.rules, n.bucket.as_deref())
    } else {
        NEUTRAL
    };
    let mut spans = vec![
        Span::styled(arrow, t.dim()),
        Span::styled(
            format!("{:<name_w$}", data::truncate_middle(&name, name_w)),
            if is_dir {
                t.text().add_modifier(Modifier::BOLD)
            } else {
                t.text()
            },
        ),
        Span::raw(" "),
    ];
    spans.extend(
        widgets::bar(Bar {
            frac: pct(size, biggest),
            width: bar_w,
            colour,
            theme: t,
        })
        .spans,
    );
    spans.push(Span::styled(
        format!(" {:>9}", human::bytes(size)),
        t.text(),
    ));
    spans.push(Span::styled(
        format!(" {:>5.1}%", pct(size, parent_size) * 100.0),
        t.dim(),
    ));
    if wide {
        let label = n
            .bucket
            .as_deref()
            .filter(|_| n.claim)
            .and_then(|b| r.ctx.rules.bucket(b))
            .map(|b| b.label.clone())
            .unwrap_or_default();
        spans.push(Span::styled(
            format!(" {:<15}", data::truncate_middle(&label, 15)),
            Style::new().fg(t.rgb(colour)),
        ));
    }
    let stale = n
        .stale_days
        .filter(|d| *d >= r.filter.stale_days)
        .map(human::days)
        .unwrap_or_default();
    spans.push(Span::styled(
        format!(" {stale:>5}"),
        Style::new().fg(t.warn()),
    ));
    let count = if is_dir && n.child_count > 0 {
        n.child_count.to_string()
    } else {
        String::new()
    };
    spans.push(Span::styled(format!(" {count:>6}"), t.dim()));
    Line::from(spans)
}

pub fn render(
    frame: &mut Frame,
    area: Rect,
    s: &mut ExplorerState,
    ctx: &Ctx,
    theme: &Theme,
    f: RowFilter,
) {
    let [crumb_area, body] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(area);
    let mut crumbs = vec![Span::styled(ctx.tilde(&s.root.path), theme.text())];
    let mut trail = s.breadcrumb();
    let selected = trail.pop();
    for name in trail {
        crumbs.push(Span::styled(" › ", theme.dim()));
        crumbs.push(Span::styled(name, theme.text()));
    }
    if let Some(name) = selected {
        crumbs.push(Span::styled("   ▸ ", theme.dim()));
        crumbs.push(Span::styled(name, theme.dim()));
    }
    frame.render_widget(Paragraph::new(Line::from(crumbs)), crumb_area);

    let show_preview = s.preview && area.width >= 80;
    let (tree_area, preview_area) = if show_preview {
        let [a, b] = Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)])
            .areas(body);
        (a, Some(b))
    } else {
        (body, None)
    };

    let style = RowStyle {
        ctx,
        theme,
        filter: f,
        width: tree_area.width as usize,
    };
    let open = s.opened();
    let items = s.items(&[], s.root.size(f.toggles.size), open, &style);
    let block = Block::new()
        .borders(Borders::TOP)
        .border_style(theme.border());
    if items.is_empty() {
        let msg = if f.text.is_empty() {
            "Nothing to show here."
        } else {
            "No rows match the filter."
        };
        frame.render_widget(
            Paragraph::new(msg).style(theme.dim()).block(block),
            tree_area,
        );
    } else {
        let tree = Tree::new(&items)
            .expect("root children are unique")
            .block(block)
            .highlight_style(theme.selected())
            .node_closed_symbol("")
            .node_open_symbol("")
            .node_no_children_symbol("");
        frame.render_stateful_widget(tree, tree_area, &mut s.tree);
    }

    if let Some(area) = preview_area {
        let dir = s.preview_dir().unwrap_or(s.root.id);
        let name = if dir == s.root.id {
            ctx.tilde(&s.root.path)
        } else {
            s.selected()
                .filter(|n| n.id == dir)
                .map(|n| n.name.clone())
                .or_else(|| s.breadcrumb().iter().rev().nth(1).cloned())
                .unwrap_or_default()
        };
        let block = Block::new()
            .borders(Borders::LEFT | Borders::TOP)
            .border_style(theme.border())
            .title(Span::styled(format!(" {name} "), theme.title()));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let kids: Vec<NodeInfo> = s
            .children
            .get(&dir)
            .map(|k| {
                k.iter()
                    .filter(|n| n.size(f.toggles.size) > 0)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let size = Rect::new(0, 0, inner.width, inner.height.saturating_mul(2));
        let tiles: Vec<_> = treemap::level(&kids, size, f.toggles, ctx, theme)
            .into_iter()
            .enumerate()
            .map(|(i, (_, _, mut tile))| {
                tile.top = i;
                tile
            })
            .collect();
        treemap::paint(
            frame.buffer_mut(),
            inner,
            &tiles,
            &[],
            &treemap::Paint {
                theme,
                selected: None,
                hover: None,
                view: None,
                outline: (0, 0, 0),
            },
        );
    }
}
