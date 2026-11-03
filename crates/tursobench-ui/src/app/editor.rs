use super::{
    App, View,
    icons::{self, Icon},
    shell,
};
use eframe::egui;
use tursobench_core::{Bindings, Budget, quote_identifier};
use tursobench_db::Operation;

use tursobench_db::sql::{self, TokenStyle};

fn byte_index(text: &str, character: usize) -> usize {
    text.char_indices()
        .nth(character)
        .map_or(text.len(), |(i, _)| i)
}
#[cfg(test)]
#[test]
fn completions_and_highlighting_preserve_unicode_and_quoted_text() {
    assert_eq!(byte_index("λx", 1), 2);
    assert_eq!(completion_span("SELECT users", 9), Some(7..12));
    assert_eq!(completion_span("SELECT λ", "SELECT λ".len()), Some(7..9));
    for text in [
        "SELECT 'na",
        "SELECT \"na",
        "SELECT :na",
        "-- comment",
        "/* unfinished",
        "# 'unfinished",
    ] {
        assert!(completion_span(text, text.len()).is_none());
    }
    let text = "SELECT 'λ;'; -- comment\nSELECT :id";
    let ctx = egui::Context::default();
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
        assert_eq!(highlighted(ui, text, 800.0).text(), text);
    });
    output.textures_delta.clear();
}
fn completion_span(text: &str, cursor: usize) -> Option<std::ops::Range<usize>> {
    if text.len() > 64 * 1024 || cursor > text.len() || !text.is_char_boundary(cursor) {
        return None;
    }
    for token in sql::tokens(text) {
        if token.range.start < cursor && token.range.end >= cursor {
            let raw = &text[token.range.clone()];
            if matches!(
                token.style,
                TokenStyle::String
                    | TokenStyle::Comment
                    | TokenStyle::Parameter
                    | TokenStyle::Invalid
            ) || raw.starts_with(['\'', '"', '`', '['])
            {
                return None;
            }
        }
    }
    let start = text[..cursor]
        .char_indices()
        .rev()
        .find(|(_, c)| !c.is_alphanumeric() && *c != '_')
        .map_or(0, |(i, c)| i + c.len_utf8());
    let end = cursor
        + text[cursor..]
            .char_indices()
            .take_while(|(_, c)| c.is_alphanumeric() || *c == '_')
            .last()
            .map_or(0, |(i, c)| i + c.len_utf8());
    Some(start..end)
}
fn highlighted(ui: &egui::Ui, text: &str, width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = width;
    let plain = ui.visuals().text_color();
    if text.len() > 64 * 1024 {
        job.append(
            text,
            0.0,
            egui::TextFormat {
                font_id: egui::FontId::monospace(14.0),
                color: plain,
                ..Default::default()
            },
        );
    } else {
        for token in sql::tokens(text) {
            let color = match token.style {
                TokenStyle::Keyword => ui.visuals().hyperlink_color,
                TokenStyle::String => plain,
                TokenStyle::Number | TokenStyle::Parameter => ui.visuals().warn_fg_color,
                TokenStyle::Comment => ui.visuals().weak_text_color(),
                _ => plain,
            };
            job.append(
                &text[token.range],
                0.0,
                egui::TextFormat {
                    font_id: egui::FontId::monospace(14.0),
                    color,
                    ..Default::default()
                },
            );
        }
    }
    ui.fonts_mut(|fonts| fonts.layout_job(job))
}
impl App {
    pub(super) fn run_text(&mut self, text: String, explain: Option<bool>) {
        if self.has_drafts() || self.commit_ticket.is_some() {
            self.status = "Commit or discard drafts first".into();
            return;
        }
        match serde_json::from_str::<Bindings>(&self.document.params) {
            Ok(parameters) => {
                let operation = match explain {
                    Some(bytecode) => Operation::Explain {
                        sql: text.clone(),
                        parameters,
                        bytecode,
                    },
                    None => Operation::ExecuteBound {
                        sql: text.clone(),
                        parameters,
                        budget: Budget {
                            rows: self.preferences.row_limit,
                            ..Default::default()
                        },
                    },
                };
                if self.workspace.enabled {
                    if let Some(previous) = self.navigation_entry() {
                        self.tools.result_return = Some(previous);
                    }
                    let source = if self.document.kind == super::tiling::Kind::Result {
                        self.document.source.unwrap_or(self.document.id)
                    } else {
                        self.document.id
                    };
                    let sql = self.document.sql.clone();
                    let params = self.document.params.clone();
                    if self
                        .create_window(super::tiling::Kind::Result, None, Some(source), false)
                        .is_none()
                    {
                        return;
                    }
                    self.document.sql = sql;
                    self.document.params = params;
                }
                if let Some(ticket) = self.submit(
                    operation,
                    if explain.is_some() {
                        View::Explain
                    } else {
                        View::Query
                    },
                ) {
                    let history_sql = match explain {
                        Some(true) => format!("EXPLAIN {text}"),
                        Some(false) => format!("EXPLAIN QUERY PLAN {text}"),
                        None => text,
                    };
                    self.tools.pending_history.insert(
                        ticket.request,
                        (self.tools.database_scope.clone(), history_sql),
                    );
                    self.document.table = None;
                    self.document.browse = None;
                    self.document.tools.related = None;
                }
            }
            Err(e) => self.status = format!("Typed bindings: {e}"),
        }
    }
    pub(super) fn run_selection(&mut self) {
        let start = byte_index(&self.document.sql, self.document.tools.selection.start);
        let end = byte_index(&self.document.sql, self.document.tools.selection.end);
        match sql::selection_or_statement(&self.document.sql, start..end) {
            Ok(text) => self.run_text(text, None),
            Err(e) => self.status = e.to_string(),
        }
    }
    pub(super) fn editor_ui(&mut self, ui: &mut egui::Ui) {
        if !self.workspace.enabled {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                if ui
                    .add_enabled(
                        self.ready
                            && !self.closing
                            && !self.has_drafts()
                            && self.commit_ticket.is_none(),
                        egui::Button::new("Run"),
                    )
                    .on_hover_text("Run one statement · Ctrl Enter")
                    .clicked()
                {
                    self.run_sql();
                }
                if icons::button(
                    ui,
                    Icon::Selection,
                    "Run selection / statement at cursor",
                    "Ctrl Shift Enter",
                )
                .clicked()
                {
                    self.run_selection();
                }
                if icons::button(ui, Icon::Explain, "Explain query plan", "").clicked() {
                    self.run_text(self.document.sql.clone(), Some(false));
                }
                if icons::control(
                    ui,
                    Icon::Parameters,
                    "Typed parameters",
                    "",
                    self.document.ui.params_open,
                    false,
                )
                .clicked()
                {
                    self.document.ui.params_open = !self.document.ui.params_open;
                }
                if icons::control(
                    ui,
                    Icon::Complete,
                    "Complete",
                    "Ctrl Space",
                    self.document.tools.complete,
                    false,
                )
                .clicked()
                {
                    self.document.tools.complete = !self.document.tools.complete;
                }
                if icons::control(
                    ui,
                    Icon::Fit,
                    "Expand SQL editor",
                    "",
                    self.document.ui.sql_expanded,
                    false,
                )
                .clicked()
                {
                    self.document.ui.sql_expanded = !self.document.ui.sql_expanded;
                }
                if icons::button(ui, Icon::Close, "Close SQL editor", "Ctrl J").clicked() {
                    self.document.ui.sql_open = false;
                }
                let more = icons::button(ui, Icon::More, "SQL actions", "");
                egui::Popup::menu(&more).show(|ui| {
                    if ui.button("Explain bytecode").clicked() {
                        self.run_text(self.document.sql.clone(), Some(true));
                        ui.close();
                    }
                    if ui.button("Bookmark editor SQL").clicked() {
                        self.ui.sidebar = shell::ToolPanel::Saved;
                        ui.close();
                    }
                });
            });
        }
        let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, width: f32| {
            highlighted(ui, text.as_str(), width)
        };
        let output = egui::ScrollArea::vertical()
            .id_salt("sql-editor-scroll")
            .max_height(if self.workspace.enabled {
                (ui.available_height()
                    - if self.document.ui.params_open {
                        120.0
                    } else {
                        8.0
                    })
                .max(24.0)
            } else {
                (ui.ctx().content_rect().height() * 0.35).clamp(100.0, 400.0)
            })
            .show(ui, |ui| {
                egui::TextEdit::multiline(&mut self.document.sql)
                    .id_salt("sql-editor")
                    .code_editor()
                    .hint_text("Write SQL…  Ctrl Enter to run")
                    .desired_rows(5)
                    .desired_width(f32::INFINITY)
                    .layouter(&mut layouter)
                    .show(ui)
            })
            .inner;
        self.document.tools.sql_focus = Some(output.response.id);
        if let Some(cursor) = output.cursor_range {
            let range = cursor.as_sorted_char_range();
            self.document.tools.selection = range.start.0..range.end.0;
        }
        if self.document.ui.params_open {
            ui.horizontal_wrapped(|ui| {
            ui.label("Typed bindings (JSON):");ui.add(egui::TextEdit::singleline(&mut self.document.params).desired_width(ui.available_width().min(420.0)));
            if ui.button("Generate NULL bindings").clicked(){
                let names=sql::parameter_names(&self.document.sql);
                if names.iter().any(|n|n.starts_with('?')) {self.status="Positional parameters use an array ordered by slot: [{\"type\":\"Null\"}, ...]".into();}
                else {self.document.params=serde_json::to_string(&sql::named_template(&self.document.sql)).unwrap_or_default();}
            }
        });
            ui.small("?1: [{\"type\":\"Integer\",\"value\":42}]    :id: {\":id\":{\"type\":\"Integer\",\"value\":42}}. Values are never interpolated or persisted.");
        }
        if self.document.tools.complete {
            self.completion_ui(ui, output.response.id);
        }
    }
    fn completion_ui(&mut self, ui: &mut egui::Ui, editor_id: egui::Id) {
        let cursor = byte_index(&self.document.sql, self.document.tools.selection.end);
        let Some(span) = completion_span(&self.document.sql, cursor) else {
            ui.small("Completion unavailable here: quoted/comment/parameter text, a lexical error, or a buffer above 64 KiB.");
            return;
        };
        let start = span.start;
        let prefix = self.document.sql[start..cursor].to_lowercase();
        let mut candidates = vec![
            "SELECT",
            "FROM",
            "WHERE",
            "ORDER BY",
            "GROUP BY",
            "LIMIT",
            "JOIN",
            "INSERT INTO",
            "UPDATE",
            "DELETE FROM",
            "RETURNING",
            "NULL",
            "fts_match",
            "fts_score",
            "uuid4",
            "uuid7",
            "vector_distance_cos",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
        candidates.extend(
            self.schema
                .iter()
                .filter(|o| o.kind == "table" || o.kind == "view")
                .map(|o| quote_identifier(&o.name)),
        );
        if let Some(info) = &self.document.tools.inspection {
            candidates.extend(info.table.columns.iter().map(|c| quote_identifier(&c.name)));
        }
        candidates.retain(|c| c.trim_matches('"').to_lowercase().starts_with(&prefix));
        candidates.sort();
        candidates.dedup();
        ui.small("Prefix completions: SQL/Turso functions, schema objects and inspected-table columns. Alias resolution is not inferred.");
        ui.horizontal_wrapped(|ui| {
            for candidate in candidates.into_iter().take(30) {
                if ui.button(&candidate).clicked() {
                    self.document.sql.replace_range(span.clone(), &candidate);
                    let position =
                        self.document.sql[..start].chars().count() + candidate.chars().count();
                    self.document.tools.selection = position..position;
                    if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), editor_id) {
                        state
                            .cursor
                            .set_char_range(Some(egui::text::CCursorRange::one(
                                egui::text::CCursor::new(position),
                            )));
                        state.store(ui.ctx(), editor_id);
                        ui.memory_mut(|m| m.request_focus(editor_id));
                    }
                    self.document.tools.complete = false;
                }
            }
        });
    }
}
