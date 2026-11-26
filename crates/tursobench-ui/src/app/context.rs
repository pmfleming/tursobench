use super::{
    App, View,
    icons::{self, Icon},
};
use eframe::egui;
use std::collections::VecDeque;
use tursobench_core::{Delivery, Event, EventKind, ForeignKey, ResultSet, Ticket, Value};
use tursobench_db::Operation;

#[derive(Default)]
pub(super) struct ContextReads {
    pub pending: Option<Read>,
    cache: VecDeque<Cached>,
    cache_bytes: usize,
    /// The grid cell under the pointer, when it holds exactly one reference.
    pub hovered: Option<(usize, usize)>,
    hover: Option<(String, f64)>,
    relation: Option<(String, i64)>,
}

pub(super) struct Read {
    pub result: ResultSet,
    owner: Option<Ticket>,
    key: String,
    error: String,
}
struct Cached {
    bytes: usize,
    key: String,
    result: ResultSet,
    error: String,
}
impl Cached {
    fn new(key: String, result: ResultSet, error: String) -> Self {
        let column = |c: &tursobench_core::Column| {
            c.name.len() + c.declared_type.as_ref().map_or(0, String::len)
        };
        let columns: usize = result.columns.iter().map(column).sum();
        let values: usize = result.rows.iter().flatten().map(Value::bytes).sum();
        Self {
            bytes: key.len() + error.len() + columns + values,
            key,
            result,
            error,
        }
    }
    /// The count a complete, successful count read returned.
    fn count(&self) -> Option<i64> {
        let done = self.result.completion.as_ref()?;
        if done.delivery != Delivery::Complete || done.error.is_some() {
            return None;
        }
        match self.result.rows.first()?.first()? {
            Value::Integer(n) => Some(*n),
            _ => None,
        }
    }
    /// The read stopped at the retention budget.
    fn limited(&self) -> bool {
        let done = self.result.completion.as_ref();
        done.is_some_and(|c| c.delivery != Delivery::Complete)
    }
}
/// One side of a key's column pairs, joined by `separator`.
fn join<'a>(
    key: &'a ForeignKey,
    side: impl Fn(&'a (String, Option<String>)) -> &'a str,
    separator: &str,
) -> String {
    key.columns
        .iter()
        .map(side)
        .collect::<Vec<_>>()
        .join(separator)
}
/// The link icon that starts the related-records line.
fn related_icon(ui: &mut egui::Ui) {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(14.0, 22.0), egui::Sense::hover());
    icons::paint(
        ui.painter(),
        egui::Rect::from_center_size(rect.center(), egui::vec2(13.0, 13.0)),
        Icon::Link,
        crate::appearance::Tokens::of(ui.ctx()).muted,
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, "Related records"));
    response.on_hover_text("Related records");
}
#[derive(Clone, Copy, Debug)]
pub(super) enum ReadKind {
    Parent,
    Rows,
    Count,
}

struct Relationship<'a> {
    id: String,
    child: &'a str,
    key: &'a ForeignKey,
    values: Vec<&'a Value>,
    kind: ReadKind,
}
impl Relationship<'_> {
    fn into_operation(self) -> (String, Operation) {
        let table = self.child.to_owned();
        let key = self.key.clone();
        let values = self.values.into_iter().cloned().collect();
        let operation = match self.kind {
            ReadKind::Parent => Operation::Peek { table, key, values },
            ReadKind::Rows => Operation::References { table, key, values },
            ReadKind::Count => Operation::ReferenceCount { table, key, values },
        };
        (self.id, operation)
    }
}
impl ContextReads {
    fn cached(&self, key: &str) -> Option<&Cached> {
        self.cache.iter().find(|c| c.key == key)
    }
    fn remember(&mut self, cached: Cached) {
        self.cache_bytes += cached.bytes;
        self.cache.push_back(cached);
        while self.cache.len() > 32 || self.cache_bytes > 8 * 1024 * 1024 {
            if let Some(old) = self.cache.pop_front() {
                self.cache_bytes -= old.bytes;
            }
        }
    }
    fn forget(&mut self, key: &str) {
        self.cache.retain(|c| {
            if c.key != key {
                return true;
            }
            self.cache_bytes -= c.bytes;
            false
        });
    }
}
impl App {
    fn relationship_read<'a>(
        &'a self,
        row: usize,
        child: &'a str,
        key: &'a ForeignKey,
        kind: ReadKind,
    ) -> Option<Relationship<'a>> {
        if self.database.document.view != View::Browse {
            return None;
        }
        let table = self.database.document.table.as_ref()?;
        let row = self.database.document.result.rows.get(row)?;
        let outgoing = matches!(kind, ReadKind::Parent);
        if outgoing && table.name != child
            || !outgoing && !table.name.eq_ignore_ascii_case(&key.target)
        {
            return None;
        }
        let values: Vec<_> = key
            .columns
            .iter()
            .map(|(source, target)| {
                let name = if outgoing {
                    source.as_str()
                } else {
                    target.as_deref()?
                };
                let i = table
                    .columns
                    .iter()
                    .position(|c| c.name.eq_ignore_ascii_case(name))?;
                row.get(i).filter(|v| **v != Value::Null)
            })
            .collect::<Option<_>>()?;
        if values.is_empty() || values.iter().map(|v| v.bytes()).sum::<usize>() > 32 * 1024 {
            return None;
        }
        let id = format!("{kind:?}:{child}:{key:?}:{values:?}");
        if id.len() > 128 * 1024 {
            return None;
        }
        Some(Relationship {
            id,
            child,
            key,
            values,
            kind,
        })
    }
    /// The cache key of a relationship read, when the row's key values allow one.
    fn read_id(&self, row: usize, child: &str, key: &ForeignKey, kind: ReadKind) -> Option<String> {
        self.relationship_read(row, child, key, kind)
            .map(|plan| plan.id)
    }
    pub(super) fn request_context(
        &mut self,
        row: usize,
        child: &str,
        key: &ForeignKey,
        kind: ReadKind,
    ) {
        let tools = &self.database.document.tools;
        if !self.requests_enabled()
            || self.database.document.active.is_some()
            || tools.context.pending.is_some()
            || tools.pages.pending.is_some()
        {
            return;
        }
        let Some(plan) = self.relationship_read(row, child, key, kind) else {
            return;
        };
        if tools.context.cached(&plan.id).is_some() {
            return;
        }
        let (id, operation) = plan.into_operation();
        if let Some(Ok(ticket)) = self.database.request(operation) {
            let mut result = ResultSet::default();
            result.start(ticket);
            self.database.document.tools.context.pending = Some(Read {
                result,
                owner: self.database.document.result.current,
                key: id,
                error: String::new(),
            });
        }
    }
    pub(super) fn context_event(&mut self, event: Event) {
        let context = &mut self.database.document.tools.context;
        let Some(read) = context.pending.as_mut() else {
            return;
        };
        match &event.kind {
            EventKind::Failed(e) => read.error = e.to_string(),
            EventKind::Finished(c) => {
                self.database.transaction = c.transaction;
                if let Some(e) = &c.error {
                    read.error = e.to_string();
                }
            }
            _ => {}
        }
        let done = matches!(event.kind, EventKind::Failed(_) | EventKind::Finished(_));
        read.result.apply(event);
        let Some(read) = context.pending.take_if(|_| done) else {
            return;
        };
        if read.owner == self.database.document.result.current && !self.database.closing {
            let cached = Cached::new(read.key, read.result, read.error);
            self.database.document.tools.context.remember(cached);
        }
    }
    /// After the pointer rests on a reference for a moment, read its parent for the preview.
    pub(super) fn hover_context(&mut self, ctx: &egui::Context) {
        let Some((row, col)) = self.database.document.tools.context.hovered else {
            self.database.document.tools.context.hover = None;
            return;
        };
        let Some((table, id)) = self.hovered_parent(row, col) else {
            return;
        };
        let context = &mut self.database.document.tools.context;
        let cached = context.cached(&id).is_some();
        let now = ctx.input(|i| i.time);
        let since = match &context.hover {
            Some((old, since)) if old == &id => *since,
            _ => {
                context.hover = Some((id, now));
                now
            }
        };
        if cached {
            return;
        }
        if now - since < 0.35 {
            ctx.request_repaint_after(std::time::Duration::from_millis(350));
        } else if let Some(&key) = self.cell_references(row, col).first() {
            let key = key.clone();
            self.request_context(row, &table, &key, ReadKind::Parent);
        }
    }
    /// The browsed table and the read of the parent the hovered cell refers to.
    fn hovered_parent(&self, row: usize, col: usize) -> Option<(String, String)> {
        let table = &self.database.document.table.as_ref()?.name;
        let id = match self.cell_references(row, col).as_slice() {
            [key] => self.read_id(row, table, key, ReadKind::Parent)?,
            _ => return None,
        };
        Some((table.clone(), id))
    }
    pub(super) fn peek_view(&self, ui: &mut egui::Ui, row: usize, key: &ForeignKey) {
        ui.strong(format!("Parent · {}", key.target));
        let Some(table) = &self.database.document.table else {
            return;
        };
        let Some(id) = self.read_id(row, &table.name, key, ReadKind::Parent) else {
            ui.label("Unresolved or NULL key");
            return;
        };
        let Some(c) = self.database.document.tools.context.cached(&id) else {
            ui.label("Loading parent preview…");
            return;
        };
        if !c.error.is_empty() {
            ui.label(&c.error);
            return;
        }
        match c.result.rows.first() {
            Some(parent) => {
                for (column, value) in c.result.columns.iter().zip(parent).take(12) {
                    ui.label(format!("{}  {}", column.name, value.preview()));
                }
            }
            None => {
                ui.label("No retained parent row");
            }
        }
        if c.limited() {
            ui.small("Preview limited by retention budget");
        }
    }
    pub(super) fn incoming_keys(&self) -> Vec<(String, ForeignKey)> {
        let Some(table) = &self.database.document.table else {
            return vec![];
        };
        self.database
            .ui
            .diagram
            .data
            .iter()
            .flat_map(|g| &g.tables)
            .flat_map(|n| {
                n.inspection
                    .iter()
                    .flat_map(|i| &i.foreign_keys)
                    .filter(|k| k.target.eq_ignore_ascii_case(&table.name))
                    .map(|k| (n.name.clone(), k.clone()))
            })
            .collect()
    }
    pub(super) fn related_visible(&self) -> bool {
        self.database.document.view == View::Browse
            && (self.schema_incomplete() || !self.incoming_keys().is_empty())
    }
    pub(super) fn schema_incomplete(&self) -> bool {
        self.database.ui.diagram.stale
            || self.database.ui.diagram.data.as_ref().is_none_or(|g| {
                g.limited
                    || g.tables
                        .iter()
                        .any(|n| n.error.is_some() || n.inspection.is_none())
            })
    }
    /// A parent link is usable only when every component has a resolved target and a retained non-NULL value.
    fn record_parent_ready(&self, row: usize, key: &ForeignKey) -> bool {
        let document = &self.database.document;
        let value = |source: &str| {
            let table = document.table.as_ref()?;
            let col = table
                .columns
                .iter()
                .position(|c| c.name.eq_ignore_ascii_case(source))?;
            document.result.rows.get(row)?.get(col)
        };
        !key.columns.is_empty()
            && key.columns.iter().all(|(source, target)| {
                target.is_some()
                    && value(source).is_some_and(|v| !matches!(v, Value::Null | Value::Deferred(_)))
            })
    }
    /// Relationship reads keep staged edits and loads in place, so none may be running.
    fn relationships_available(&self) -> bool {
        self.requests_enabled() && self.database.document.active.is_none()
    }
    pub(super) fn record_relationships(&mut self, ui: &mut egui::Ui, row: usize) {
        let available = self.relationships_available();
        let mut follow = None;
        if let Some(info) = self.database.document.browse_inspection()
            && !info.foreign_keys.is_empty()
        {
            ui.weak("References · records this row points to");
            for key in &info.foreign_keys {
                if self.parent_link(ui, row, key, available) {
                    follow = Some(key.clone());
                }
            }
        }
        if let Some(key) = follow {
            self.follow_key(row, key);
        }
        if self.related_visible() {
            ui.weak("Referenced by · rows that point to this record");
            self.related_list(ui, row);
            if !self.incoming_keys().is_empty() {
                egui::CollapsingHeader::new("Preview related rows")
                    .id_salt("record-related-preview")
                    .default_open(false)
                    .show(ui, |ui| self.wide_related(ui));
            }
        }
    }
    /// Relationships as one 24-point line, its parent and referring counts
    /// beside it, that opens to the record's references below.
    pub(super) fn relationships_line(&mut self, ui: &mut egui::Ui, row: usize, height: f32) {
        let id = ui.make_persistent_id("record-relationships-section");
        let mut state =
            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 24.0), egui::Sense::click());
        let parents = self
            .database
            .document
            .browse_inspection()
            .map_or(0, |info| info.foreign_keys.len());
        let referring = self.incoming_keys().len();
        let plural = if parents == 1 { "" } else { "s" };
        let counts = format!("{parents} parent{plural} · {referring} referring");
        let open = state.is_open();
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::Role::Button, true, open, "Relationships")
        });
        if response.clicked() {
            state.toggle(ui);
        }
        let marker = egui::Rect::from_center_size(
            rect.left_center() + egui::vec2(8.0, 0.0),
            egui::Vec2::splat(9.0),
        );
        let marker = ui.interact(marker, id.with("marker"), egui::Sense::hover());
        let openness = state.openness(ui.ctx());
        egui::collapsing_header::paint_default_icon(ui, openness, &marker);
        let tokens = crate::appearance::Tokens::of(ui.ctx());
        let label = egui::Rect::from_min_max(rect.min + egui::vec2(20.0, 0.0), rect.max);
        super::presentation::text_at(ui, label, "Relationships", ui.visuals().text_color(), 12.5);
        let after = super::presentation::text_width(ui, "Relationships", 12.5) + 30.0;
        let link = label.left_center() + egui::vec2(after, 0.0);
        super::presentation::icon_at(ui.painter(), link, 12.0, Icon::Link, tokens.faint);
        // Narrow tiles keep the numbers and name them on hover.
        let rest = egui::Rect::from_min_max(link + egui::vec2(10.0, -12.0), rect.max);
        let short = format!("{parents} · {referring}");
        let wide = super::presentation::text_width(ui, counts.as_str(), 12.0) <= rest.width();
        let shown = if wide { &counts } else { &short };
        super::presentation::text_at(ui, rest, shown, tokens.faint, 12.0);
        response.on_hover_text(counts);
        state.show_body_unindented(ui, |ui| {
            ui.set_min_height(height);
            egui::ScrollArea::vertical()
                .id_salt("record-relationships-scroll")
                .max_height(height)
                .auto_shrink([false, true])
                .show(ui, |ui| self.record_relationships(ui, row));
        });
    }
    /// A link to the record `key` points to, then its column mapping; returns
    /// whether the link was clicked.
    fn parent_link(
        &self,
        ui: &mut egui::Ui,
        row: usize,
        key: &ForeignKey,
        available: bool,
    ) -> bool {
        let target = |target: &Option<String>| {
            let column = target.as_deref().unwrap_or("unresolved PK");
            format!("{}.{column}", key.target)
        };
        let mapping = key
            .columns
            .iter()
            .map(|(source, to)| format!("{source} → {}", target(to)))
            .collect::<Vec<_>>()
            .join(", ");
        let color = crate::appearance::Tokens::of(ui.ctx()).relationship;
        let text = egui::RichText::new(format!("Follow {}", key.target)).color(color);
        let enabled = available && self.record_parent_ready(row, key);
        ui.horizontal_wrapped(|ui| {
            let clicked = ui
                .add_enabled(enabled, egui::Button::new(text).frame(false))
                .on_hover_text("Open the referenced record; requires resolved non-NULL keys and no staged edits")
                .clicked();
            ui.label(mapping);
            clicked
        })
        .inner
    }
    pub(super) fn related_list(&mut self, ui: &mut egui::Ui, row: usize) {
        if !self.related_visible() {
            return;
        }
        let keys = self.incoming_keys();
        let available = self.relationships_available()
            && self.database.document.tools.context.pending.is_none();
        egui::ScrollArea::horizontal()
            .id_salt("related-line")
            .max_height(24.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().interact_size.y = 22.0;
                    related_icon(ui);
                    if self.schema_incomplete() {
                        ui.colored_label(ui.visuals().warn_fg_color, "Incomplete schema")
                            .on_hover_text(
                                "Cached or incomplete schema — references may be missing",
                            );
                    }
                    for (child, key) in keys {
                        self.related_key(ui, row, &child, &key, available);
                    }
                });
            });
    }
    /// One incoming key: a link to the related rows with their count once
    /// counted, and a button that counts them.
    fn related_key(
        &mut self,
        ui: &mut egui::Ui,
        row: usize,
        child: &str,
        key: &ForeignKey,
        available: bool,
    ) {
        let spec = self.read_id(row, child, key, ReadKind::Count);
        let cached = spec
            .as_ref()
            .and_then(|id| self.database.document.tools.context.cached(id));
        let count = cached.and_then(Cached::count);
        let hint = match cached.map(|c| c.error.as_str()) {
            Some(error) if !error.is_empty() => format!("{error} · click to retry count"),
            _ => "Count matching rows (full scan, may be slow)".to_owned(),
        };
        let enabled = available && spec.is_some();
        let count_text = count.map_or("—".into(), |n| n.to_string());
        let label = format!(
            "{child}.{} → {} · {count_text}",
            join(key, |(from, _)| from.as_str(), " + "),
            join(
                key,
                |(_, to)| to.as_deref().unwrap_or("unresolved PK"),
                " + "
            ),
        );
        let tooltip = format!(
            "Related {child} · FK {} · {}",
            key.id,
            key.columns
                .iter()
                .map(|(from, to)| format!("{from} → {}", to.as_deref().unwrap_or("unresolved PK")))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let color = crate::appearance::Tokens::of(ui.ctx()).relationship;
        let link = egui::Button::new(egui::RichText::new(label).color(color)).frame(false);
        if ui
            .add_enabled(enabled, link)
            .on_hover_text(tooltip)
            .clicked()
        {
            self.follow_incoming(row, child.to_owned(), key.clone());
        }
        if count.is_some() {
            return;
        }
        let name = format!("Count related {child} (FK {})", key.id);
        if ui
            .add_enabled_ui(enabled, |ui| icons::button(ui, Icon::Count, &name, ""))
            .inner
            .on_hover_text(hint)
            .clicked()
        {
            if let Some(id) = &spec {
                self.database.document.tools.context.forget(id);
            }
            self.request_context(row, child, key, ReadKind::Count);
        }
    }
    pub(super) fn wide_related(&mut self, ui: &mut egui::Ui) {
        let document = &self.database.document;
        let Some(row) = document.inspect.map(|(r, _)| r).or(document.tools.detail) else {
            return;
        };
        let keys = self.incoming_keys();
        if keys.is_empty() {
            return;
        }
        let (child, key) = self.choose_relation(ui, &keys);
        if ui.link("Open related rows in full view").clicked() {
            self.follow_incoming(row, child.clone(), key.clone());
        } else {
            self.related_rows(ui, row, child, key);
        }
    }
    /// The incoming keys to choose from; the chosen one, or else the first, is remembered.
    fn choose_relation<'k>(
        &mut self,
        ui: &mut egui::Ui,
        keys: &'k [(String, ForeignKey)],
    ) -> &'k (String, ForeignKey) {
        let relation = &mut self.database.document.tools.context.relation;
        let chosen = |relation: &Option<(String, i64)>, (child, key): &(String, ForeignKey)| {
            relation
                .as_ref()
                .is_some_and(|(name, id)| name == child && *id == key.id)
        };
        ui.horizontal_wrapped(|ui| {
            ui.strong("Related rows");
            for incoming @ (child, key) in keys {
                let label = format!("{child} · FK {}", key.id);
                if ui
                    .selectable_label(chosen(relation, incoming), label)
                    .clicked()
                {
                    *relation = Some((child.clone(), key.id));
                }
            }
        });
        let selected = keys.iter().find(|k| chosen(relation, k));
        selected.unwrap_or_else(|| {
            *relation = Some((keys[0].0.clone(), keys[0].1.id));
            &keys[0]
        })
    }
    /// The related rows through the chosen key, read once and then cached.
    fn related_rows(&mut self, ui: &mut egui::Ui, row: usize, child: &str, key: &ForeignKey) {
        self.request_context(row, child, key, ReadKind::Rows);
        let Some(id) = self.read_id(row, child, key, ReadKind::Rows) else {
            ui.weak("Select a resolved non-NULL parent key");
            return;
        };
        let Some(c) = self.database.document.tools.context.cached(&id) else {
            ui.spinner();
            ui.weak("Loading related rows…");
            return;
        };
        if !c.error.is_empty() {
            ui.colored_label(ui.visuals().error_fg_color, &c.error);
            if ui.button("Retry related rows").clicked() {
                self.database.document.tools.context.forget(&id);
                self.request_context(row, child, key, ReadKind::Rows);
            }
            return;
        }
        ui.small(format!(
            "{} retained related rows · lookup capped at 100",
            c.result.rows.len()
        ));
        if c.limited() {
            ui.weak("Related result limited by retention budget");
        }
        related_grid(ui, &c.result);
    }
}
/// The first 12 columns of related rows, under a header row.
fn related_grid(ui: &mut egui::Ui, result: &ResultSet) {
    let rows = result.rows.len() + 1;
    egui::ScrollArea::both()
        .id_salt("related-grid")
        .show_rows(ui, 22.0, rows, |ui, range| {
            for r in range {
                ui.horizontal(|ui| related_row(ui, result, r));
            }
        });
    if result.columns.len() > 12 {
        ui.weak("Showing first 12 columns; open full view for all columns");
    }
}

/// Row `r` of the related grid: the column names, then each row's first 12 values.
fn related_row(ui: &mut egui::Ui, result: &ResultSet, r: usize) {
    let cell = |ui: &mut egui::Ui, text: egui::WidgetText| {
        ui.add_sized([140.0, 22.0], egui::Label::new(text).truncate())
    };
    let Some(r) = r.checked_sub(1) else {
        for col in result.columns.iter().take(12) {
            cell(ui, egui::RichText::new(&col.name).strong().into());
        }
        return;
    };
    for v in result.rows[r].iter().take(12) {
        cell(ui, v.preview().into()).on_hover_text(crate::export::cell_text(v));
    }
}
#[cfg(test)]
mod tests {
    use super::{Cached, ContextReads, ReadKind, View};
    use crate::app::{App, test_ui};
    use eframe::egui;
    use tursobench_core::{Column, ForeignKey, ResultSet, Table, TableColumn, Value};

    #[test]
    fn cache_enforces_fifo_count_and_byte_limits_and_accounts_for_retries() {
        let mut reads = ContextReads::default();
        for i in 0..33 {
            let empty = Cached::new(i.to_string(), ResultSet::default(), String::new());
            reads.remember(empty);
        }
        assert_eq!(reads.cache.len(), 32);
        assert!(reads.cached("0").is_none());
        assert!(reads.cached("32").is_some());
        let bytes = reads.cache_bytes;
        reads.forget("13");
        reads.forget("absent");
        assert_eq!(reads.cache_bytes, bytes - 2);
        assert_eq!(reads.cache.len(), 31);

        let mut reads = ContextReads::default();
        for i in 0..3 {
            let result = ResultSet {
                rows: vec![vec![Value::Blob(vec![0; 3 * 1024 * 1024])]],
                ..Default::default()
            };
            reads.remember(Cached::new(i.to_string(), result, String::new()));
        }
        assert_eq!(reads.cache.len(), 2);
        assert!(reads.cached("0").is_none());
        assert!(reads.cached("1").is_some());
        assert!(reads.cache_bytes <= 8 * 1024 * 1024);
        let error = "x".repeat(8 * 1024 * 1024 + 1);
        reads.remember(Cached::new("oversized".into(), ResultSet::default(), error));
        assert!(reads.cache.is_empty());
        assert_eq!(reads.cache_bytes, 0);
    }
    #[test]
    fn parent_preview_reports_unresolved_keys_loading_failures_and_the_cached_row() {
        let (ctx, mut app) = test_ui::app();
        let document = &mut app.database.document;
        document.view = View::Browse;
        document.table = Some(Table {
            name: "child".into(),
            columns: vec![TableColumn {
                name: "parent_id".into(),
                declared_type: "INTEGER".into(),
                not_null: false,
                primary_key: 0,
                hidden: 0,
                default_sql: None,
            }],
            is_table: true,
            has_rowid: true,
            binary_collations: true,
        });
        document.result.rows = vec![vec![Value::Null], vec![Value::Integer(7)]];
        let key = ForeignKey {
            id: 0,
            target: "parent".into(),
            columns: vec![("parent_id".into(), Some("id".into()))],
            on_update: String::new(),
            on_delete: String::new(),
        };
        let peek = |app: &App, row| {
            let size = egui::vec2(300.0, 200.0);
            let output = test_ui::render(&ctx, size, vec![], |ui| app.peek_view(ui, row, &key));
            let texts = test_ui::texts(&output).map(|t| t.galley.text().to_owned());
            texts.skip(1).collect::<Vec<_>>()
        };
        assert_eq!(peek(&app, 0), ["Unresolved or NULL key"]);
        assert_eq!(peek(&app, 1), ["Loading parent preview…"]);
        let id = app.read_id(1, "child", &key, ReadKind::Parent).unwrap();
        let parent = ResultSet {
            columns: vec![Column {
                name: "id".into(),
                declared_type: None,
            }],
            rows: vec![vec![Value::Integer(7)]],
            ..Default::default()
        };
        for (result, error, shown) in [
            (parent, "", "id  7"),
            (ResultSet::default(), "", "No retained parent row"),
            (ResultSet::default(), "lookup failed", "lookup failed"),
        ] {
            let context = &mut app.database.document.tools.context;
            context.forget(&id);
            context.remember(Cached::new(id.clone(), result, error.into()));
            assert_eq!(peek(&app, 1), [shown]);
        }
    }
}
