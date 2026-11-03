//! Fixed pane geometry. Only minimize and workspace expansion affect placement.
use eframe::egui::{Rect, pos2};
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
pub(super) fn layout(
    area: Rect,
    expanded: Option<Kind>,
    minimized: &BTreeSet<Kind>,
) -> Vec<(Kind, Rect)> {
    if let Some(kind) = expanded {
        return vec![(kind, area)];
    }
    let split = area.left() + (area.width() - GAP) * 0.63;
    let left = Rect::from_min_max(area.min, pos2(split, area.bottom()));
    let right = Rect::from_min_max(pos2(split + GAP, area.top()), area.max);
    let mut tiles = Vec::with_capacity(5);
    for (rect, panes) in [
        (left, &[(Kind::Map, 0.85), (Kind::Data, 1.35)][..]),
        (
            right,
            &[
                (Kind::Record, 1.08),
                (Kind::Structure, 1.03),
                (Kind::Sql, 1.0),
            ][..],
        ),
    ] {
        let collapsed = panes
            .iter()
            .filter(|(kind, _)| minimized.contains(kind))
            .count();
        let gaps = GAP * (panes.len() - 1) as f32;
        let bar = BAR.min(((rect.height() - gaps) / panes.len() as f32).max(0.0));
        let remaining = (rect.height() - gaps - collapsed as f32 * bar).max(0.0);
        let weights: f32 = panes
            .iter()
            .filter(|(kind, _)| !minimized.contains(kind))
            .map(|(_, weight)| weight)
            .sum();
        let mut y = rect.top();
        for &(kind, weight) in panes {
            let height = if minimized.contains(&kind) {
                bar
            } else {
                remaining * weight / weights
            };
            tiles.push((
                kind,
                Rect::from_min_max(
                    pos2(rect.left(), y),
                    pos2(rect.right(), (y + height).min(rect.bottom())),
                ),
            ));
            y += height + GAP;
        }
    }
    tiles
}
#[cfg(test)]
mod tests {
    use super::{BAR, Kind, layout};
    use eframe::egui::{Pos2, Rect, vec2};
    use std::collections::BTreeSet;
    #[test]
    fn fixed_panes_minimize_expand_and_reclaim_space_without_overlap() {
        for size in [
            vec2(720.0, 480.0),
            vec2(1040.0, 640.0),
            vec2(3100.0, 1360.0),
        ] {
            let area = Rect::from_min_size(Pos2::ZERO, size);
            for mask in 0..32 {
                let minimized: BTreeSet<_> = Kind::PANES
                    .into_iter()
                    .enumerate()
                    .filter_map(|(i, k)| (mask & (1 << i) != 0).then_some(k))
                    .collect();
                let tiles = layout(area, None, &minimized);
                assert_eq!(tiles.len(), 5);
                assert!(tiles[0].1.bottom() < tiles[1].1.top());
                for (i, (kind, rect)) in tiles.iter().enumerate() {
                    assert!(area.contains_rect(*rect));
                    if minimized.contains(kind) {
                        assert!((rect.height() - BAR).abs() < 0.01);
                    }
                    assert!(
                        tiles[i + 1..]
                            .iter()
                            .all(|(_, other)| !rect.intersects(*other))
                    );
                    assert_eq!(layout(area, Some(*kind), &minimized), vec![(*kind, area)]);
                }
            }
        }
    }
}
