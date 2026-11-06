pub(super) mod presentation;
use super::{
    App, View,
    icons::{self, Icon},
    shell, tools,
};
use eframe::egui;
use tursobench_core::{Event, EventKind, SchemaObject};
use tursobench_core::{ForeignKey, Value};
use tursobench_db::{Operation, Request};

use tools::{Destination, NavigationEntry};
impl App {
    pub(super) fn finish_count(&mut self, failure: Option<String>) {
        let Some(tools::CountRequest {
            name,
            result,
            owner,
        }) = self.database.tools.count_request.take()
        else {
            return;
        };
        if result
            .completion
            .as_ref()
            .is_some_and(|c| c.error.is_none() && c.delivery == tursobench_core::Delivery::Complete)
            && let Some(Value::Integer(n)) = result.rows.first().and_then(|r| r.first())
        {
            self.database.status = format!("{name}: {n} rows · explicit full scan");
            if let Some((id, ticket)) = owner {
                let document = if self.database.document.id == id {
                    Some(&mut self.database.document)
                } else {
                    self.database.documents.get_mut(&id)
                };
                if let Some(document) = document
                    && document.result.current == ticket
                    && !document.stale
                {
                    document.tools.filtered_count = Some(*n);
                }
            } else {
                self.database.tools.counts.insert(name, *n);
            }
        } else {
            self.database.status = failure.map_or_else(
                || format!("Count failed or incomplete: {:?}", result.completion),
                |error| format!("Count failed: {error}"),
            );
        }
        self.log_outcome(self.database.status.clone());
    }
    pub(super) fn catalog_event(&mut self, event: Event) {
        let done = matches!(event.kind, EventKind::Finished(_) | EventKind::Failed(_));
        let error = match &event.kind {
            EventKind::Finished(c) => {
                self.database.transaction = c.transaction;
                if c.error.is_none() && c.delivery != tursobench_core::Delivery::Complete {
                    self.database.catalog_error =
                        "Schema discovery incomplete: metadata budget or delivery limit reached"
                            .into();
                    self.database
                        .status
                        .clone_from(&self.database.catalog_error);
                }
                c.error.as_ref()
            }
            EventKind::Failed(error) => Some(error),
            _ => None,
        };
        if let Some(error) = error {
            self.database.catalog_error = error.to_string();
            self.database
                .status
                .clone_from(&self.database.catalog_error);
        }
        self.database.catalog.apply(event);
        if done {
            self.database.catalog_ticket = None;
            self.database.schema = self
                .database
                .catalog
                .rows
                .drain(..)
                .filter_map(|row| {
                    let [
                        Value::Text(kind),
                        Value::Text(name),
                        Value::Text(table),
                        ddl,
                    ]: [Value; 4] = row.try_into().ok()?
                    else {
                        return None;
                    };
                    Some(SchemaObject {
                        kind,
                        name,
                        table,
                        ddl: match ddl {
                            Value::Text(sql) => Some(sql),
                            _ => None,
                        },
                    })
                })
                .collect();
            if self.database.catalog_refresh_pending {
                self.refresh_catalog();
            }
        }
    }

    pub(super) fn count_table(&mut self, name: String) {
        if self.database.tools.count_request.is_some()
            || !self.database.ready
            || self.database.closing
        {
            return;
        }
        let Some(session) = &self.database.session else {
            return;
        };
        self.database.sequence += 1;
        let ticket = session.ticket(self.database.sequence, self.database.generation);
        match session.submit(Request {
            ticket,
            operation: self
                .database
                .document
                .browse
                .as_ref()
                .filter(|b| b.table == name)
                .map_or_else(
                    || Operation::Count(name.clone()),
                    |b| Operation::CountFiltered(b.clone()),
                ),
        }) {
            Ok(()) => {
                let mut result = tursobench_core::ResultSet::default();
                result.start(ticket);
                let filtered = self.database.document.browse.as_ref().is_some_and(|b| {
                    b.table == name
                        && (!b.filters.is_empty()
                            || b.options.predicate.is_some()
                            || b.options.relation.is_some())
                });
                self.database.tools.count_request = Some(tools::CountRequest {
                    name,
                    result,
                    owner: filtered.then_some((
                        self.database.document.id,
                        self.database.document.result.current,
                    )),
                });
                self.database.status =
                    "Counting rows; full scan, discard interrupts SELECT reads; writes finish normally".into();
            }
            Err(e) => self.database.status = e.to_string(),
        }
    }
    pub(super) fn request_inspection(&mut self, name: String) {
        self.database.document.tools.inspection = None;
        self.database.document.tools.inspection_error.clear();
        let Some(session) = &self.database.session else {
            return;
        };
        self.database.sequence += 1;
        let ticket = session.ticket(self.database.sequence, self.database.generation);
        match session.submit(Request {
            ticket,
            operation: Operation::Inspect(name),
        }) {
            Ok(()) => self.database.document.tools.inspection_ticket = Some(ticket),
            Err(e) => {
                self.database.document.tools.inspection_ticket = None;
                self.database.document.tools.inspection_error = e.to_string();
            }
        }
    }
    pub(super) fn structure_inspector(&mut self, ui: &mut egui::Ui) {
        let name = self
            .database
            .document
            .tools
            .ddl
            .as_ref()
            .map(|o| o.name.as_str())
            .or(self.database.document.object.as_deref())
            .or(self.database.ui.diagram.selected.as_deref());
        let Some(name) = name else {
            icons::empty_state(
                ui,
                Icon::Structure,
                "Select a table to inspect its structure",
            );
            return;
        };
        let mut section = self.database.document.ui.structure_section;
        let info = self.loaded_inspection(name);
        let object = self
            .database
            .document
            .tools
            .ddl
            .as_ref()
            .or_else(|| self.database.schema.iter().find(|o| o.name == name));
        if let Some(node) = self
            .database
            .ui
            .diagram
            .data
            .as_ref()
            .and_then(|g| g.tables.iter().find(|n| n.name == name))
            && let Some(error) = &node.error
        {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        if !self.database.document.tools.inspection_error.is_empty() {
            ui.colored_label(
                ui.visuals().error_fg_color,
                &self.database.document.tools.inspection_error,
            );
        }
        let action = presentation::Structure {
            info,
            object,
            schema: &self.database.schema,
            loading: self.database.document.tools.inspection_ticket.is_some(),
            graph: self.database.ui.diagram.data.as_ref(),
            incomplete: self.schema_incomplete(),
            selected: self
                .database
                .document
                .tools
                .highlight_column
                .as_ref()
                .filter(|(table, _)| table == name)
                .map(|(_, column)| column.as_str())
                .or_else(|| {
                    self.database
                        .document
                        .table
                        .as_ref()
                        .filter(|table| table.name == name)
                        .and(self.database.document.inspect)
                        .and_then(|(_, col)| self.database.document.result.columns.get(col))
                        .map(|c| c.name.as_str())
                }),
        }
        .show(ui, &mut section);
        self.database.document.ui.structure_section = section;
        match action {
            Some(presentation::Action::Column(column)) => {
                let table = name.to_owned();
                if self
                    .database
                    .document
                    .table
                    .as_ref()
                    .is_some_and(|t| t.name == table)
                    && let Some(col) = self
                        .database
                        .document
                        .result
                        .columns
                        .iter()
                        .position(|c| c.name.eq_ignore_ascii_case(&column))
                    && let Some(row) = self
                        .database
                        .document
                        .inspect
                        .map(|(r, _)| r)
                        .or(self.database.document.tools.detail)
                        .or_else(|| (!self.database.document.result.rows.is_empty()).then_some(0))
                {
                    self.select_cell(row, col, false);
                    self.database.document.tools.record_scroll_to = Some(col);
                }
                self.database.document.tools.highlight_column = Some((table, column));
                self.database.document.tools.scroll_to_column = true;
            }
            Some(presentation::Action::Load) => self.request_inspection(name.into()),
            Some(presentation::Action::Browse(table)) => self.browse_table(table),
            Some(presentation::Action::Definition(name)) => {
                self.submit(Operation::Definition(name), View::Diagnostics);
            }
            None => {}
        }
    }
    pub(super) fn follow_key(&mut self, row: usize, key: ForeignKey) {
        if !self.database.ready || self.database.closing {
            self.database.status =
                "Open a database before following references; retained values are unchanged".into();
            return;
        }
        if self.has_drafts()
            || self.database.commit_ticket.is_some()
            || self.database.document.view != View::Browse
            || self.database.document.active.is_some()
        {
            self.database.status =
                "Finish execution and commit or discard drafts before following a reference".into();
            return;
        }
        let Some(table) = self.database.document.table.as_ref() else {
            return;
        };
        let Some(values) = self.database.document.result.rows.get(row) else {
            return;
        };
        let mut keys = Vec::new();
        for (source, target) in key.columns {
            let Some(target) = target else {
                self.database.status = "Referenced primary key could not be resolved".into();
                return;
            };
            let Some(index) = table
                .columns
                .iter()
                .position(|c| c.name.eq_ignore_ascii_case(&source))
            else {
                return;
            };
            let Some(value) = values.get(index) else {
                return;
            };
            if *value == Value::Null {
                self.database.status = "NULL foreign key has no referenced row".into();
                return;
            }
            keys.push((target, value.clone()));
        }
        let previous = self.navigation_entry();
        if self.database.workspace.enabled
            && self
                .create_window(
                    super::tiling::Kind::Data,
                    Some(key.target.clone()),
                    Some(self.database.document.id),
                    true,
                )
                .is_none()
        {
            return;
        }
        if self
            .submit(
                Operation::Lookup {
                    table: key.target.clone(),
                    keys: keys.clone(),
                },
                View::Browse,
            )
            .is_some()
        {
            if let Some(previous) = previous {
                self.database.tools.back.push(previous);
                if self.database.tools.back.len() > 32 {
                    self.database.tools.back.remove(0);
                }
            }
            self.database.document.browse = None;
            self.database.document.table = None;
            self.database.ui.diagram.selected = Some(key.target.clone());
            self.database.document.tools.ddl = None;
            self.database.document.tools.related = Some(Destination::Related(key.target, keys));
        }
    }
    pub(super) fn follow_incoming(&mut self, row: usize, child: String, key: ForeignKey) {
        if self.database.document.active.is_some() || self.database.document.view != View::Browse {
            return;
        }
        let (Some(table), Some(values)) = (
            &self.database.document.table,
            self.database.document.result.rows.get(row),
        ) else {
            return;
        };
        if !table.name.eq_ignore_ascii_case(&key.target) {
            return;
        }
        let mut bound = Vec::new();
        for (_, target) in &key.columns {
            let Some(target) = target else {
                return;
            };
            let Some(index) = table
                .columns
                .iter()
                .position(|c| c.name.eq_ignore_ascii_case(target))
            else {
                return;
            };
            let Some(value) = values.get(index).filter(|v| **v != Value::Null) else {
                return;
            };
            bound.push(value.clone());
        }
        let mut browse = tursobench_core::Browse {
            table: child.clone(),
            offset: 0,
            sort: None,
            descending: false,
            filters: vec![],
            options: Default::default(),
        };
        browse.options.relation = Some(Box::new((key, bound)));
        let destination = Destination::Page(browse.clone());
        let previous = self.navigation_entry();
        if self.has_drafts() || self.database.commit_ticket.is_some() {
            return;
        }
        if self.database.workspace.enabled
            && self
                .create_window(
                    super::tiling::Kind::Data,
                    Some(child.clone()),
                    Some(self.database.document.id),
                    true,
                )
                .is_none()
        {
            return;
        }
        if self.submit(destination.operation(), View::Browse).is_some() {
            if let Some(previous) = previous {
                self.database.tools.back.push(previous);
                if self.database.tools.back.len() > 32 {
                    self.database.tools.back.remove(0);
                }
            }
            self.database.document.browse = Some(browse);
            self.database.document.table = None;
            self.database.document.tools.ddl = None;
            self.database.ui.diagram.selected = Some(child);
            self.database.document.tools.related = None;
        }
    }
    pub(super) fn navigation_entry(&self) -> Option<NavigationEntry> {
        if self.database.document.view != View::Browse {
            return None;
        }
        let destination = self
            .database
            .document
            .tools
            .related
            .clone()
            .or_else(|| self.database.document.browse.clone().map(Destination::Page))?;
        Some(NavigationEntry {
            destination,
            cell: self.database.document.inspect.and_then(|(r, c)| {
                self.database
                    .document
                    .result
                    .rows
                    .get(r)
                    .cloned()
                    .map(|row| (row, c))
            }),
            record: self
                .database
                .document
                .tools
                .detail
                .and_then(|r| self.database.document.result.rows.get(r).cloned()),
            structure: self.database.document.ui.table_view == shell::TableView::Structure,
            scroll: self.database.document.tools.grid_scroll,
            window_rows: self.database.document.result.rows.len(),
        })
    }
    pub(super) fn restore_destination(&mut self, entry: NavigationEntry) -> bool {
        if self.has_drafts() || self.database.commit_ticket.is_some() {
            return false;
        }
        if self.database.workspace.enabled
            && self
                .create_window(
                    super::tiling::Kind::Data,
                    Some(entry.destination.table().into()),
                    None,
                    false,
                )
                .is_none()
        {
            return false;
        }
        if self
            .submit(entry.destination.operation(), View::Browse)
            .is_none()
        {
            return false;
        }
        self.database.document.tools.ddl = None;
        self.database.document.table = None;
        self.database.ui.diagram.selected = Some(entry.destination.table().into());
        self.database.ui.unfolded = Some(entry.destination.table().into());
        match &entry.destination {
            Destination::Page(b) => {
                self.database.document.browse = Some(b.clone());
                self.database.document.tools.related = None;
            }
            related => {
                self.database.document.browse = None;
                self.database.document.tools.related = Some(related.clone());
            }
        }
        self.database.document.ui.table_view = if entry.structure {
            shell::TableView::Structure
        } else {
            shell::TableView::Data
        };
        self.database.document.tools.pages.restore_rows = if self.database.document.browse.is_some()
        {
            entry
                .window_rows
                .min(self.preferences.row_limit.clamp(200, 10_000) - 100)
        } else {
            0
        };
        self.database.document.tools.restore_navigation = Some(entry);
        true
    }
    pub(super) fn navigate_back(&mut self) {
        let Some(entry) = self.database.tools.back.last().cloned() else {
            return;
        };
        if self.restore_destination(entry) {
            self.database.tools.back.pop();
        }
    }
    pub(super) fn return_to_table(&mut self) {
        let Some(entry) = self.database.tools.result_return.clone() else {
            return;
        };
        if self.restore_destination(entry) {
            self.database.tools.result_return = None;
        }
    }
    pub(super) fn restore_browse_selection(&mut self) {
        if self.database.document.tools.pages.restore_rows
            > self.database.document.result.rows.len()
            && !self.database.document.tools.pages.capacity_reached
            && self.database.document.tools.pages.end.is_none()
        {
            return;
        }
        self.database.document.tools.pages.restore_rows = 0;
        let Some(entry) = self.database.document.tools.restore_navigation.take() else {
            return;
        };
        self.database.document.inspect = entry.cell.and_then(|(values, column)| {
            (column < self.database.document.result.columns.len())
                .then(|| {
                    self.database
                        .document
                        .result
                        .rows
                        .iter()
                        .position(|r| *r == values)
                        .map(|row| (row, column))
                })
                .flatten()
        });
        self.database.document.tools.detail = entry.record.and_then(|values| {
            self.database
                .document
                .result
                .rows
                .iter()
                .position(|r| *r == values)
        });
        self.database.document.tools.grid_selection =
            self.database
                .document
                .inspect
                .map(|cell| super::selection::Selection {
                    ticket: self.database.document.result.current,
                    anchor: cell,
                    cursor: cell,
                });
        self.database.document.tools.restore_scroll = Some(entry.scroll);
    }
}
