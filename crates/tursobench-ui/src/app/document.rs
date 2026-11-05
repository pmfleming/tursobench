//! A view owns its buffers and interaction state. Switching moves ownership, never clones rows.
use super::{App, View, shell, tools};
use tursobench_core::{Browse, DraftBatch, Inspection, ResultSet, Table, Ticket};

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
    pub tools: tools::ViewTools,
    pub ui: shell::ViewUi,
    pub discard_ticket: Option<Ticket>,
    pub active: Option<Ticket>,
    pub view: View,
    pub result: ResultSet,
    pub script_results: Vec<tursobench_core::ScriptResult>,
    pub script_message: String,
    pub table: Option<Table>,
    pub browse: Option<Browse>,
    pub sql: String,
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
            tools: tools::ViewTools {
                export_table: "exported_rows".into(),
                ..Default::default()
            },
            ui: Default::default(),
            active: None,
            discard_ticket: None,
            view: View::Query,
            result: Default::default(),
            script_results: vec![],
            script_message: String::new(),
            table: None,
            browse: None,
            sql: "SELECT name, type FROM sqlite_schema ORDER BY name".into(),
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
