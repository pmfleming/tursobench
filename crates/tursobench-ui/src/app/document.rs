//! A view owns its buffers and interaction state. Switching moves ownership, never clones rows.
use super::{App, View, results, shell, tools};
use tursobench_core::{Browse, DraftBatch, Inspection, ResultSet, SchemaObject, Table, Ticket};

use std::{collections::BTreeMap, ops::Range};

#[derive(Default)]
pub(super) struct ViewTools {
    pub spreadsheet: super::spreadsheet::State,
    pub large: results::Viewer,
    pub filtered_count: Option<i64>,
    pub draft_ui: super::editing::Presentation,
    pub sql_focus: Option<eframe::egui::Id>,
    pub filter_focus: bool,
    pub filter_error: String,
    pub filter_editor: Option<super::filters::FilterEditor>,
    pub ddl: Option<SchemaObject>,
    pub inspection: Option<Inspection>,
    pub inspection_ticket: Option<Ticket>,
    pub inspection_error: String,
    pub related: Option<tools::Destination>,
    pub restore_navigation: Option<tools::NavigationEntry>,
    pub context: super::context::ContextReads,
    pub pages: super::paging::Pages,
    pub grid_scroll: eframe::egui::Vec2,
    pub row_stride: f32,
    pub jump_row: usize,
    pub advanced_filter: String,
    pub restore_scroll: Option<eframe::egui::Vec2>,
    pub cell_editor: results::CellEditor,
    pub selection: Range<usize>,
    pub complete: bool,
    pub detail: Option<usize>,
    pub summary_cache: Option<super::selection::SummaryCache>,
    pub grid_selection: Option<super::selection::Selection>,
    pub grid_height: f32,
    pub grid_focus: Option<eframe::egui::Id>,
    pub dragging_selection: bool,
    pub scroll_to_cell: bool,
    pub record_scroll_to: Option<usize>,
    pub inline: Option<super::selection::InlineEdit>,
    pub columns: super::columns::Columns,
    pub column_scope: String,
    pub column_layouts: BTreeMap<String, super::columns::Columns>,
    pub highlight_column: Option<(String, String)>,
    pub scroll_to_column: bool,
    pub format: crate::export::Format,
    pub export_table: String,
}

pub(super) struct Document {
    pub migration: Option<std::sync::Arc<tursobench_core::SchemaSnapshot>>,
    pub id: u64,
    pub profile: Option<tursobench_db::profile::Request>,
    pub explain: super::explain::State,
    pub restore_cursor: Option<std::ops::Range<usize>>,
    pub needs_load: bool,
    pub stale: bool,
    pub external_refresh: bool,
    pub kind: super::tiling::Kind,
    pub object: Option<String>,
    pub source: Option<u64>,
    pub tools: ViewTools,
    pub ui: shell::ViewUi,
    pub discard_ticket: Option<Ticket>,
    pub active: Option<Ticket>,
    pub view: View,
    pub result: ResultSet,
    pub selected_script: Option<usize>,
    pub script_results: Vec<tursobench_core::ScriptResult>,
    pub script_message: String,
    pub table: Option<Table>,
    pub browse: Option<Browse>,
    pub sql: String,
    pub sql_revision: u64,
    pub params: String,
    pub filter: String,
    pub inspect: Option<(usize, usize)>,
    pub drafts: Option<DraftBatch>,
    pub draft_revision: u64,
    pub insert_json: String,
    pub insert_fields: Vec<super::editing::InsertField>,
    pub insert_use_json: bool,
    pub show_insert: bool,
    pub show_preview: bool,
}
impl Default for Document {
    fn default() -> Self {
        Self {
            migration: None,
            id: 0,
            profile: None,
            explain: Default::default(),
            restore_cursor: None,
            needs_load: false,
            stale: false,
            external_refresh: false,
            kind: Default::default(),
            object: None,
            source: None,
            tools: ViewTools {
                export_table: "exported_rows".into(),
                ..Default::default()
            },
            ui: Default::default(),
            active: None,
            discard_ticket: None,
            view: View::Query,
            result: Default::default(),
            selected_script: None,
            script_results: vec![],
            script_message: String::new(),
            table: None,
            browse: None,
            sql: "SELECT name, type FROM sqlite_schema ORDER BY name".into(),
            sql_revision: 0,
            params: "[]".into(),
            filter: String::new(),
            inspect: None,
            drafts: None,
            draft_revision: 0,
            insert_json: String::new(),
            insert_fields: vec![],
            insert_use_json: false,
            show_insert: false,
            show_preview: false,
        }
    }
}
impl Document {
    pub fn start_result(&mut self, ticket: Ticket, view: View) {
        self.tools.spreadsheet = Default::default();
        self.tools.restore_navigation = None;
        self.tools.grid_scroll = eframe::egui::Vec2::ZERO;
        self.tools.restore_scroll = None;
        self.tools.context = Default::default();
        self.tools.pages = Default::default();
        self.tools.grid_selection = None;
        self.tools.inline = None;
        self.tools.filtered_count = None;
        self.profile = None;
        self.script_results.clear();
        self.selected_script = None;
        self.script_message.clear();
        self.stale = false;
        self.external_refresh = false;
        self.result.start(ticket);
        self.active = Some(ticket);
        self.view = view;
        self.ui.nav = if view == View::Browse {
            shell::Nav::Table
        } else {
            shell::Nav::Result
        };
        self.ui.table_view = shell::TableView::Data;
        self.ui.filter_open = false;
        if matches!(view, View::Query | View::Explain) {
            self.ui.sql_open = true;
        }
        if view != View::Browse {
            self.tools.export_table = "exported_rows".into();
        }
        self.inspect = None;
        self.tools.detail = None;
    }
    pub fn set_sql(&mut self, sql: String) {
        if self.sql != sql {
            self.sql = sql;
            self.sql_changed();
        }
    }
    pub fn sql_changed(&mut self) {
        self.sql_revision = self.sql_revision.wrapping_add(1);
    }
    pub fn push_script_result(&mut self, result: tursobench_core::ScriptResult) {
        self.script_results.push(result);
        self.select_script_result(self.script_results.len() - 1);
    }
    pub fn select_script_result(&mut self, index: usize) {
        if index >= self.script_results.len() {
            return;
        }
        // The visible payload has one owner. Park it in its script slot before
        // moving another statement into the ordinary result/export/grid path.
        if let Some(previous) = self.selected_script {
            self.swap_script_payload(previous);
        }
        self.swap_script_payload(index);
        self.result.completion = Some(self.script_results[index].completion.clone());
        self.selected_script = Some(index);
    }
    fn swap_script_payload(&mut self, index: usize) {
        let result = &mut self.script_results[index];
        std::mem::swap(&mut self.result.columns, &mut result.columns);
        std::mem::swap(&mut self.result.rows, &mut result.rows);
    }
    pub fn browse_inspection(&self) -> Option<&Inspection> {
        self.tools.inspection.as_ref().filter(|info| {
            self.view == View::Browse
                && self
                    .table
                    .as_ref()
                    .is_some_and(|t| t.name == info.table.name)
        })
    }

    pub fn disconnect(&mut self) {
        self.tools.spreadsheet = Default::default();
        self.active = None;
        self.tools.large.ticket = None;
        self.tools.inspection_ticket = None;
        self.tools.context.pending = None;
        self.tools.pages.pending = None;
        self.needs_load = false;
    }
    pub fn has_drafts(&self) -> bool {
        self.drafts.as_ref().is_some_and(|d| !d.edits.is_empty())
    }
}
impl App {
    pub(super) fn switch_document(&mut self, id: u64) -> bool {
        if id == self.database.document.id {
            return true;
        }
        let Some(next) = self.database.documents.remove(&id) else {
            return false;
        };
        let previous = std::mem::replace(&mut self.database.document, next);
        self.database.documents.insert(previous.id, previous);
        true
    }
}
