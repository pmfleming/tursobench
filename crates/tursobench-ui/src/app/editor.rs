use super::{
    App, View,
    icons::{self, Icon},
};
use eframe::egui;
use tursobench_core::{Bindings, Budget};
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
    use super::test_ui::{control, pointer, render};
    assert_eq!(byte_index("λx", 1), 2);
    for (text, cursor) in [("λ", 1), ("x", 2), (&"x".repeat(65537), 0)] {
        assert!(completion_span(text, cursor).is_none());
    }
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
    ctx.enable_accesskit();
    let mut app = App::new(&ctx);
    app.database.document.ui.params_open = true;
    let draw = |app: &mut App, events| {
        render(&ctx, egui::vec2(800.0, 500.0), events, |ui| {
            app.editor_ui(ui)
        })
    };
    for sql in ["SELECT :id, @name, :id", "SELECT ?1", "SELECT 1"] {
        app.database.document.sql = sql.into();
        draw(&mut app, vec![]);
        let output = draw(&mut app, vec![]);
        let pos = control(&output, "Generate NULL bindings").center();
        for pressed in [true, false] {
            draw(&mut app, pointer(pos, pressed));
        }
        if sql.contains('?') {
            assert!(app.database.status.contains("Positional parameters"));
        } else {
            let Bindings::Named(values) =
                Bindings::parse_json(&app.database.document.params).unwrap()
            else {
                panic!("Expected named bindings")
            };
            assert_eq!(values, sql::named_template(sql));
        }
    }
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
                font_id: crate::appearance::monospace(ui.ctx(), 14.0),
                color: plain,
                ..Default::default()
            },
        );
    } else {
        for token in sql::tokens(text) {
            let color = match token.style {
                TokenStyle::Keyword => ui.visuals().hyperlink_color,
                TokenStyle::String | TokenStyle::Number => {
                    crate::appearance::Tokens::of(ui.ctx()).relationship
                }
                TokenStyle::Parameter => ui.visuals().warn_fg_color,
                TokenStyle::Comment => ui.visuals().weak_text_color(),
                _ => plain,
            };
            job.append(
                &text[token.range],
                0.0,
                egui::TextFormat {
                    font_id: crate::appearance::monospace(ui.ctx(), 14.0),
                    color,
                    italics: token.style == TokenStyle::Comment,
                    ..Default::default()
                },
            );
        }
    }
    ui.fonts_mut(|fonts| fonts.layout_job(job))
}
impl App {
    pub(super) fn run_transaction(&mut self, sql: &str) {
        self.submit(
            Operation::Execute {
                sql: sql.into(),
                parameters: vec![],
                budget: Budget::default(),
            },
            View::Query,
        );
    }
    pub(super) fn run_text(&mut self, text: String, explain: Option<bool>) {
        let migration = self.database.document.migration.clone();
        if self.has_drafts() || self.database.commit_ticket.is_some() {
            self.database.status = "Commit or discard drafts first".into();
            return;
        }
        match Bindings::parse_json(&self.database.document.params) {
            Ok(parameters) => {
                let operation = match explain {
                    Some(bytecode) => Operation::Explain {
                        sql: text.clone(),
                        parameters,
                        bytecode,
                    },
                    None if migration.is_some() => Operation::Migration {
                        sql: text.clone(),
                        expected: migration.clone().unwrap(),
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
                if self.database.workspace.enabled {
                    if let Some(previous) = self.navigation_entry() {
                        self.database.tools.result_return = Some(previous);
                    }
                    let source = if self.database.document.kind == super::tiling::Kind::Result {
                        self.database
                            .document
                            .source
                            .unwrap_or(self.database.document.id)
                    } else {
                        self.database.document.id
                    };
                    let sql = self.database.document.sql.clone();
                    let params = self.database.document.params.clone();
                    if self
                        .create_window(super::tiling::Kind::Result, None, Some(source), false)
                        .is_none()
                    {
                        return;
                    }
                    self.database.document.sql = sql;
                    self.database.document.params = params;
                }
                self.database.document.migration = migration.clone();
                if let Some(ticket) = self.submit(
                    operation,
                    if explain.is_some() {
                        View::Explain
                    } else {
                        View::Query
                    },
                ) {
                    if let Some(bytecode) = explain {
                        self.database.document.explain = super::explain::State {
                            sql: text.clone(),
                            bytecode,
                            raw: false,
                        };
                    }
                    let history_sql = match explain {
                        Some(true) => format!("EXPLAIN {text}"),
                        Some(false) => format!("EXPLAIN QUERY PLAN {text}"),
                        None => text,
                    };
                    // Restoring SQL text from history must not lose a migration guard.
                    if migration.is_none() {
                        self.database.tools.pending_history.insert(
                            ticket.request,
                            (self.database.tools.database_scope.clone(), history_sql),
                        );
                    }
                    self.database.document.table = None;
                    self.database.document.browse = None;
                    self.database.document.tools.related = None;
                }
            }
            Err(e) => self.database.status = format!("Typed bindings: {e}"),
        }
    }
    pub(super) fn run_current(&mut self, explain: Option<bool>) {
        match self.current_text() {
            Ok(text) => self.run_text(text, explain),
            Err(e) => self.database.status = e.to_string(),
        }
    }
    pub(super) fn current_text(&self) -> Result<String, tursobench_core::Failure> {
        let start = byte_index(
            &self.database.document.sql,
            self.database.document.tools.selection.start,
        );
        let end = byte_index(
            &self.database.document.sql,
            self.database.document.tools.selection.end,
        );
        sql::selection_or_statement(&self.database.document.sql, start..end)
    }

    pub(super) fn run_all(&mut self, atomic: bool) {
        if !self.action_enabled(super::ribbon::Action::RunAll) {
            return;
        }
        if self.database.document.migration.is_some() {
            self.run_text(self.database.document.sql.clone(), None);
            return;
        }
        match Bindings::parse_json(&self.database.document.params) {
            Ok(parameters) => {
                let text = self.database.document.sql.clone();
                let operation = Operation::Script {
                    sql: text.clone(),
                    parameters,
                    atomic,
                    budget: Budget {
                        rows: self.preferences.row_limit,
                        ..Default::default()
                    },
                };
                if let Some(ticket) = self.submit(operation, View::Query) {
                    self.database.tools.pending_history.insert(
                        ticket.request,
                        (self.database.tools.database_scope.clone(), text),
                    );
                }
            }
            Err(e) => self.database.status = e.to_string(),
        }
    }
    pub(super) fn run_all_menu(&mut self, ui: &mut egui::Ui, ribbon: bool) -> egui::Response {
        let enabled = self.action_enabled(super::ribbon::Action::RunAll);
        let response = ui
            .add_enabled_ui(enabled, |ui| {
                if ribbon {
                    icons::ribbon_button(ui, Icon::RunAll, "Run all", "", None)
                } else {
                    icons::button(ui, Icon::RunAll, "Run all", "")
                }
            })
            .inner;
        egui::Popup::menu(&response).show(|ui| {
            for (label, atomic) in [
                ("Run statements · stop on error", false),
                ("Run as one atomic transaction", true),
            ] {
                if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                    self.run_all(atomic);
                    ui.close();
                }
            }
        });
        response
    }
    pub(super) fn editor_ui(&mut self, ui: &mut egui::Ui) {
        if self.database.document.migration.is_some() {
            ui.label("Guarded migration · Run all is atomic · target schema must match the comparison · session-only draft");
        }
        let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, width: f32| {
            highlighted(ui, text.as_str(), width)
        };
        let output = egui::ScrollArea::vertical()
            .id_salt("sql-editor-scroll")
            .max_height(if self.database.workspace.enabled {
                (ui.available_height()
                    - if self.database.document.ui.params_open {
                        120.0
                    } else {
                        8.0
                    })
                .max(24.0)
            } else {
                (ui.ctx().content_rect().height() * 0.35).clamp(100.0, 400.0)
            })
            .show(ui, |ui| {
                let empty_size =
                    if self.database.workspace.enabled && self.database.document.sql.is_empty() {
                        ui.available_rect_before_wrap()
                            .intersect(ui.clip_rect())
                            .size()
                    } else {
                        egui::Vec2::ZERO
                    };
                egui::TextEdit::multiline(&mut self.database.document.sql)
                    .id_salt("sql-editor")
                    .code_editor()
                    .min_size(empty_size)
                    .desired_rows(5)
                    .desired_width(f32::INFINITY)
                    .layouter(&mut layouter)
                    .show(ui)
            })
            .inner;
        if self.database.document.sql.is_empty() && !output.response.has_focus() {
            icons::empty_state_at(
                ui,
                output.response.rect.intersect(ui.clip_rect()),
                Icon::Sql,
                "Write SQL · Ctrl Enter to run",
            );
        }
        if output.response.has_focus()
            && let Some(cursor) = output.cursor_range
        {
            let caret = output.galley.pos_from_cursor(cursor.primary);
            let line = egui::Rect::from_min_max(
                egui::pos2(
                    output.response.rect.left(),
                    output.galley_pos.y + caret.top(),
                ),
                egui::pos2(
                    output.response.rect.right(),
                    output.galley_pos.y + caret.bottom(),
                ),
            );
            ui.painter()
                .with_clip_rect(output.response.rect.intersect(ui.clip_rect()))
                .rect_filled(
                    line,
                    0.0,
                    crate::appearance::Tokens::of(ui.ctx())
                        .primary
                        .gamma_multiply(0.06),
                );
        }
        self.database.document.tools.sql_focus = Some(output.response.id);
        if let Some(range) = self.database.document.restore_cursor.take() {
            let mut state = output.state.clone();
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(range.start),
                    egui::text::CCursor::new(range.end),
                )));
            state.store(ui.ctx(), output.response.id);
        } else if let Some(cursor) = output.cursor_range {
            let range = cursor.as_sorted_char_range();
            self.database.document.tools.selection = range.start.0..range.end.0;
        }
        if self.database.document.ui.params_open {
            self.bindings_ui(ui);
        }
        if self.database.document.tools.complete {
            self.completion_ui(ui, output.response.id);
        }
    }
    fn bindings_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label("Typed bindings (JSON):");ui.add(egui::TextEdit::singleline(&mut self.database.document.params).desired_width(ui.available_width().min(420.0)));
            if ui.button("Generate NULL bindings").clicked(){
                let names=sql::parameter_names(&self.database.document.sql);
                if names.iter().any(|n|n.starts_with('?')) {self.database.status="Positional parameters use an array ordered by slot: [{\"type\":\"Null\"}, ...]".into();}
                else {self.database.document.params=serde_json::to_string(&sql::named_template(&self.database.document.sql)).unwrap_or_default();}
            }
        });
        ui.small("?1: [{\"type\":\"Integer\",\"value\":42}]    :id: {\":id\":{\"type\":\"Integer\",\"value\":42}}. Values are never interpolated or persisted.");
    }
    fn completion_ui(&mut self, ui: &mut egui::Ui, editor_id: egui::Id) {
        let cursor = byte_index(
            &self.database.document.sql,
            self.database.document.tools.selection.end,
        );
        let Some(span) = completion_span(&self.database.document.sql, cursor) else {
            ui.small("Completion unavailable here: quoted/comment/parameter text, a lexical error, or a buffer above 64 KiB.");
            return;
        };
        let start = span.start;
        let mut catalog = tursobench_db::completion::Catalog::new();
        for object in &self.database.schema {
            if object.kind == "table" || object.kind == "view" {
                catalog.entry(object.name.clone()).or_default();
            }
        }
        if let Some(graph) = &self.database.ui.diagram.data {
            for table in &graph.tables {
                if let Some(info) = &table.inspection {
                    catalog.insert(
                        table.name.clone(),
                        info.table.columns.iter().map(|c| c.name.clone()).collect(),
                    );
                }
            }
        }
        if let Some(info) = &self.database.document.tools.inspection {
            catalog.insert(
                info.table.name.clone(),
                info.table.columns.iter().map(|c| c.name.clone()).collect(),
            );
        }
        let candidates = tursobench_db::completion::candidates(
            &self.database.document.sql,
            cursor,
            start,
            &catalog,
        );
        ui.small("Context completions from cached schema: aliases, qualified columns and CTEs. Unknown expressions remain unresolved.");
        ui.horizontal_wrapped(|ui| {
            for candidate in candidates.into_iter().take(30) {
                if ui.button(candidate.as_str()).clicked() {
                    self.database
                        .document
                        .sql
                        .replace_range(span.clone(), &candidate);
                    let position = self.database.document.sql[..start].chars().count()
                        + candidate.chars().count();
                    self.database.document.tools.selection = position..position;
                    if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), editor_id) {
                        state
                            .cursor
                            .set_char_range(Some(egui::text::CCursorRange::one(
                                egui::text::CCursor::new(position),
                            )));
                        state.store(ui.ctx(), editor_id);
                        ui.memory_mut(|m| m.request_focus(editor_id));
                    }
                    self.database.document.tools.complete = false;
                }
            }
        });
    }
}
