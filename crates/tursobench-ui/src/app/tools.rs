use super::results;
use tursobench_core::{Browse, Inspection, SchemaObject, Ticket, Value};

use std::{collections::BTreeMap, ops::Range};
#[derive(Clone)]
pub(super) enum Destination {
    Page(Browse),
    Related(String, Vec<(String, Value)>),
    References(String, tursobench_core::ForeignKey, Vec<Value>),
}
impl Destination {
    pub fn table(&self) -> &str {
        match self {
            Self::Page(b) => &b.table,
            Self::Related(t, _) | Self::References(t, _, _) => t,
        }
    }
    pub fn operation(&self) -> tursobench_db::Operation {
        match self {
            Self::Page(b) => tursobench_db::Operation::Browse(b.clone()),
            Self::Related(table, keys) => tursobench_db::Operation::Lookup {
                table: table.clone(),
                keys: keys.clone(),
            },
            Self::References(table, key, values) => tursobench_db::Operation::References {
                table: table.clone(),
                key: key.clone(),
                values: values.clone(),
            },
        }
    }
}
/// A browse destination owns typed predicates, not a display-only table name.
/// Selection is matched by retained values after reload, never by a stale index.
#[derive(Clone)]
pub(super) struct NavigationEntry {
    pub destination: Destination,
    pub cell: Option<(Vec<Value>, usize)>,
    pub record: Option<Vec<Value>>,
    pub structure: bool,
    pub scroll: eframe::egui::Vec2,
}
pub(super) struct Tools {
    pub schema_filter: String,
    pub ddl: Option<SchemaObject>,
    pub inspection: Option<Inspection>,
    pub inspection_ticket: Option<Ticket>,
    pub inspection_error: String,
    pub show_metadata: bool,
    pub related: Option<Destination>,
    pub back: Vec<NavigationEntry>,
    pub result_return: Option<NavigationEntry>,
    pub restore_navigation: Option<NavigationEntry>,
    pub counts: BTreeMap<String, i64>,
    pub count_request: Option<(String, tursobench_core::ResultSet)>,
    pub grid_scroll: eframe::egui::Vec2,
    pub restore_scroll: Option<eframe::egui::Vec2>,
    pub cell_kind: results::CellKind,
    pub cell_input: String,
    pub selection: Range<usize>,
    pub complete: bool,
    pub help: bool,
    pub detail: Option<usize>,
    pub widths: Vec<f32>,
    pub highlight_column: Option<(String, String)>,
    pub scroll_to_column: bool,
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
            result_return: None,
            restore_navigation: None,
            counts: BTreeMap::new(),
            count_request: None,
            grid_scroll: eframe::egui::Vec2::ZERO,
            restore_scroll: None,
            cell_kind: results::CellKind::Text,
            cell_input: String::new(),
            selection: 0..0,
            complete: false,
            help: false,
            detail: None,
            widths: vec![],
            highlight_column: None,
            scroll_to_column: false,
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
