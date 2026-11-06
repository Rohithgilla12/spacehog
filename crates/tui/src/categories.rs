use std::collections::BTreeMap;

use anyhow::Result;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use spacehog_core::human;
use spacehog_core::report::{self, BucketSummary, Item, RuleSummary, TopQuery};
use spacehog_core::rules::Safety;

use crate::cleanup::describe;
use crate::data::{Ctx, Snapshot, Toggles, truncate_middle};
use crate::theme::Theme;
use crate::widgets::{self, Bar};

/// A rule's row: its totals and, once expanded, every path it matched.
pub struct RuleRow {
    pub summary: RuleSummary,
    pub items: Option<Vec<Item>>,
    pub open: bool,
}

pub struct Group {
    pub bucket: BucketSummary,
    pub rules: Vec<RuleRow>,
}

/// What a screen row points at. Bucket headers are not selectable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Bucket(usize),
    Rule(usize, usize),
    Item(usize, usize, usize),
}

#[derive(Default)]
pub struct CategoriesState {
    pub groups: Vec<Group>,
    /// Selected paths with their size and safety, so the footer needs no lookups.
    pub selected: BTreeMap<String, (u64, Safety)>,
    pub cursor: usize,
    pub stale_only: bool,
    scroll: usize,
    pub rows_on_screen: Vec<(Rect, usize)>,
}

impl CategoriesState {
    pub fn load(&mut self, ctx: &Ctx, snap: &Snapshot, t: Toggles) -> Result<()> {
        let summary = report::summary(&ctx.index, &ctx.rules, snap.scan.id, ctx.config.stale_days)?;
        let open: Vec<String> = self
            .groups
            .iter()
            .flat_map(|g| g.rules.iter())
            .filter(|r| r.open)
            .map(|r| r.summary.rule_id.clone())
            .collect();
        self.stale_only = t.stale_only;
        self.groups = summary
            .buckets
            .into_iter()
            .filter(|b| b.id != "other")
            .map(|bucket| {
                let mut rules: Vec<RuleRow> = summary
                    .rules
                    .iter()
                    .filter(|r| r.bucket == bucket.id && r.bytes > 0)
                    .filter(|r| !t.stale_only || r.stale_bytes > 0)
                    .map(|r| RuleRow {
                        summary: r.clone(),
                        items: None,
                        open: false,
                    })
                    .collect();
                rules.sort_by_key(|r| std::cmp::Reverse(rule_bytes(&r.summary, t.stale_only)));
                Group { bucket, rules }
            })
            .filter(|g| !g.rules.is_empty())
            .collect();
        self.groups
            .sort_by_key(|g| std::cmp::Reverse(group_bytes(g, t.stale_only)));
        for gi in 0..self.groups.len() {
            for ri in 0..self.groups[gi].rules.len() {
                if open.contains(&self.groups[gi].rules[ri].summary.rule_id) {
                    self.expand(ctx, snap, gi, ri)?;
                }
            }
        }
        self.cursor = self.cursor.min(self.rows().len().saturating_sub(1));
        self.skip_headers(1);
        Ok(())
    }

    fn items_of(&mut self, ctx: &Ctx, snap: &Snapshot, gi: usize, ri: usize) -> Result<&[Item]> {
        let stale = self.stale_only.then_some(ctx.config.stale_days);
        let rule = &mut self.groups[gi].rules[ri];
        if rule.items.is_none() {
            let q = TopQuery {
                limit: (rule.summary.items as usize).clamp(1, 5_000),
                category: Some(rule.summary.rule_id.clone()),
                stale_days: stale,
                ..Default::default()
            };
            rule.items = Some(report::top(&ctx.index, &ctx.rules, snap.scan.id, &q)?);
        }
        Ok(rule.items.as_deref().unwrap_or_default())
    }

    fn expand(&mut self, ctx: &Ctx, snap: &Snapshot, gi: usize, ri: usize) -> Result<()> {
        self.items_of(ctx, snap, gi, ri)?;
        self.groups[gi].rules[ri].open = true;
        Ok(())
    }

    pub fn rows(&self) -> Vec<Row> {
        let mut out = Vec::new();
        for (gi, g) in self.groups.iter().enumerate() {
            out.push(Row::Bucket(gi));
            for (ri, r) in g.rules.iter().enumerate() {
                out.push(Row::Rule(gi, ri));
                if r.open {
                    for ii in 0..r.items.as_ref().map_or(0, Vec::len) {
                        out.push(Row::Item(gi, ri, ii));
                    }
                }
            }
        }
        out
    }

    pub fn current(&self) -> Option<Row> {
        self.rows().get(self.cursor).copied()
    }

    pub fn item(&self, gi: usize, ri: usize, ii: usize) -> Option<&Item> {
        self.groups.get(gi)?.rules.get(ri)?.items.as_ref()?.get(ii)
    }

    /// The path `i`, `o`, and a lone `d` act on.
    pub fn current_item(&self) -> Option<&Item> {
        match self.current()? {
            Row::Item(g, r, i) => self.item(g, r, i),
            _ => None,
        }
    }

    /// Moves off bucket headers, preferring the direction of travel.
    fn skip_headers(&mut self, dir: isize) {
        let rows = self.rows();
        let ok = |i: usize| matches!(rows.get(i), Some(Row::Rule(..) | Row::Item(..)));
        if ok(self.cursor) {
            return;
        }
        let forward = (self.cursor..rows.len()).find(|&i| ok(i));
        let back = (0..self.cursor).rev().find(|&i| ok(i));
        let pick = if dir >= 0 {
            forward.or(back)
        } else {
            back.or(forward)
        };
        self.cursor = pick.unwrap_or(self.cursor);
    }

    pub fn step(&mut self, delta: isize) {
        let n = self.rows().len();
        self.cursor = self
            .cursor
            .saturating_add_signed(delta)
            .min(n.saturating_sub(1));
        self.skip_headers(delta.signum());
    }

    pub fn jump(&mut self, last: bool) {
        self.cursor = if last {
            self.rows().len().saturating_sub(1)
        } else {
            0
        };
        self.skip_headers(if last { -1 } else { 1 });
    }

    pub fn set_cursor(&mut self, i: usize) {
        self.cursor = i.min(self.rows().len().saturating_sub(1));
        self.skip_headers(1);
    }

    pub fn toggle_open(&mut self, ctx: &Ctx, snap: &Snapshot) -> Result<()> {
        match self.current() {
            Some(Row::Rule(g, r)) if self.groups[g].rules[r].open => {
                self.groups[g].rules[r].open = false;
            }
            Some(Row::Rule(g, r)) => self.expand(ctx, snap, g, r)?,
            _ => {}
        }
        Ok(())
    }

    pub fn collapse(&mut self) {
        let (g, r) = match self.current() {
            Some(Row::Rule(g, r) | Row::Item(g, r, _)) => (g, r),
            _ => return,
        };
        self.groups[g].rules[r].open = false;
        if let Some(i) = self.rows().iter().position(|x| *x == Row::Rule(g, r)) {
            self.cursor = i;
        }
    }

    fn select_items(&mut self, items: Vec<(String, u64, Safety)>) {
        let all = items.iter().all(|(p, _, _)| self.selected.contains_key(p));
        for (p, size, safety) in items {
            if all {
                self.selected.remove(&p);
            } else {
                self.selected.insert(p, (size, safety));
            }
        }
    }

    /// `space`: an item toggles itself; a rule toggles every path it matched.
    pub fn select(&mut self, ctx: &Ctx, snap: &Snapshot) -> Result<()> {
        let items = match self.current() {
            Some(Row::Item(g, r, i)) => self.item(g, r, i).into_iter().cloned().collect(),
            Some(Row::Rule(g, r)) => self.items_of(ctx, snap, g, r)?.to_vec(),
            _ => Vec::new(),
        };
        self.select_items(
            items
                .iter()
                .map(|i| (i.path.clone(), i.size, i.safety))
                .collect(),
        );
        Ok(())
    }

    /// `a`: every path under every listed rule, or none if all already are.
    pub fn select_all(&mut self, ctx: &Ctx, snap: &Snapshot) -> Result<()> {
        let mut all = Vec::new();
        for g in 0..self.groups.len() {
            for r in 0..self.groups[g].rules.len() {
                all.extend(
                    self.items_of(ctx, snap, g, r)?
                        .iter()
                        .map(|i| (i.path.clone(), i.size, i.safety)),
                );
            }
        }
        self.select_items(all);
        Ok(())
    }

    pub fn footer(&self) -> Option<String> {
        (!self.selected.is_empty())
            .then(|| format!("selected: {}", describe(self.selected.values().copied())))
    }
}

fn rule_bytes(r: &RuleSummary, stale: bool) -> u64 {
    if stale { r.stale_bytes } else { r.bytes }
}

fn group_bytes(g: &Group, stale: bool) -> u64 {
    g.rules.iter().map(|r| rule_bytes(&r.summary, stale)).sum()
}

pub fn render(frame: &mut Frame, area: Rect, s: &mut CategoriesState, ctx: &Ctx, t: &Theme) {
    let [head, body, foot] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(area);
    let w = area.width as usize;
    let num_w = 10;
    let wide = w >= 110;
    let label_w = if wide {
        44
    } else {
        w.saturating_sub(3 * num_w + 6 + 8).max(12)
    };
    let bar_w = if wide {
        w.saturating_sub(label_w + 3 * num_w + 6 + 8)
    } else {
        0
    };
    let max = s
        .groups
        .iter()
        .map(|g| group_bytes(g, s.stale_only))
        .max()
        .unwrap_or(1)
        .max(1);
    let title = if s.stale_only {
        format!(
            " Categories · stale only (idle {}+ days) ",
            ctx.config.stale_days
        )
    } else {
        " Categories ".to_string()
    };
    let mut header = vec![Span::styled(format!("{:<w2$}", "", w2 = 4), t.dim())];
    header.push(Span::styled(
        format!("{:<label_w$}", "bucket / rule"),
        t.dim(),
    ));
    if wide {
        header.push(Span::styled(format!(" {:<bar_w$}", ""), t.dim()));
    }
    header.push(Span::styled(
        format!(
            " {:>num_w$} {:>num_w$} {:>num_w$}",
            "total", "items", "stale"
        ),
        t.dim(),
    ));
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(title, t.title())),
            Line::from(header),
        ]),
        head,
    );

    let rows = s.rows();
    let h = body.height as usize;
    if s.cursor < s.scroll {
        s.scroll = s.cursor.saturating_sub(1);
    } else if s.cursor >= s.scroll + h {
        s.scroll = s.cursor + 1 - h;
    }
    s.rows_on_screen.clear();
    let mut lines = Vec::new();
    for (i, row) in rows.iter().enumerate().skip(s.scroll).take(h) {
        s.rows_on_screen.push((
            Rect::new(body.x, body.y + (i - s.scroll) as u16, body.width, 1),
            i,
        ));
        let line = match *row {
            Row::Bucket(g) => {
                let grp = &s.groups[g];
                let colour = t.bucket_hex(&grp.bucket.colour);
                let bytes = group_bytes(grp, s.stale_only);
                let items: u64 = grp.rules.iter().map(|r| r.summary.items).sum();
                let stale: u64 = grp.rules.iter().map(|r| r.summary.stale_bytes).sum();
                let mut spans = vec![
                    Span::styled("██ ", Style::new().fg(colour)),
                    Span::styled(
                        format!(" {:<label_w$}", truncate_middle(&grp.bucket.label, label_w)),
                        t.text().add_modifier(Modifier::BOLD),
                    ),
                ];
                if wide {
                    spans.push(Span::raw(" "));
                    spans.extend(
                        widgets::bar(Bar {
                            frac: bytes as f64 / max as f64,
                            width: bar_w,
                            colour: t.bucket_rgb(&grp.bucket.colour),
                            theme: t,
                        })
                        .spans,
                    );
                }
                spans.push(Span::styled(
                    format!(
                        " {:>num_w$} {:>num_w$} {:>num_w$}",
                        human::bytes(bytes),
                        items,
                        human::bytes(stale)
                    ),
                    t.text().add_modifier(Modifier::BOLD),
                ));
                Line::from(spans)
            }
            Row::Rule(g, r) => {
                let grp = &s.groups[g];
                let rule = &grp.rules[r];
                let sel = rule.items.as_ref().map_or(0, |v| {
                    v.iter()
                        .filter(|i| s.selected.contains_key(&i.path))
                        .count()
                });
                let arrow = if rule.open { "▾" } else { "▸" };
                let tick = match (sel, rule.items.as_ref().map_or(0, Vec::len)) {
                    (0, _) => " ",
                    (a, b) if a == b => "●",
                    _ => "◐",
                };
                let label = format!(
                    "{} {} {}",
                    arrow,
                    rule.summary.safety.glyph(),
                    rule.summary.label
                );
                let bytes = rule_bytes(&rule.summary, s.stale_only);
                let mut spans = vec![
                    Span::styled(format!("  {tick} "), Style::new().fg(t.accent())),
                    Span::styled(
                        format!("{:<label_w$}", truncate_middle(&label, label_w)),
                        t.text(),
                    ),
                ];
                if wide {
                    spans.push(Span::raw(" "));
                    spans.extend(
                        widgets::bar(Bar {
                            frac: bytes as f64 / max as f64,
                            width: bar_w,
                            colour: t.bucket_rgb(&grp.bucket.colour),
                            theme: t,
                        })
                        .spans,
                    );
                }
                spans.push(Span::styled(
                    format!(
                        " {:>num_w$} {:>num_w$} ",
                        human::bytes(bytes),
                        rule.summary.items
                    ),
                    t.text(),
                ));
                spans.push(Span::styled(
                    format!("{:>num_w$}", human::bytes(rule.summary.stale_bytes)),
                    if rule.summary.stale_bytes > 0 {
                        Style::new().fg(t.warn())
                    } else {
                        t.dim()
                    },
                ));
                Line::from(spans)
            }
            Row::Item(g, r, ii) => {
                let item = s.item(g, r, ii).expect("row exists");
                let picked = s.selected.contains_key(&item.path);
                let stale = item
                    .stale_days
                    .filter(|d| *d >= ctx.config.stale_days)
                    .map(human::days)
                    .unwrap_or_default();
                let path_w = (label_w + bar_w + if wide { 1 } else { 0 }).saturating_sub(4);
                Line::from(vec![
                    Span::styled(
                        if picked { "      ● " } else { "      · " },
                        Style::new().fg(t.accent()),
                    ),
                    Span::styled(
                        format!(
                            "{:<path_w$}",
                            truncate_middle(&ctx.tilde(&item.path), path_w)
                        ),
                        if picked {
                            t.text().add_modifier(Modifier::BOLD)
                        } else {
                            t.text()
                        },
                    ),
                    Span::styled(format!(" {:>num_w$}", human::bytes(item.size)), t.text()),
                    Span::styled(format!(" {:>num_w$}", item.safety.glyph()), t.dim()),
                    Span::styled(format!(" {stale:>num_w$}"), Style::new().fg(t.warn())),
                ])
            }
        };
        lines.push(if i == s.cursor {
            line.style(t.selected())
        } else {
            line
        });
    }
    if rows.len() <= 1 {
        lines.push(Line::styled(
            if s.stale_only {
                "Nothing stale. Press s to show everything."
            } else {
                "No categorised items in this scan."
            },
            t.dim(),
        ));
    }
    frame.render_widget(Paragraph::new(lines), body);

    let foot_line = match s.footer() {
        Some(f) => Line::from(vec![
            Span::styled(f, t.title()),
            Span::styled("   d queue  space toggle  a all", t.dim()),
        ]),
        None => Line::styled(
            "enter expand  space select  a select all  s stale only  d queue",
            t.dim(),
        ),
    };
    frame.render_widget(
        Paragraph::new(foot_line).block(Block::new().borders(Borders::NONE)),
        foot,
    );
}
