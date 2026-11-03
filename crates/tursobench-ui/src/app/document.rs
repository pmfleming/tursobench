//! A view owns its buffers and interaction state. Switching moves ownership, never clones rows.
use super::{App, View, results, shell, tools};
use tursobench_core::{Browse, DraftBatch, ResultSet, Table, Ticket};

pub(super) struct Document {
    pub id: u64,
    pub kind: super::tiling::Kind,
    pub object: Option<String>,
    pub source: Option<u64>,
    pub tools: tools::ViewTools,
    pub ui: shell::ViewUi,
    pub discard_ticket: Option<Ticket>,
    pub active: Option<Ticket>,
    pub view: View,
    pub result: ResultSet,
    pub table: Option<Table>,
    pub browse: Option<Browse>,
    pub sql: String,
    pub params: String,
    pub filter: String,
    pub filter_column: usize,
    pub filter_op: tursobench_core::FilterOp,
    pub filter_kind: results::CellKind,
    pub inspect: Option<(usize, usize)>,
    pub drafts: Option<DraftBatch>,
    pub insert_json: String,
    pub show_insert: bool,
    pub show_preview: bool,
}
impl Default for Document {
    fn default() -> Self {
        Self {
            id: 0,
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
            table: None,
            browse: None,
            sql: "SELECT name, type FROM sqlite_schema ORDER BY name".into(),
            params: "[]".into(),
            filter: String::new(),
            filter_column: 0,
            filter_op: Default::default(),
            filter_kind: Default::default(),
            inspect: None,
            drafts: None,
            insert_json: String::new(),
            show_insert: false,
            show_preview: false,
        }
    }
}
impl Document {
    pub fn title(&self) -> String {
        match self.kind {
            super::tiling::Kind::Map => "Database · Map".into(),
            super::tiling::Kind::Sql => format!("Query {} · SQL", self.id),
            super::tiling::Kind::Result => {
                format!("Query {} · Result", self.source.unwrap_or(self.id))
            }
            kind => format!(
                "{} · {}",
                self.object.as_deref().unwrap_or("Retained"),
                kind.label()
            ),
        }
    }
    pub fn owns(&self, ticket: Ticket) -> bool {
        self.result.current == Some(ticket)
            || self.tools.inspection_ticket == Some(ticket)
            || self
                .tools
                .context
                .pending
                .as_ref()
                .is_some_and(|r| r.result.current == Some(ticket))
            || self
                .tools
                .pages
                .pending
                .as_ref()
                .is_some_and(|r| r.result.current == Some(ticket))
    }
    pub fn has_drafts(&self) -> bool {
        self.drafts.as_ref().is_some_and(|d| !d.edits.is_empty())
    }
}
impl App {
    pub(super) fn switch_document(&mut self, id: u64) -> bool {
        if id == self.document.id {
            return true;
        }
        let Some(next) = self.documents.remove(&id) else {
            return false;
        };
        let previous = std::mem::replace(&mut self.document, next);
        self.documents.insert(previous.id, previous);
        true
    }
    pub(super) fn document_owner(&self, ticket: Ticket) -> Option<u64> {
        if self.commit_ticket == Some(ticket) {
            return self.commit_owner;
        }
        if self.document.owns(ticket) {
            return Some(self.document.id);
        }
        self.documents
            .iter()
            .find_map(|(id, document)| document.owns(ticket).then_some(*id))
    }
}
