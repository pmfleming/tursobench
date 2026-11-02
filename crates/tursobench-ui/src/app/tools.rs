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
#[derive(Default)]
pub(super) struct Tools {
    pub schema_filter: String,
    pub ddl: Option<SchemaObject>,
    pub inspection: Option<Inspection>,
    pub inspection_ticket: Option<Ticket>,
    pub inspection_error: String,
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
    pub columns: super::columns::Columns,
    pub column_scope: String,
    pub column_layouts: BTreeMap<String, super::columns::Columns>,
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
}
impl Tools {
    pub fn new() -> Self {
        Self {
            export_table: "exported_rows".into(),
            ..Self::default()
        }
    }
}
