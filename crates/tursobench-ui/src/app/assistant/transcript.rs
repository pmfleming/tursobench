//! The conversation as the panel draws it: messages, tool calls, notices and
//! replies in the chat's Markdown subset, with actions on their SQL.
use super::Entry;
use crate::app::{
    assistant_export::{Block, Span, blocks, spans},
    icons::{self, Icon},
    presentation,
};
use crate::appearance::{self, Tokens};
use crate::assistant_tools as tools;
use eframe::egui;
use serde_json::Value as Json;

const MAX_TOOL_TEXT: usize = 4096;

/// What a click in the conversation asks of the app.
pub(super) enum Action {
    OpenSql(String),
    Copy(String),
}

pub(super) fn entry_ui(ui: &mut egui::Ui, entry: &Entry, actions: &mut Vec<Action>) {
    match entry {
        Entry::User(text) => raised(ui, 8, |ui| {
            ui.add(egui::Label::new(text).wrap());
        }),
        Entry::Reply(text) => markdown(ui, text, actions),
        Entry::Thought(text) => {
            let short: String = text.chars().take(240).collect();
            ui.add(egui::Label::new(egui::RichText::new(short).italics().weak().small()).wrap())
                .on_hover_text(text);
        }
        Entry::Tool {
            name,
            input,
            result,
            ..
        } => tool_ui(ui, name, input, result.as_ref(), actions),
        Entry::Notice(intent, text) => presentation::notice(ui, Icon::Assistant, *intent, text),
    }
}

/// A tool call's icon and summary; its SQL and result fold out below.
fn tool_ui(
    ui: &mut egui::Ui,
    name: &str,
    input: &Json,
    result: Option<&(bool, String)>,
    actions: &mut Vec<Action>,
) {
    let tokens = Tokens::of(ui.ctx());
    let argument = |key: &str| input.get(key).and_then(Json::as_str).unwrap_or("");
    let (icon, summary) = match name {
        tools::LIST_TABLES => (Icon::Data, "Listed tables".to_owned()),
        tools::DESCRIBE_TABLE => (Icon::Structure, format!("Described {}", argument("name"))),
        tools::RUN_QUERY => (Icon::Run, first_line(argument("sql"))),
        tools::PROPOSE_SQL => (Icon::Sql, "Proposed SQL".to_owned()),
        other => (Icon::Command, other.to_owned()),
    };
    let (color, state) = match result {
        None => (tokens.muted, "running"),
        Some((true, _)) => (tokens.primary, "done"),
        Some((false, _)) => (ui.visuals().error_fg_color, "failed"),
    };
    let text = if name == tools::RUN_QUERY {
        egui::RichText::new(summary).monospace().small()
    } else {
        egui::RichText::new(summary).small()
    };
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(2, 0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (rect, response) =
                    ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
                icons::paint(ui.painter(), rect, icon, color);
                response.on_hover_text(format!("{name} · {state}"));
                egui::CollapsingHeader::new(text)
                    .id_salt("tool")
                    .default_open(false)
                    .show(ui, |ui| tool_details(ui, argument("sql"), result, actions));
                if result.is_none() {
                    ui.spinner();
                }
            });
        });
}

/// A tool call's SQL, then its result, or a spinner while it runs.
fn tool_details(
    ui: &mut egui::Ui,
    sql: &str,
    result: Option<&(bool, String)>,
    actions: &mut Vec<Action>,
) {
    if !sql.is_empty() {
        code_block(ui, sql, true, actions);
    }
    match result {
        None => {
            ui.spinner();
        }
        Some((_, text)) => {
            let shown: String = text.chars().take(MAX_TOOL_TEXT).collect();
            ui.add(egui::Label::new(egui::RichText::new(shown).monospace().small()).wrap());
        }
    }
}

fn first_line(sql: &str) -> String {
    let line = sql.trim().lines().next().unwrap_or("").trim();
    let mut short: String = line.chars().take(60).collect();
    if short.len() < line.len() || sql.trim().lines().nth(1).is_some() {
        short.push('…');
    }
    short
}

/// A small Markdown subset: fenced code, headings, bullets, `code` and **bold**.
fn markdown(ui: &mut egui::Ui, text: &str, actions: &mut Vec<Action>) {
    let mut paragraph: Vec<&str> = vec![];
    for block in blocks(text) {
        if !matches!(block, Block::Text(_)) && !paragraph.is_empty() {
            inline(ui, &paragraph.join("\n"), false);
            paragraph.clear();
        }
        match block {
            Block::Code { language, code } => {
                let sql = matches!(language.as_str(), "" | "sql" | "sqlite");
                code_block(ui, &code, sql, actions);
            }
            Block::Heading(_, heading) => inline(ui, heading, true),
            Block::Item(item) => {
                ui.horizontal_top(|ui| {
                    ui.label("•");
                    inline(ui, item, false);
                });
            }
            Block::Blank => ui.add_space(4.0),
            Block::Text(line) => paragraph.push(line),
        }
    }
    if !paragraph.is_empty() {
        inline(ui, &paragraph.join("\n"), false);
    }
}

fn inline(ui: &mut egui::Ui, text: &str, heading: bool) {
    let mut job = egui::text::LayoutJob::default();
    let body = appearance::proportional(ui.ctx(), if heading { 15.0 } else { 14.0 });
    let code = egui::TextFormat {
        font_id: appearance::monospace(ui.ctx(), 13.0),
        color: ui.visuals().text_color(),
        background: Tokens::of(ui.ctx()).raised,
        ..Default::default()
    };
    let plain = egui::TextFormat {
        font_id: body,
        color: ui.visuals().text_color(),
        ..Default::default()
    };
    let strong = egui::TextFormat {
        color: ui.visuals().strong_text_color(),
        ..plain.clone()
    };
    for (span, text) in spans(text, heading) {
        let format = match span {
            Span::Plain => &plain,
            Span::Bold => &strong,
            Span::Code => &code,
        };
        job.append(text, 0.0, format.clone());
    }
    job.wrap.max_width = ui.available_width();
    ui.add(egui::Label::new(job).wrap());
}

/// A full-width box on the raised surface, `radius` round and inset.
fn raised(ui: &mut egui::Ui, radius: u8, contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(Tokens::of(ui.ctx()).raised)
        .corner_radius(radius)
        .inner_margin(f32::from(radius))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            contents(ui);
        });
}

fn code_block(ui: &mut egui::Ui, code: &str, sql: bool, actions: &mut Vec<Action>) {
    raised(ui, 6, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                let mut action = |icon, name, wanted: fn(String) -> Action| {
                    if icons::control(ui, icon, name, "", false, false).clicked() {
                        actions.push(wanted(code.into()));
                    }
                };
                action(Icon::Copy, "Copy", Action::Copy);
                if sql {
                    action(
                        Icon::Sql,
                        "Open in a new SQL editor tile (not run)",
                        Action::OpenSql,
                    );
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                    ui.add(egui::Label::new(egui::RichText::new(code).monospace().small()).wrap());
                });
            });
        });
    });
}
