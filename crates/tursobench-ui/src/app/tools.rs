use super::results;
use tursobench_core::{Browse, Inspection, SchemaObject, Ticket, Value};

use std::{collections::BTreeMap, ops::Range};
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

#[derive(Default)]
pub(super) struct ViewTools {
    pub large: super::large_values::Viewer,
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
    pub related: Option<Destination>,
    pub restore_navigation: Option<NavigationEntry>,
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
