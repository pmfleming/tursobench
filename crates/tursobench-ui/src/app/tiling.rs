//! Fixed pane topology with shared, resizable column and row boundaries.
use eframe::egui::{Rect, Vec2, pos2};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub(super) const GAP: f32 = 8.0;
pub(super) const BAR: f32 = 44.0;
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
}
impl Kind {
    pub const PANES: [Self; 5] = [
        Self::Map,
        Self::Data,
        Self::Record,
        Self::Structure,
        Self::Sql,
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
        }
    }
}
const MIN_WIDTH: f32 = 180.0;
const MIN_HEIGHT: f32 = 96.0;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub(super) struct Splits {
    pub column: f32,
    pub rows: [f32; 5],
}
impl Default for Splits {
    fn default() -> Self {
        Self {
            column: 0.63,
            rows: [0.85, 1.35, 1.08, 1.03, 1.0],
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
    }
    pub fn resize(
        &mut self,
        divider: Divider,
        delta: Vec2,
        panes: &[(Kind, Rect)],
        minimized: &BTreeSet<Kind>,
    ) {
        match divider {
            Divider::Columns => {
                let total = panes[0].1.width() + panes[2].1.width();
                if total <= 0.0 {
                    return;
                }
                let min = MIN_WIDTH.min(total * 0.5);
                self.column = (panes[0].1.width() + delta.x).clamp(min, total - min) / total;
            }
            Divider::Rows(a, b) => {
                let total = panes[a].1.height() + panes[b].1.height();
                if total <= 0.0 {
                    return;
                }
                let min = MIN_HEIGHT.min(total * 0.5);
                let first = (panes[a].1.height() + delta.y).clamp(min, total - min);
                let column = if a < 2 { 0..2 } else { 2..5 };
                let active = column.filter(|&i| !minimized.contains(&panes[i].0));
                let height: f32 = active.clone().map(|i| panes[i].1.height()).sum();
                let weight: f32 = active.clone().map(|i| self.rows[i]).sum();
                for i in active {
                    let h = if i == a {
                        first
                    } else if i == b {
                        total - first
                    } else {
                        panes[i].1.height()
                    };
                    self.rows[i] = (h / height * weight).max(f32::EPSILON);
                }
            }
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Divider {
    Columns,
    Rows(usize, usize),
}
impl Divider {
    pub fn vertical(self) -> bool {
        self == Self::Columns
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Columns => "Resize workspace columns",
            Self::Rows(0, 1) => "Resize Relationships and Table data",
            Self::Rows(2, 3) => "Resize Record and Structure",
            _ => "Resize Structure and SQL",
        }
    }
}
pub(super) fn dividers(
    panes: &[(Kind, Rect)],
    minimized: &BTreeSet<Kind>,
    area: Rect,
) -> Vec<(Divider, Rect)> {
    if panes.len() != 5 {
        return vec![];
    }
    let left = panes[0].1;
    let right = panes[2].1;
    let mut handles = vec![(
        Divider::Columns,
        Rect::from_min_max(
            pos2(left.right(), left.top()),
            pos2(right.left(), area.bottom()),
        ),
    )];
    for (a, b) in [(0, 1), (2, 3), (3, 4)] {
        if minimized.contains(&panes[a].0) || minimized.contains(&panes[b].0) {
            continue;
        }
        let before = panes[a].1;
        let after = panes[b].1;
        handles.push((
            Divider::Rows(a, b),
            Rect::from_min_max(
                pos2(before.left(), before.bottom()),
                pos2(before.right(), after.top()),
            ),
        ));
    }
    handles
}
pub(super) fn layout(
    area: Rect,
    expanded: Option<Kind>,
    minimized: &BTreeSet<Kind>,
    splits: &Splits,
) -> Vec<(Kind, Rect)> {
    if let Some(kind) = expanded {
        return vec![(kind, area)];
    }
    let gap = GAP.min(area.width().max(0.0));
    let available = (area.width() - gap).max(0.0);
    let min = MIN_WIDTH.min(available * 0.5);
    let split = area.left() + (available * splits.column).clamp(min, available - min);
    let left = Rect::from_min_max(area.min, pos2(split, area.bottom()));
    let right = Rect::from_min_max(pos2(split + gap, area.top()), area.max);
    let mut tiles = Vec::with_capacity(5);
    stack(&mut tiles, left, 0..2, minimized, splits);
    stack(&mut tiles, right, 2..5, minimized, splits);
    tiles
}
fn stack(
    tiles: &mut Vec<(Kind, Rect)>,
    rect: Rect,
    indices: std::ops::Range<usize>,
    minimized: &BTreeSet<Kind>,
    splits: &Splits,
) {
    let count = indices.len();
    let gap = GAP.min(rect.height().max(0.0) / (count - 1) as f32);
    let collapsed = indices
        .clone()
        .filter(|&i| minimized.contains(&Kind::PANES[i]))
        .count();
    let available = (rect.height() - gap * (count - 1) as f32).max(0.0);
    let bar = BAR.min(available / count as f32);
    let mut remaining = (available - collapsed as f32 * bar).max(0.0);
    let mut expanded = count - collapsed;
    let min = MIN_HEIGHT.min(remaining / expanded.max(1) as f32);
    let mut weights: f32 = indices
        .clone()
        .filter(|&i| !minimized.contains(&Kind::PANES[i]))
        .map(|i| splits.rows[i])
        .sum();
    let mut y = rect.top();
    for i in indices {
        let kind = Kind::PANES[i];
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
                (remaining * (splits.rows[i] / weights)).clamp(floor, maximum)
            };
            remaining = (remaining - height).max(0.0);
            weights -= splits.rows[i];
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
#[cfg(test)]
mod tests {
    use super::{BAR, Divider, Kind, Splits, dividers, layout};
    use eframe::egui::{Pos2, Rect, vec2};
    use std::collections::BTreeSet;
    #[test]
    fn fixed_panes_minimize_expand_and_reclaim_space_without_overlap() {
        for size in [
            vec2(1.0, 1.0),
            vec2(40.0, 20.0),
            vec2(720.0, 480.0),
            vec2(1040.0, 640.0),
            vec2(3100.0, 1360.0),
        ] {
            let area = Rect::from_min_size(Pos2::ZERO, size);
            for splits in [
                Splits::default(),
                Splits {
                    column: 0.4,
                    rows: [1.0, 2.0, 3.0, 1.0, 2.0],
                },
                Splits {
                    column: 0.99,
                    rows: [1e-20, 1.0, 1e20, 0.2, 1.0],
                },
            ] {
                for mask in 0..32 {
                    let minimized: BTreeSet<_> = Kind::PANES
                        .into_iter()
                        .enumerate()
                        .filter_map(|(i, k)| (mask & (1 << i) != 0).then_some(k))
                        .collect();
                    let tiles = layout(area, None, &minimized, &splits);
                    assert_eq!(tiles.len(), 5);
                    let handles = dividers(&tiles, &minimized, area);
                    assert_eq!(handles[0].0, Divider::Columns);
                    assert_eq!(handles[0].1.height(), area.height());
                    assert!(tiles[0].1.bottom() < tiles[1].1.top());
                    for (i, (kind, rect)) in tiles.iter().enumerate() {
                        assert!(area.contains_rect(*rect));
                        if minimized.contains(kind) {
                            assert!(rect.height() <= BAR + 0.01);
                            if size.y >= 480.0 {
                                assert!((rect.height() - BAR).abs() < 0.01);
                            }
                        }
                        assert!(
                            tiles[i + 1..]
                                .iter()
                                .all(|(_, other)| !rect.intersects(*other))
                        );
                        assert_eq!(
                            layout(area, Some(*kind), &minimized, &splits),
                            vec![(*kind, area)]
                        );
                    }
                }
            }
        }
    }
}
