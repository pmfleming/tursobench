//! Deterministic split geometry. A small viewport projects branches into tabs without
//! changing the saved tree, so growing the viewport restores the user's arrangement.
use eframe::egui::{self, Rect, Vec2};
use serde::{Deserialize, Serialize};

pub(super) const GAP: f32 = 7.0;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
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
    pub fn minimum(self) -> Vec2 {
        match self {
            Self::Data | Self::Result => egui::vec2(420.0, 200.0),
            Self::Structure => egui::vec2(260.0, 180.0),
            Self::Map => egui::vec2(320.0, 220.0),
            Self::Sql => egui::vec2(460.0, 200.0),
            Self::Record => egui::vec2(280.0, 180.0),
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Data => "Data",
            Self::Structure => "Structure",
            Self::Map => "Map",
            Self::Sql => "SQL",
            Self::Result => "Result",
            Self::Record => "Record",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) enum Node {
    Tabs {
        ids: Vec<u64>,
        selected: u64,
    },
    Split {
        horizontal: bool,
        ratio: f32,
        a: Box<Node>,
        b: Box<Node>,
    },
}
#[derive(Debug)]
pub(super) struct Tile {
    pub ids: Vec<u64>,
    pub selected: u64,
    pub rect: Rect,
}
pub(super) struct Divider {
    pub path: Vec<bool>,
    pub rect: Rect,
    pub parent: Rect,
    pub horizontal: bool,
    pub ratio: f32,
    pub range: std::ops::RangeInclusive<f32>,
}
impl Node {
    pub fn leaf(id: u64) -> Self {
        Self::Tabs {
            ids: vec![id],
            selected: id,
        }
    }
    pub fn ids(&self) -> Vec<u64> {
        match self {
            Self::Tabs { ids, .. } => ids.clone(),
            Self::Split { a, b, .. } => {
                let mut ids = a.ids();
                ids.extend(b.ids());
                ids
            }
        }
    }
    pub fn select(&mut self, id: u64) {
        match self {
            Self::Tabs { ids, selected } if ids.contains(&id) => *selected = id,
            Self::Split { a, b, .. } => {
                a.select(id);
                b.select(id);
            }
            _ => {}
        }
    }
    pub fn minimum(&self, profile: &impl Fn(u64) -> Vec2) -> Vec2 {
        match self {
            Self::Tabs { ids, .. } => {
                ids.iter()
                    .map(|id| profile(*id))
                    .fold(Vec2::ZERO, |a, b| a.max(b))
                    + egui::vec2(0.0, if ids.len() > 1 { 30.0 } else { 0.0 })
            }
            Self::Split {
                horizontal, a, b, ..
            } => {
                let (a, b) = (a.minimum(profile), b.minimum(profile));
                if *horizontal {
                    egui::vec2(a.x + b.x + GAP, a.y.max(b.y))
                } else {
                    egui::vec2(a.x.max(b.x), a.y + b.y + GAP)
                }
            }
        }
    }
    pub(super) fn layout(
        &self,
        area: Rect,
        focused: u64,
        profile: &impl Fn(u64) -> Vec2,
    ) -> (Vec<Tile>, Vec<Divider>) {
        let mut tiles = vec![];
        let mut dividers = vec![];
        self.place(
            area,
            focused,
            profile,
            &mut vec![],
            &mut tiles,
            &mut dividers,
        );
        (tiles, dividers)
    }
    fn place(
        &self,
        area: Rect,
        focused: u64,
        profile: &impl Fn(u64) -> Vec2,
        path: &mut Vec<bool>,
        tiles: &mut Vec<Tile>,
        dividers: &mut Vec<Divider>,
    ) {
        match self {
            Self::Tabs { ids, selected } => tiles.push(Tile {
                ids: ids.clone(),
                selected: *selected,
                rect: area,
            }),
            Self::Split {
                horizontal,
                ratio,
                a,
                b,
            } => {
                let minimum = self.minimum(profile);
                if minimum.x > area.width() || minimum.y > area.height() {
                    let ids = self.ids();
                    let selected = if ids.contains(&focused) {
                        focused
                    } else {
                        a.selected()
                    };
                    tiles.push(Tile {
                        ids,
                        selected,
                        rect: area,
                    });
                    return;
                }
                let axis = usize::from(!horizontal);
                let available = area.size()[axis] - GAP;
                let lower = a.minimum(profile)[axis] / available;
                let range = lower..=(1.0 - b.minimum(profile)[axis] / available).max(lower);
                let ratio = ratio.clamp(*range.start(), *range.end());
                let mut ra = area;
                ra.max[axis] = area.min[axis] + available * ratio;
                let mut rb = area;
                rb.min[axis] = ra.max[axis] + GAP;
                let mut divider = area;
                divider.min[axis] = ra.max[axis];
                divider.max[axis] = rb.min[axis];
                dividers.push(Divider {
                    path: path.clone(),
                    rect: divider,
                    parent: area,
                    horizontal: *horizontal,
                    ratio,
                    range,
                });
                path.push(false);
                a.place(ra, focused, profile, path, tiles, dividers);
                path.pop();
                path.push(true);
                b.place(rb, focused, profile, path, tiles, dividers);
                path.pop();
            }
        }
    }
    fn selected(&self) -> u64 {
        match self {
            Self::Tabs { selected, .. } => *selected,
            Self::Split { a, .. } => a.selected(),
        }
    }
    pub fn resize(&mut self, path: &[bool], ratio: f32) {
        if let Self::Split {
            a,
            b,
            ratio: current,
            ..
        } = self
        {
            if let Some((first, rest)) = path.split_first() {
                if *first {
                    b.resize(rest, ratio);
                } else {
                    a.resize(rest, ratio);
                }
            } else {
                *current = ratio;
            }
        }
    }
    pub fn remove(self, id: u64) -> Option<Self> {
        match self {
            Self::Tabs { mut ids, selected } => {
                ids.retain(|x| *x != id);
                if ids.is_empty() {
                    None
                } else {
                    Some(Self::Tabs {
                        selected: if ids.contains(&selected) {
                            selected
                        } else {
                            ids[0]
                        },
                        ids,
                    })
                }
            }
            Self::Split {
                horizontal,
                ratio,
                a,
                b,
            } => match (a.remove(id), b.remove(id)) {
                (Some(a), Some(b)) => Some(Self::Split {
                    horizontal,
                    ratio,
                    a: Box::new(a),
                    b: Box::new(b),
                }),
                (a, b) => a.or(b),
            },
        }
    }
    pub fn insert(
        &mut self,
        id: u64,
        focused: u64,
        area: Rect,
        profile: &impl Fn(u64) -> Vec2,
        preferred_width: &impl Fn(u64) -> f32,
    ) {
        // Score both existing and incoming content, including wrapping a whole
        // subtree. A wide incoming table must not inherit a narrow empty profile.
        let mut paths = vec![];
        self.paths(&mut vec![], &mut paths);
        let fresh = profile(id);
        let mut candidate: Option<(f32, Self)> = None;
        for path in paths {
            let Some((subtree, rect)) = self.region(&path, area, profile) else {
                continue;
            };
            let old = subtree.minimum(profile);
            for horizontal in [true, false] {
                let axis = usize::from(!horizontal);
                let available = rect.size()[axis] - GAP;
                if available < old[axis] + fresh[axis]
                    || rect.size()[1 - axis] < old[1 - axis].max(fresh[1 - axis])
                {
                    continue;
                }
                let wanted = if horizontal {
                    let old_width = subtree.preferred_width(preferred_width);
                    old_width / (old_width + preferred_width(id))
                } else {
                    (subtree.ids().len() as f32 / (subtree.ids().len() + 1) as f32).clamp(0.4, 0.7)
                };
                let lower = old[axis] / available;
                let ratio = wanted.clamp(lower, (1.0 - fresh[axis] / available).max(lower));
                let mut trial = self.clone();
                trial.split_at(&path, id, horizontal, ratio, false);
                let (tiles, _) = trial.layout(area, id, profile);
                let mut score: f32 = tiles
                    .iter()
                    .map(|tile| {
                        let coverage =
                            (tile.rect.width() / preferred_width(tile.selected).max(1.0)).min(1.0);
                        let rows = ((tile.rect.height() - 44.0).max(1.0)
                            / (area.height() - 44.0).max(1.0))
                        .min(1.0);
                        // Losing columns costs more than losing some rows. When both
                        // layouts fit columns, prefer more vertical room for data.
                        coverage.powi(3) * rows.powf(0.35)
                    })
                    .sum::<f32>()
                    / (self.ids().len() + 1) as f32;
                if matches!(subtree, Self::Tabs { ids, .. } if ids.contains(&focused)) {
                    score += 0.02;
                }
                if horizontal == (rect.width() > rect.height()) {
                    score += 0.005;
                }
                if candidate.as_ref().is_none_or(|c| score > c.0) {
                    candidate = Some((score, trial));
                }
            }
        }
        if let Some((_, tree)) = candidate {
            *self = tree;
        } else {
            self.group(focused, id);
        }
    }
    fn paths(&self, path: &mut Vec<bool>, paths: &mut Vec<Vec<bool>>) {
        paths.push(path.clone());
        if let Self::Split { a, b, .. } = self {
            path.push(false);
            a.paths(path, paths);
            path.pop();
            path.push(true);
            b.paths(path, paths);
            path.pop();
        }
    }
    fn region<'a>(
        &'a self,
        path: &[bool],
        area: Rect,
        profile: &impl Fn(u64) -> Vec2,
    ) -> Option<(&'a Self, Rect)> {
        let Some((right, rest)) = path.split_first() else {
            return Some((self, area));
        };
        let Self::Split {
            horizontal,
            ratio,
            a,
            b,
        } = self
        else {
            return None;
        };
        let minimum = self.minimum(profile);
        if minimum.x > area.width() || minimum.y > area.height() {
            return None;
        }
        let axis = usize::from(!horizontal);
        let available = area.size()[axis] - GAP;
        let lower = a.minimum(profile)[axis] / available;
        let ratio = ratio.clamp(
            lower,
            (1.0 - b.minimum(profile)[axis] / available).max(lower),
        );
        let mut rect = area;
        if *right {
            rect.min[axis] += available * ratio + GAP;
            b.region(rest, rect, profile)
        } else {
            rect.max[axis] = area.min[axis] + available * ratio;
            a.region(rest, rect, profile)
        }
    }
    fn preferred_width(&self, width: &impl Fn(u64) -> f32) -> f32 {
        match self {
            Self::Tabs { ids, .. } => ids.iter().map(|id| width(*id)).fold(0.0, f32::max),
            Self::Split {
                horizontal, a, b, ..
            } => {
                let (a, b) = (a.preferred_width(width), b.preferred_width(width));
                if *horizontal { a + b + GAP } else { a.max(b) }
            }
        }
    }
    fn split_at(&mut self, path: &[bool], id: u64, horizontal: bool, ratio: f32, before: bool) {
        if let Some((right, rest)) = path.split_first() {
            if let Self::Split { a, b, .. } = self {
                if *right {
                    b.split_at(rest, id, horizontal, ratio, before);
                } else {
                    a.split_at(rest, id, horizontal, ratio, before);
                }
            }
        } else {
            let old = std::mem::replace(self, Self::leaf(id));
            let (a, b) = if before {
                (Self::leaf(id), old)
            } else {
                (old, Self::leaf(id))
            };
            *self = Self::Split {
                horizontal,
                ratio,
                a: Box::new(a),
                b: Box::new(b),
            };
        }
    }
    fn group(&mut self, target: u64, id: u64) {
        match self {
            Self::Tabs { ids, selected } if ids.contains(&target) => {
                ids.push(id);
                *selected = id;
            }
            Self::Split { a, b, .. } => {
                a.group(target, id);
                b.group(target, id);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn incoming_content_selects_rows_or_columns_and_can_wrap_a_group() {
        let area = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1500.0, 800.0));
        let minimum = |_| Kind::Data.minimum();
        let mut narrow = Node::leaf(1);
        narrow.insert(2, 1, area, &minimum, &|_| 500.0);
        assert!(matches!(
            narrow,
            Node::Split {
                horizontal: true,
                ..
            }
        ));
        let mut wide = Node::leaf(1);
        wide.insert(2, 1, area, &minimum, &|id| {
            if id == 2 { 1400.0 } else { 500.0 }
        });
        assert!(matches!(
            wide,
            Node::Split {
                horizontal: false,
                ..
            }
        ));
        narrow.insert(3, 2, area, &minimum, &|id| {
            if id == 3 { 1400.0 } else { 500.0 }
        });
        let tiles = narrow.layout(area, 3, &minimum).0;
        let incoming = tiles.iter().find(|t| t.selected == 3).unwrap();
        assert_eq!(
            incoming.rect.width(),
            area.width(),
            "wide view should span below the whole group"
        );
        for tile in tiles.iter().filter(|t| t.selected != 3) {
            assert!(tile.rect.bottom() < incoming.rect.top());
        }
        let short = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1500.0, 350.0));
        let mut constrained = Node::leaf(1);
        constrained.insert(2, 1, short, &minimum, &|_| 1400.0);
        assert!(matches!(
            constrained,
            Node::Split {
                horizontal: true,
                ..
            }
        ));
    }
    #[test]
    fn split_resize_close_and_projected_groups_preserve_every_view() {
        let area = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1500.0, 800.0));
        let profile = |id| {
            if id == 2 {
                Kind::Structure.minimum()
            } else {
                Kind::Data.minimum()
            }
        };
        let mut tree = Node::leaf(1);
        for id in 2..=8 {
            tree.insert(id, id - 1, area, &profile, &|_| 500.0);
        }
        let saved = tree.clone();
        for size in [
            egui::vec2(420.0, 300.0),
            egui::vec2(1100.0, 650.0),
            area.size(),
        ] {
            let bounds = Rect::from_min_size(egui::Pos2::ZERO, size);
            let (tiles, dividers) = tree.layout(bounds, 8, &profile);
            let mut ids: Vec<_> = tiles.iter().flat_map(|t| t.ids.clone()).collect();
            ids.sort();
            assert_eq!(ids, (1..=8).collect::<Vec<_>>());
            for (i, tile) in tiles.iter().enumerate() {
                assert!(bounds.contains_rect(tile.rect));
                for other in &tiles[i + 1..] {
                    assert!(!tile.rect.intersects(other.rect));
                }
            }
            assert!(dividers.iter().all(|d| d.range.contains(&d.ratio)));
        }
        assert_eq!(
            tree, saved,
            "small displays must not destroy the saved layout"
        );
        let (before, dividers) = tree.layout(area, 8, &profile);
        if let Some(d) = dividers.first() {
            tree.resize(&d.path, *d.range.end());
            assert_ne!(tree.layout(area, 8, &profile).0[0].rect, before[0].rect);
        }
        for id in 1..8 {
            tree = tree.remove(id).unwrap();
        }
        assert_eq!(tree.ids(), vec![8]);
        assert!(tree.remove(8).is_none());
    }
}
