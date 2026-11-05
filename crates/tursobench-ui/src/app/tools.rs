use tursobench_core::{Browse, Ticket, Value};

use std::collections::BTreeMap;
#[derive(Clone)]
pub(super) enum Destination {
    Page(Browse),
    Related(String, Vec<(String, Value)>),
}
impl Destination {
    pub fn table(&self) -> &str {
        match self {
            Self::Page(b) => &b.table,
            Self::Related(t, _) => t,
        }
    }
    pub fn operation(&self) -> tursobench_db::Operation {
        match self {
            Self::Page(b) => tursobench_db::Operation::Browse(b.clone()),
            Self::Related(table, keys) => tursobench_db::Operation::Lookup {
                table: table.clone(),
                keys: keys.clone(),
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
    pub window_rows: usize,
}
pub(super) struct CountRequest {
    pub name: String,
    pub result: tursobench_core::ResultSet,
    pub owner: Option<(u64, Option<Ticket>)>,
}
#[derive(Default)]
pub(super) struct Tools {
    pub snapshot: super::schema_compare::Capture,
    pub admin: super::admin::State,
    pub profile: super::profile::State,
    pub transfers: super::transfers::State,
    pub schema_filter: String,
    pub tree_focus: Option<eframe::egui::Id>,
    pub back: Vec<NavigationEntry>,
    pub result_return: Option<NavigationEntry>,
    pub counts: BTreeMap<String, i64>,
    pub count_request: Option<CountRequest>,
    pub help: bool,
    pub history_search: String,
    pub bookmark_search: String,
    pub bookmark_name: String,
    pub slow_only: bool,
    pub sort_slow: bool,
    pub pending_history: BTreeMap<u64, (String, String)>,
    pub database_scope: String,
}
