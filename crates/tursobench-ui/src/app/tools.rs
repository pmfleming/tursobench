use super::results;
use tursobench_core::{Browse, Inspection, SchemaObject, Ticket, Value};

use std::{collections::BTreeMap, ops::Range};
#[derive(Clone)]
pub(super) enum Destination {
    Page(Browse),
    Related(String, Vec<(String, Value)>),
}
pub(super) struct Tools {
    pub schema_filter: String,
    pub ddl: Option<SchemaObject>,
    pub inspection: Option<Inspection>,
    pub inspection_ticket: Option<Ticket>,
    pub inspection_error: String,
    pub show_metadata: bool,
    pub related: Option<(String, Vec<(String, Value)>)>,
    pub back: Vec<Destination>,
    pub cell_kind: results::CellKind,
    pub cell_input: String,
    pub selection: Range<usize>,
    pub complete: bool,
    pub help: bool,
    pub detail: Option<usize>,
    pub widths: Vec<f32>,
    pub history_search: String,
    pub bookmark_search: String,
    pub bookmark_name: String,
    pub slow_only: bool,
    pub sort_slow: bool,
    pub pending_history: BTreeMap<u64, (String, String)>,
    pub database_scope: String,
    pub last_elapsed: u64,
    pub format: crate::export::Format,
    pub export_table: String,
    pub diagnostic: tursobench_db::Diagnostic,
}
impl Default for Tools {
    fn default() -> Self {
        Self {
            schema_filter: String::new(),
            ddl: None,
            inspection: None,
            inspection_ticket: None,
            inspection_error: String::new(),
            show_metadata: false,
            related: None,
            back: vec![],
            cell_kind: results::CellKind::Text,
            cell_input: String::new(),
            selection: 0..0,
            complete: false,
            help: false,
            detail: None,
            widths: vec![],
            history_search: String::new(),
            bookmark_search: String::new(),
            bookmark_name: String::new(),
            slow_only: false,
            sort_slow: false,
            pending_history: BTreeMap::new(),
            database_scope: String::new(),
            last_elapsed: 0,
            format: crate::export::Format::JsonLines,
            export_table: "exported_rows".into(),
            diagnostic: tursobench_db::Diagnostic::DatabaseList,
        }
    }
}
