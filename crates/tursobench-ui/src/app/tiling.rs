//! Tiles in ordered columns with shared, resizable column and row boundaries.
use eframe::egui::{Pos2, Rect, Vec2, pos2};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub(super) const GAP: f32 = 8.0;
pub(super) const BAR: f32 = 44.0;
/// A column whose tiles are all minimized folds to a line this wide.
pub(super) const LINE: f32 = 10.0;
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub(crate) enum Kind {
    #[default]
    Data,
    Structure,
    Map,
    Sql,
    Result,
    Record,
    /// The assistant. It has no document; it is a tile like the others.
    Chat,
}
impl Kind {
    pub const PANES: [Self; 6] = [
        Self::Map,
        Self::Data,
        Self::Record,
        Self::Structure,
        Self::Sql,
        Self::Chat,
    ];
    pub fn pane(self) -> Self {
        if self == Self::Result {
            Self::Sql
        } else {
            self
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Data => "Table data",
            Self::Structure => "Structure",
            Self::Map => "Relationships",
            Self::Sql => "SQL",
            Self::Result => "Result",
            Self::Record => "Record",
            Self::Chat => "Assistant",
        }
    }
    /// The narrowest a column holding this tile gets while the window has room.
    fn min_width(self) -> f32 {
        if self == Self::Chat {
            CHAT_WIDTH
        } else {
            MIN_WIDTH
        }
    }
}
pub(super) const MIN_WIDTH: f32 = 180.0;
const CHAT_WIDTH: f32 = 260.0;
const MIN_HEIGHT: f32 = 96.0;
/// A tile dragged this close to the workspace's right edge starts a new column.
const EDGE: f32 = 20.0;

/// The two-column splits saved before tiles could move; read only to migrate.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq)]
#[serde(default)]
pub(super) struct Splits {
    pub column: f32,
    pub rows: [f32; 5],
    pub sql_editor: f32,
}
impl Default for Splits {
    fn default() -> Self {
        Self {
            column: 0.63,
            rows: [0.85, 1.35, 1.08, 1.03, 1.0],
            sql_editor: 0.44,
        }
    }
}
impl Splits {
    pub fn valid(&self) -> bool {
        self.column.is_finite()
            && self.column > 0.0
            && self.column < 1.0
            && self.rows.iter().all(|w| w.is_finite() && *w > 0.0)
            && self.rows.iter().sum::<f32>().is_finite()
            && self.sql_editor.is_finite()
            && self.sql_editor > 0.0
            && self.sql_editor < 1.0
    }
}
/// The assistant's default width, as a share of the two tile columns' width.
const CHAT_SHARE: f32 = 0.52;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(super) struct Column {
    /// Top to bottom. Never empty: a column that loses its last tile is removed.
    pub tiles: Vec<Kind>,
    /// Share of the workspace width among the columns that are open.
    pub width: f32,
    /// Tiles that were open when the column folded; unfolding opens these again.
    #[serde(default)]
    pub unfold: Vec<Kind>,
}
/// Where every tile sits. Saved per database; the default is the fixed layout
/// the workspace had before tiles could move.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(super) struct Layout {
    /// Left to right.
    pub columns: Vec<Column>,
    /// Each tile's share of its column's height. It follows the tile between columns.
    pub rows: BTreeMap<Kind, f32>,
    /// The SQL tile's editor share of its height when results show below it.
    pub sql_editor: f32,
}
impl Default for Layout {
    fn default() -> Self {
        Splits::default().into()
    }
}
impl From<Splits> for Layout {
    fn from(splits: Splits) -> Self {
        let column = |tiles: &[Kind], width| Column {
            tiles: tiles.to_vec(),
            width,
            unfold: vec![],
        };
        Self {
            columns: vec![
                column(&[Kind::Map, Kind::Data], splits.column),
                column(
                    &[Kind::Record, Kind::Structure, Kind::Sql],
                    1.0 - splits.column,
                ),
                column(&[Kind::Chat], CHAT_SHARE),
            ],
            rows: Kind::PANES
                .into_iter()
                .zip(splits.rows.into_iter().chain([1.0]))
                .collect(),
            sql_editor: splits.sql_editor,
        }
    }
}
/// Where a moved tile lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Target {
    /// Into an existing column, before the tile now at `index`.
    Slot { column: usize, index: usize },
    /// Alone in a new column after the last one. Columns start nowhere else.
    NewColumn,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Direction {
    Left,
    Right,
    Up,
    Down,
}
impl Layout {
    pub fn valid(&self) -> bool {
        let tiles: Vec<_> = self
            .columns
            .iter()
            .flat_map(|c| &c.tiles)
            .copied()
            .collect();
        tiles.len() == Kind::PANES.len()
            && Kind::PANES.iter().all(|kind| tiles.contains(kind))
            && self.columns.iter().all(|column| {
                !column.tiles.is_empty()
                    && column.width.is_finite()
                    && column.width > 0.0
                    && column.unfold.iter().all(|kind| column.tiles.contains(kind))
            })
            && self
                .columns
                .iter()
                .map(|c| c.width)
                .sum::<f32>()
                .is_finite()
            && self.rows.keys().all(|kind| Kind::PANES.contains(kind))
            && self.rows.values().all(|w| w.is_finite() && *w > 0.0)
            && self.rows.values().sum::<f32>().is_finite()
            && self.sql_editor.is_finite()
            && self.sql_editor > 0.0
            && self.sql_editor < 1.0
    }
    pub fn column_of(&self, kind: Kind) -> Option<usize> {
        self.columns.iter().position(|c| c.tiles.contains(&kind))
    }
    /// Every tile, left to right and top to bottom.
    pub fn order(&self) -> Vec<Kind> {
        self.columns
            .iter()
            .flat_map(|c| &c.tiles)
            .copied()
            .collect()
    }
    fn row(&self, kind: Kind) -> f32 {
        self.rows.get(&kind).copied().unwrap_or(1.0)
    }
    /// "Record, Structure and SQL": a column's tiles, for tooltips and accessible names.
    pub fn names(&self, column: usize) -> String {
        let labels: Vec<_> = self.columns[column]
            .tiles
            .iter()
            .map(|k| k.label())
            .collect();
        match labels.split_last() {
            Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
            _ => labels.concat(),
        }
    }
    /// Whether every tile in the column is minimized.
    pub fn all_minimized(&self, column: usize, minimized: &BTreeSet<Kind>) -> bool {
        self.columns[column]
            .tiles
            .iter()
            .all(|kind| minimized.contains(kind))
    }
    /// Which columns are drawn as lines: those whose tiles are all minimized,
    /// unless that is every column, which would leave nothing but lines.
    pub fn folds(&self, minimized: &BTreeSet<Kind>) -> Vec<bool> {
        let folds: Vec<_> = (0..self.columns.len())
            .map(|column| self.all_minimized(column, minimized))
            .collect();
        if folds.iter().all(|folded| *folded) {
            vec![false; folds.len()]
        } else {
            folds
        }
    }
    /// Fold a column to a line, remembering which of its tiles were open.
    pub fn fold(&mut self, column: usize, minimized: &mut BTreeSet<Kind>) {
        let column = &mut self.columns[column];
        column.unfold = column
            .tiles
            .iter()
            .copied()
            .filter(|kind| !minimized.contains(kind))
            .collect();
        minimized.extend(&column.tiles);
    }
    /// Open the tiles that were open before the column folded (all if none were).
    pub fn unfold(&mut self, column: usize, minimized: &mut BTreeSet<Kind>) {
        let column = &mut self.columns[column];
        let open = std::mem::take(&mut column.unfold);
        for kind in if open.is_empty() {
            &column.tiles
        } else {
            &open
        } {
            minimized.remove(kind);
        }
    }
    /// Whether `kind` can leave for a new column with every column keeping its
    /// minimum width in a workspace `width` wide.
    pub fn fits_new_column(&self, kind: Kind, width: f32, minimized: &BTreeSet<Kind>) -> bool {
        let folds = self.folds(minimized);
        let mut columns = 1;
        let mut needed = kind.min_width();
        for (column, folded) in self.columns.iter().zip(folds) {
            let mut rest = column
                .tiles
                .iter()
                .filter(|other| **other != kind)
                .peekable();
            if rest.peek().is_none() {
                continue;
            }
            columns += 1;
            needed += if folded {
                LINE
            } else {
                rest.map(|kind| kind.min_width()).fold(0.0, f32::max)
            };
        }
        needed + GAP * (columns - 1) as f32 <= width
    }
    /// Where the tile goes when moved one step with the keyboard. Left and right
    /// join the neighbouring column; right from the last column starts a new one.
    pub fn step(
        &self,
        kind: Kind,
        direction: Direction,
        width: f32,
        minimized: &BTreeSet<Kind>,
    ) -> Option<Target> {
        let column = self.column_of(kind)?;
        let tiles = &self.columns[column].tiles;
        let index = tiles.iter().position(|other| *other == kind)?;
        let join = |column: usize| Target::Slot {
            column,
            index: index.min(self.columns[column].tiles.len()),
        };
        match direction {
            Direction::Up => (index > 0).then(|| Target::Slot {
                column,
                index: index - 1,
            }),
            Direction::Down => (index + 1 < tiles.len()).then(|| Target::Slot {
                column,
                index: index + 2,
            }),
            Direction::Left => (column > 0).then(|| join(column - 1)),
            Direction::Right if column + 1 < self.columns.len() => Some(join(column + 1)),
            Direction::Right => (tiles.len() > 1 && self.fits_new_column(kind, width, minimized))
                .then_some(Target::NewColumn),
        }
    }
    /// Move a tile. A column left empty is removed and the others share its
    /// width in proportion. Returns whether anything moved.
    pub fn move_tile(&mut self, kind: Kind, target: Target) -> bool {
        let Some(from) = self.column_of(kind) else {
            return false;
        };
        let at = self.columns[from]
            .tiles
            .iter()
            .position(|other| *other == kind)
            .unwrap();
        match target {
            Target::Slot { column, index } => {
                if column >= self.columns.len()
                    || index > self.columns[column].tiles.len()
                    || column == from && (index == at || index == at + 1)
                {
                    return false;
                }
                self.columns[from].tiles.remove(at);
                let index = index - usize::from(column == from && index > at);
                self.columns[column].tiles.insert(index, kind);
            }
            Target::NewColumn => {
                if from + 1 == self.columns.len() && self.columns[from].tiles.len() == 1 {
                    return false;
                }
                self.columns[from].tiles.remove(at);
                let width =
                    self.columns.iter().map(|c| c.width).sum::<f32>() / self.columns.len() as f32;
                self.columns.push(Column {
                    tiles: vec![kind],
                    width,
                    unfold: vec![],
                });
            }
        }
        for column in &mut self.columns {
            column.unfold.retain(|other| *other != kind);
        }
        self.columns.retain(|column| !column.tiles.is_empty());
        true
    }
    pub fn resize(
        &mut self,
        divider: Divider,
        delta: Vec2,
        frame: &Frame,
        minimized: &BTreeSet<Kind>,
    ) {
        match divider {
            Divider::Columns(left) => {
                let (Some(a), Some(b)) = (frame.columns.get(left), frame.columns.get(left + 1))
                else {
                    return;
                };
                let total = a.rect.width() + b.rect.width();
                if total <= 0.0 {
                    return;
                }
                let floor = |column: usize| self.min_width(column).min(total * 0.5);
                let first = (a.rect.width() + delta.x).clamp(floor(left), total - floor(left + 1));
                let weight = self.columns[left].width + self.columns[left + 1].width;
                let share = (weight * first / total).max(f32::EPSILON);
                self.columns[left].width = share;
                self.columns[left + 1].width = (weight - share).max(f32::EPSILON);
            }
            Divider::Rows(a, b) => {
                let (Some(above), Some(below)) = (frame.rect(a), frame.rect(b)) else {
                    return;
                };
                let total = above.height() + below.height();
                if total <= 0.0 {
                    return;
                }
                let min = MIN_HEIGHT.min(total * 0.5);
                let first = (above.height() + delta.y).clamp(min, total - min);
                let Some(column) = self.column_of(a) else {
                    return;
                };
                let active: Vec<_> = self.columns[column]
                    .tiles
                    .iter()
                    .copied()
                    .filter(|kind| !minimized.contains(kind))
                    .filter_map(|kind| Some((kind, frame.rect(kind)?.height())))
                    .collect();
                let height: f32 = active.iter().map(|(_, h)| h).sum();
                let weight: f32 = active.iter().map(|(kind, _)| self.row(*kind)).sum();
                for (kind, own) in active {
                    let h = if kind == a {
                        first
                    } else if kind == b {
                        total - first
                    } else {
                        own
                    };
                    self.rows
                        .insert(kind, (h / height * weight).max(f32::EPSILON));
                }
            }
        }
    }
    fn min_width(&self, column: usize) -> f32 {
        self.columns[column]
            .tiles
            .iter()
            .map(|kind| kind.min_width())
            .fold(0.0, f32::max)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Divider {
    /// Between this column and the next.
    Columns(usize),
    /// Between two tiles stacked in one column.
    Rows(Kind, Kind),
}
impl Divider {
    pub fn vertical(self) -> bool {
        matches!(self, Self::Columns(_))
    }
    pub fn label(self) -> String {
        match self {
            Self::Columns(_) => "Resize workspace columns".into(),
            Self::Rows(a, b) => format!("Resize {} and {}", a.label(), b.label()),
        }
    }
}
/// One column as drawn: its full-height rectangle, and whether it is a line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Lane {
    pub rect: Rect,
    pub folded: bool,
}
/// Where everything is drawn this frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct Frame {
    /// In layout order. A folded column's tiles are listed but not drawn.
    pub tiles: Vec<(Kind, Rect)>,
    /// Empty while one tile is expanded to the workspace.
    pub columns: Vec<Lane>,
}
impl Frame {
    pub fn rect(&self, kind: Kind) -> Option<Rect> {
        self.tiles
            .iter()
            .find(|(other, _)| *other == kind)
            .map(|(_, rect)| *rect)
    }
}
pub(super) fn dividers(
    frame: &Frame,
    layout: &Layout,
    minimized: &BTreeSet<Kind>,
    area: Rect,
) -> Vec<(Divider, Rect)> {
    let mut handles = vec![];
    // A folded column stays a line; there is nothing to drag beside it.
    for (index, pair) in frame.columns.windows(2).enumerate() {
        if !pair[0].folded && !pair[1].folded {
            handles.push((
                Divider::Columns(index),
                Rect::from_min_max(
                    pos2(pair[0].rect.right(), pair[0].rect.top()),
                    pos2(pair[1].rect.left(), area.bottom()),
                ),
            ));
        }
    }
    for (column, lane) in layout.columns.iter().zip(&frame.columns) {
        if lane.folded {
            continue;
        }
        for pair in column.tiles.windows(2) {
            if minimized.contains(&pair[0]) || minimized.contains(&pair[1]) {
                continue;
            }
            let (Some(before), Some(after)) = (frame.rect(pair[0]), frame.rect(pair[1])) else {
                continue;
            };
            handles.push((
                Divider::Rows(pair[0], pair[1]),
                Rect::from_min_max(
                    pos2(before.left(), before.bottom()),
                    pos2(before.right(), after.top()),
                ),
            ));
        }
    }
    handles
}
pub(super) fn layout(
    area: Rect,
    expanded: Option<Kind>,
    minimized: &BTreeSet<Kind>,
    layout: &Layout,
) -> Frame {
    if let Some(kind) = expanded {
        return Frame {
            tiles: vec![(kind, area)],
            columns: vec![],
        };
    }
    let count = layout.columns.len();
    let folds = layout.folds(minimized);
    let lines = folds.iter().filter(|folded| **folded).count();
    let gap = if count > 1 {
        GAP.min(area.width().max(0.0) / (count - 1) as f32)
    } else {
        0.0
    };
    let space = (area.width() - gap * (count.max(1) - 1) as f32).max(0.0);
    let line = LINE.min(space / lines.max(1) as f32);
    let open: Vec<_> = (0..count).filter(|column| !folds[*column]).collect();
    let room = (space - line * lines as f32).max(0.0);
    let floors: Vec<_> = open
        .iter()
        .map(|column| layout.min_width(*column))
        .collect();
    // Too narrow for every minimum: all the minimums shrink together.
    let squeeze = (room / floors.iter().sum::<f32>()).min(1.0);
    let shares: Vec<_> = open
        .iter()
        .zip(&floors)
        .map(|(column, floor)| (layout.columns[*column].width, floor * squeeze))
        .collect();
    let mut widths = share(room, &shares).into_iter();
    let mut frame = Frame::default();
    let mut x = area.left();
    for (column, folded) in layout.columns.iter().zip(folds) {
        let width = if folded {
            line
        } else {
            widths.next().unwrap_or(0.0)
        };
        let rect = Rect::from_min_max(
            pos2(x.min(area.right()), area.top()),
            pos2((x + width).min(area.right()), area.bottom()),
        );
        frame.columns.push(Lane { rect, folded });
        stack(&mut frame.tiles, rect, &column.tiles, minimized, layout);
        x += width + gap;
    }
    frame
}
/// Split `room` by weight, giving each part at least its floor. The floors fit.
fn share(room: f32, parts: &[(f32, f32)]) -> Vec<f32> {
    let mut sizes = vec![0.0; parts.len()];
    let mut fixed = vec![false; parts.len()];
    loop {
        let free: Vec<_> = (0..parts.len()).filter(|i| !fixed[*i]).collect();
        let rest = (room
            - (0..parts.len())
                .filter(|i| fixed[*i])
                .map(|i| sizes[i])
                .sum::<f32>())
        .max(0.0);
        let weight: f32 = free.iter().map(|i| parts[*i].0).sum();
        let mut changed = false;
        for (position, i) in free.iter().copied().enumerate() {
            let (own, floor) = parts[i];
            let size = if free.len() == 1 {
                rest
            } else if weight > 0.0 && weight.is_finite() {
                rest * (own / weight)
            } else {
                rest / (free.len() - position) as f32
            };
            if size < floor - 0.001 && free.len() > 1 {
                sizes[i] = floor;
                fixed[i] = true;
                changed = true;
            } else {
                sizes[i] = size;
            }
        }
        if !changed {
            return sizes;
        }
    }
}
fn stack(
    tiles: &mut Vec<(Kind, Rect)>,
    rect: Rect,
    kinds: &[Kind],
    minimized: &BTreeSet<Kind>,
    layout: &Layout,
) {
    let count = kinds.len();
    let gap = if count > 1 {
        GAP.min(rect.height().max(0.0) / (count - 1) as f32)
    } else {
        0.0
    };
    let collapsed = kinds.iter().filter(|kind| minimized.contains(kind)).count();
    let available = (rect.height() - gap * (count.max(1) - 1) as f32).max(0.0);
    let bar = BAR.min(available / count.max(1) as f32);
    let mut remaining = (available - collapsed as f32 * bar).max(0.0);
    let mut expanded = count - collapsed;
    let min = MIN_HEIGHT.min(remaining / expanded.max(1) as f32);
    let mut weights: f32 = kinds
        .iter()
        .filter(|kind| !minimized.contains(kind))
        .map(|kind| layout.row(*kind))
        .sum();
    let mut y = rect.top();
    for kind in kinds.iter().copied() {
        let height = if minimized.contains(&kind) {
            bar
        } else {
            let floor = min.min(remaining);
            let maximum = (remaining - floor * (expanded - 1) as f32)
                .max(floor)
                .min(remaining);
            let height = if expanded == 1 || remaining == 0.0 {
                remaining
            } else {
                (remaining * (layout.row(kind) / weights)).clamp(floor, maximum)
            };
            remaining = (remaining - height).max(0.0);
            weights -= layout.row(kind);
            expanded -= 1;
            height
        };
        tiles.push((
            kind,
            Rect::from_min_max(
                pos2(rect.left(), y.min(rect.bottom())),
                pos2(rect.right(), (y + height).min(rect.bottom())),
            ),
        ));
        y += height + gap;
    }
}
/// Where a tile dragged to `pointer` would land, or `None` where dropping it
/// changes nothing. Past the last column it starts a new one; over a column it
/// joins at the slot nearest the pointer; over a line it joins that column.
pub(super) fn drop_target(
    pointer: Pos2,
    kind: Kind,
    frame: &Frame,
    layout: &Layout,
    minimized: &BTreeSet<Kind>,
    area: Rect,
) -> Option<Target> {
    let from = layout.column_of(kind)?;
    // Released over the sidebar or the status bar, the tile stays where it was.
    if frame.columns.len() != layout.columns.len()
        || pointer.x < area.left() - GAP
        || !area.y_range().expand(GAP).contains(pointer.y)
    {
        return None;
    }
    if pointer.x > area.right() - EDGE {
        let alone = layout.columns[from].tiles.len() == 1;
        return (!(alone && from + 1 == layout.columns.len())
            && layout.fits_new_column(kind, area.width(), minimized))
        .then_some(Target::NewColumn);
    }
    // The gap between two columns belongs to the nearer one.
    let column = (0..frame.columns.len()).min_by(|a, b| {
        let distance = |column: &usize| {
            let x = frame.columns[*column].rect.x_range();
            (x.min - pointer.x).max(pointer.x - x.max).max(0.0)
        };
        distance(a).total_cmp(&distance(b))
    })?;
    let tiles = &layout.columns[column].tiles;
    if frame.columns[column].folded {
        return (column != from).then_some(Target::Slot {
            column,
            index: tiles.len(),
        });
    }
    let index = tiles
        .iter()
        .position(|other| {
            frame
                .rect(*other)
                .is_some_and(|rect| pointer.y < rect.center().y)
        })
        .unwrap_or(tiles.len());
    if column == from {
        let at = tiles.iter().position(|other| *other == kind)?;
        if index == at || index == at + 1 {
            return None;
        }
    }
    Some(Target::Slot { column, index })
}
/// The line that shows where a dragged tile would land.
pub(super) fn marker(target: Target, frame: &Frame, layout: &Layout, area: Rect) -> Rect {
    const THICK: f32 = 4.0;
    match target {
        Target::NewColumn => Rect::from_min_max(
            pos2(area.right() + (GAP - THICK) / 2.0, area.top()),
            pos2(area.right() + (GAP + THICK) / 2.0, area.bottom()),
        ),
        Target::Slot { column, index } => {
            let lane = frame.columns[column];
            if lane.folded {
                return lane.rect;
            }
            let tiles = &layout.columns[column].tiles;
            let edge = |index: usize| frame.rect(tiles[index]);
            let y = match (
                index.checked_sub(1).and_then(edge),
                (index < tiles.len()).then(|| edge(index)).flatten(),
            ) {
                (Some(above), Some(below)) => (above.bottom() + below.top()) / 2.0,
                (None, Some(below)) => below.top() - GAP / 2.0,
                (Some(above), None) => above.bottom() + GAP / 2.0,
                (None, None) => area.top(),
            };
            Rect::from_x_y_ranges(lane.rect.x_range(), (y - THICK / 2.0)..=(y + THICK / 2.0))
        }
    }
}
#[cfg(test)]
mod tests {
    use super::Kind::{Chat, Data, Map, Record, Sql, Structure};
    use super::{
        BAR, Column, Direction, Divider, Frame, Kind, LINE, Layout, Splits, Target, dividers,
        drop_target, layout, marker,
    };
    use eframe::egui::{Pos2, Rect, pos2, vec2};
    use std::collections::BTreeSet;
    fn arranged(columns: &[&[Kind]]) -> Layout {
        Layout {
            columns: columns
                .iter()
                .map(|tiles| Column {
                    tiles: tiles.to_vec(),
                    width: 1.0,
                    unfold: vec![],
                })
                .collect(),
            ..Layout::default()
        }
    }
    fn arrangements() -> Vec<Layout> {
        let mut uneven = Layout::default();
        uneven.columns[0].width = 1e-20;
        uneven.columns[2].width = 1e20;
        uneven.rows.insert(Map, 1e-20);
        uneven.rows.insert(Record, 1e20);
        vec![
            Layout::default(),
            uneven,
            Splits {
                column: 0.99,
                rows: [1.0, 2.0, 3.0, 1.0, 2.0],
                ..Splits::default()
            }
            .into(),
            arranged(&[&[Map, Data, Record, Structure, Sql, Chat]]),
            arranged(&[&[Chat], &[Map], &[Data], &[Record], &[Structure], &[Sql]]),
            arranged(&[&[Map, Data, Structure], &[Record, Sql], &[Chat]]),
            arranged(&[&[Map, Data], &[Record, Structure, Sql, Chat]]),
        ]
    }
    /// Every combination of minimized tiles.
    fn minimized_sets() -> impl Iterator<Item = BTreeSet<Kind>> {
        (0..1 << Kind::PANES.len()).map(|mask| {
            Kind::PANES
                .into_iter()
                .enumerate()
                .filter_map(|(i, k)| (mask & (1 << i) != 0).then_some(k))
                .collect()
        })
    }
    /// Lines are `LINE` wide with no divider beside them; open neighbours have
    /// one; columns never overlap, and the open ones fill the width.
    fn check_columns(frame: &Frame, arrangement: &Layout, minimized: &BTreeSet<Kind>, area: Rect) {
        let folds = arrangement.folds(minimized);
        assert!(folds.iter().any(|folded| !folded), "something stays open");
        let handles = dividers(frame, arrangement, minimized, area);
        let dragged = |column: usize| handles.iter().any(|(d, _)| *d == Divider::Columns(column));
        for (index, lane) in frame.columns.iter().enumerate() {
            assert_eq!(lane.folded, folds[index]);
            assert!(area.contains_rect(lane.rect));
            let next_open = frame.columns.get(index + 1).is_some_and(|n| !n.folded);
            if lane.folded {
                if area.width() >= 720.0 {
                    assert!((lane.rect.width() - LINE).abs() < 0.01);
                }
                assert!(!dragged(index) && (index == 0 || !dragged(index - 1)));
            } else if next_open {
                assert!(dragged(index));
            }
        }
        for pair in frame.columns.windows(2) {
            assert!(pair[0].rect.right() <= pair[1].rect.left() + 0.01);
        }
        if area.width() >= 720.0 && arrangement.columns.len() < 6 {
            let last = frame.columns.last().unwrap().rect;
            assert!((last.right() - area.right()).abs() < 0.5, "{last:?}");
        }
    }
    /// Drawn tiles fill their column's width, minimized ones are bars, none
    /// overlap, and each expands alone to the whole workspace.
    fn check_tiles(frame: &Frame, arrangement: &Layout, minimized: &BTreeSet<Kind>, area: Rect) {
        let folds = arrangement.folds(minimized);
        let drawn = |kind: &Kind| !folds[arrangement.column_of(*kind).unwrap()];
        for (index, (kind, rect)) in frame.tiles.iter().enumerate() {
            assert!(area.contains_rect(*rect));
            if !drawn(kind) {
                continue;
            }
            let column = arrangement.column_of(*kind).unwrap();
            assert_eq!(rect.x_range(), frame.columns[column].rect.x_range());
            if minimized.contains(kind) {
                assert!(rect.height() <= BAR + 0.01);
                if area.height() >= 480.0 {
                    assert!((rect.height() - BAR).abs() < 0.01);
                }
            }
            assert!(
                frame.tiles[index + 1..]
                    .iter()
                    .all(|(other, next)| !drawn(other) || !rect.intersects(*next))
            );
            let whole = layout(area, Some(*kind), minimized, arrangement);
            assert_eq!(whole.tiles, vec![(*kind, area)]);
            assert!(dividers(&whole, arrangement, minimized, area).is_empty());
        }
    }
    #[test]
    fn tiles_minimize_fold_and_reclaim_space_without_overlap_in_any_arrangement() {
        for size in [
            vec2(1.0, 1.0),
            vec2(40.0, 20.0),
            vec2(720.0, 480.0),
            vec2(1040.0, 640.0),
            vec2(3100.0, 1360.0),
        ] {
            let area = Rect::from_min_size(Pos2::ZERO, size);
            for arrangement in arrangements() {
                assert!(arrangement.valid());
                for minimized in minimized_sets() {
                    let frame = layout(area, None, &minimized, &arrangement);
                    let order: Vec<_> = frame.tiles.iter().map(|t| t.0).collect();
                    assert_eq!(order, arrangement.order());
                    check_columns(&frame, &arrangement, &minimized, area);
                    check_tiles(&frame, &arrangement, &minimized, area);
                }
            }
        }
    }
    #[test]
    fn the_default_layout_is_the_fixed_one_with_the_assistant_beside_it() {
        let default = Layout::default();
        assert_eq!(default.order(), Kind::PANES);
        assert_eq!(
            default
                .columns
                .iter()
                .map(|c| c.tiles.len())
                .collect::<Vec<_>>(),
            [2, 3, 1]
        );
        assert_eq!(default.names(0), "Relationships and Table data");
        assert_eq!(default.names(1), "Record, Structure and SQL");
        assert_eq!(default.names(2), "Assistant");
        // With the assistant folded, the two tile columns split as they always did.
        let area = Rect::from_min_size(Pos2::ZERO, vec2(1018.0, 718.0));
        let frame = layout(area, None, &BTreeSet::from([Chat]), &default);
        let tiles = 1018.0 - 2.0 * 8.0 - LINE;
        assert!((frame.rect(Map).unwrap().width() - tiles * 0.63).abs() < 0.01);
        assert!((frame.rect(Sql).unwrap().right() - (tiles + 8.0)).abs() < 0.01);
        let left = 718.0 - 8.0;
        assert!((frame.rect(Map).unwrap().height() - left * 0.85 / 2.2).abs() < 0.01);
        // Saved two-column splits carry over unchanged.
        let old = Splits {
            column: 0.4,
            rows: [1.0, 2.0, 3.0, 1.0, 2.0],
            sql_editor: 0.3,
        };
        let migrated = Layout::from(old);
        assert!(migrated.valid());
        assert_eq!(migrated.columns[0].width, 0.4);
        assert_eq!(migrated.rows[&Record], 3.0);
        assert_eq!(migrated.sql_editor, 0.3);
    }
    #[test]
    fn moving_any_tile_anywhere_keeps_every_tile_once_and_drops_empty_columns() {
        for arrangement in arrangements() {
            for kind in Kind::PANES {
                let mut targets = vec![Target::NewColumn];
                for (column, tiles) in arrangement.columns.iter().enumerate() {
                    targets.extend(
                        (0..=tiles.tiles.len()).map(|index| Target::Slot { column, index }),
                    );
                }
                for target in targets {
                    let mut moved = arrangement.clone();
                    let changed = moved.move_tile(kind, target);
                    assert!(moved.valid(), "{kind:?} to {target:?}");
                    assert_eq!(changed, moved != arrangement);
                    if changed && target == Target::NewColumn {
                        assert_eq!(moved.columns.last().unwrap().tiles, [kind]);
                    }
                }
            }
        }
    }
    #[test]
    fn a_column_emptied_of_tiles_is_removed_and_new_columns_start_only_on_the_right() {
        let width = 1078.0;
        let none = BTreeSet::new();
        let mut layout = Layout::default();
        // Right joins the column immediately to the right; it never starts one between.
        assert_eq!(
            layout.step(Data, Direction::Right, width, &none),
            Some(Target::Slot {
                column: 1,
                index: 1
            })
        );
        assert_eq!(layout.step(Map, Direction::Left, width, &none), None);
        assert_eq!(layout.step(Map, Direction::Up, width, &none), None);
        // The assistant alone in the last column has nowhere further right to go.
        assert_eq!(layout.step(Chat, Direction::Right, width, &none), None);
        assert!(!layout.clone().move_tile(Chat, Target::NewColumn));
        // Moving it left empties its column, which goes.
        let target = layout.step(Chat, Direction::Left, width, &none).unwrap();
        assert!(layout.move_tile(Chat, target));
        assert_eq!(layout.columns.len(), 2);
        assert_eq!(layout.columns[1].tiles, [Chat, Record, Structure, Sql]);
        // From the last column, right starts a new column.
        assert_eq!(
            layout.step(Sql, Direction::Right, width, &none),
            Some(Target::NewColumn)
        );
        assert!(layout.move_tile(Sql, Target::NewColumn));
        assert_eq!(layout.columns.len(), 3);
        assert_eq!(layout.columns[2].tiles, [Sql]);
        // Reordering within a column.
        let down = layout.step(Chat, Direction::Down, width, &none).unwrap();
        assert!(layout.move_tile(Chat, down));
        assert_eq!(layout.columns[1].tiles, [Record, Chat, Structure]);
        // A new column is refused when the columns would no longer keep their minimums.
        let mut wide = Layout::default();
        for kind in [Sql, Structure, Record] {
            assert!(wide.fits_new_column(kind, width, &none));
            assert!(wide.move_tile(kind, Target::NewColumn));
        }
        assert_eq!(wide.columns.len(), 5);
        assert!(!wide.fits_new_column(Data, width, &none));
        assert_eq!(
            wide.step(Data, Direction::Right, 400.0, &none),
            Some(Target::Slot {
                column: 1,
                index: 1
            })
        );
        assert!(wide.fits_new_column(Data, 1400.0, &none));
    }
    #[test]
    fn any_column_folds_to_a_line_and_reopens_what_was_open() {
        let mut layout = Layout::default();
        let mut minimized = BTreeSet::from([Record, Chat]);
        assert_eq!(layout.folds(&minimized), [false, false, true]);
        layout.fold(1, &mut minimized);
        assert_eq!(layout.folds(&minimized), [false, true, true]);
        layout.unfold(1, &mut minimized);
        assert_eq!(minimized, BTreeSet::from([Record, Chat]));
        layout.fold(0, &mut minimized);
        layout.unfold(0, &mut minimized);
        assert_eq!(minimized, BTreeSet::from([Record, Chat]));
        // A tile that moves away is no longer reopened with its old column.
        layout.fold(1, &mut minimized);
        assert!(layout.move_tile(
            Sql,
            Target::Slot {
                column: 0,
                index: 2
            }
        ));
        assert_eq!(layout.columns[1].unfold, [Structure]);
        // With everything minimized nothing folds: the tiles stay as headers.
        minimized.extend(Kind::PANES);
        assert_eq!(layout.folds(&minimized), [false, false, false]);
    }
    #[test]
    fn a_dragged_tile_lands_in_the_slot_under_the_pointer_or_a_new_last_column() {
        let area = Rect::from_min_size(pos2(280.0, 8.0), vec2(1078.0, 718.0));
        let arrangement = Layout::default();
        let none = BTreeSet::new();
        let frame = layout(area, None, &none, &arrangement);
        let at = |pointer, kind| drop_target(pointer, kind, &frame, &arrangement, &none, area);
        let map = frame.rect(Map).unwrap();
        let data = frame.rect(Data).unwrap();
        let chat = frame.rect(Chat).unwrap();
        // Over another column: the slot nearest the pointer.
        assert_eq!(
            at(map.center(), Structure),
            Some(Target::Slot {
                column: 0,
                index: 1
            })
        );
        assert_eq!(
            at(pos2(map.center().x, map.top() + 4.0), Structure),
            Some(Target::Slot {
                column: 0,
                index: 0
            })
        );
        assert_eq!(
            at(pos2(data.center().x, data.bottom() - 4.0), Structure),
            Some(Target::Slot {
                column: 0,
                index: 2
            })
        );
        // Where it already is, nothing changes.
        assert_eq!(at(map.center(), Map), None);
        assert_eq!(at(data.left_top() + vec2(20.0, 20.0), Map), None);
        assert_eq!(
            at(pos2(data.center().x, data.bottom() - 4.0), Map),
            Some(Target::Slot {
                column: 0,
                index: 2
            })
        );
        // The gaps between columns belong to the nearer column, never to a new one.
        assert_eq!(
            at(pos2(map.right() + 3.0, map.center().y), Sql),
            Some(Target::Slot {
                column: 0,
                index: 1
            })
        );
        // Outside the workspace nothing happens.
        assert_eq!(at(pos2(area.left() - 30.0, 300.0), Sql), None);
        assert_eq!(at(pos2(map.center().x, area.bottom() + 30.0), Sql), None);
        // Past the last column a new one starts, unless the tile is already alone there.
        let edge = pos2(area.right() - 5.0, 300.0);
        assert_eq!(at(edge, Sql), Some(Target::NewColumn));
        assert_eq!(at(edge, Chat), None);
        assert_eq!(
            at(chat.center(), Sql),
            Some(Target::Slot {
                column: 2,
                index: 1
            })
        );
        let line = marker(Target::NewColumn, &frame, &arrangement, area);
        assert!(line.left() >= area.right() && line.height() == area.height());
        let slot = marker(
            Target::Slot {
                column: 0,
                index: 1,
            },
            &frame,
            &arrangement,
            area,
        );
        assert!(slot.top() >= map.bottom() && slot.bottom() <= data.top());
        assert_eq!(slot.x_range(), map.x_range());
        // A folded column takes the tile at its end.
        let minimized = BTreeSet::from([Record, Structure, Sql]);
        let frame = layout(area, None, &minimized, &arrangement);
        let line = frame.columns[1].rect;
        assert!(frame.columns[1].folded);
        let join = Target::Slot {
            column: 1,
            index: 3,
        };
        assert_eq!(
            drop_target(line.center(), Map, &frame, &arrangement, &minimized, area),
            Some(join)
        );
        assert_eq!(marker(join, &frame, &arrangement, area), line);
    }
}
