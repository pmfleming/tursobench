//! The assistant panel sits beside the sidebar; each can be hidden on its own.
//! It drives the user's own signed-in agent CLI (Claude Code by default, or an
//! Agent Client Protocol agent) and offers it read-only tools for the database
//! in this workspace. TursoBench never handles credentials.
use super::{
    App,
    icons::{self, Icon},
    presentation::{self, Intent},
};
use crate::assistant_tools::{self as tools, Host, Proposal, Sharing};
use eframe::egui;
use serde_json::Value as Json;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
};
use tursobench_assistant::{
    agent::{Agent, Protocol, Spec, Update},
    claude, codex,
    relay::{self, Endpoint},
};

const MAX_ENTRIES: usize = 400;
const MAX_TOOL_TEXT: usize = 4096;

pub(super) enum Entry {
    User(String),
    Reply(String),
    Thought(String),
    Tool {
        id: String,
        name: String,
        input: Json,
        result: Option<(bool, String)>,
    },
    Notice(Intent, String),
}

struct Running {
    // Field order is drop order: stop the agent before its tool endpoint.
    agent: Agent,
    host: Arc<Host>,
    proposals: mpsc::Receiver<Proposal>,
    _endpoint: Endpoint,
}

/// One workspace's conversation, kept while its database stays the same.
#[derive(Default)]
pub(super) struct Conversation {
    database: String,
    session: Arc<Mutex<Option<tursobench_db::assistant::Handle>>>,
    pub(super) entries: Vec<Entry>,
    pub(super) draft: String,
    running: Option<Running>,
    pub(super) busy: bool,
    /// The running agent's name, kept for the header while it runs.
    label: Option<String>,
    /// The command that signs this agent in, for error hints.
    sign_in: Option<String>,
    pub(super) model: Option<String>,
    focus: bool,
    /// Told to the agent with the next prompt after the user changes sharing.
    note: Option<String>,
    /// Tests start a scripted agent in place of `claude`.
    #[cfg(test)]
    pub(super) program: Option<PathBuf>,
}
impl Conversation {
    /// Bind to a newly opened session; a different database starts afresh.
    pub(super) fn attach(&mut self, database: &str, handle: tursobench_db::assistant::Handle) {
        if self.database != database {
            self.reset();
            self.database = database.into();
        }
        *self.session.lock().unwrap() = Some(handle);
    }
    fn reset(&mut self) {
        self.running = None;
        self.label = None;
        self.entries.clear();
        self.busy = false;
        self.model = None;
        self.note = None;
    }
    fn push(&mut self, entry: Entry) {
        self.entries.push(entry);
        if self.entries.len() > MAX_ENTRIES {
            self.entries.remove(0);
        }
    }
    fn append_reply(&mut self, text: &str, thought: bool) {
        match self.entries.last_mut() {
            Some(Entry::Reply(reply)) if !thought => reply.push_str(text),
            Some(Entry::Thought(reply)) if thought => reply.push_str(text),
            _ => self.push(if thought {
                Entry::Thought(text.into())
            } else {
                Entry::Reply(text.into())
            }),
        }
    }
    #[cfg(test)]
    pub(super) fn host(&self) -> Option<Arc<Host>> {
        self.running.as_ref().map(|r| r.host.clone())
    }
}

enum Action {
    OpenSql(String),
    Copy(String),
}

impl App {
    fn assistant_database(&self) -> Option<String> {
        (!self.database.tools.database_scope.is_empty()
            && (self.database.session.is_some() || !self.database.assistant.entries.is_empty()))
        .then(|| self.database.tools.database_scope.clone())
    }
    fn assistant_sharing(&self) -> Option<Sharing> {
        let database = self.assistant_database()?;
        self.preferences.assistant.sharing.get(&database).copied()
    }
    pub(super) fn set_assistant_sharing(&mut self, sharing: Sharing) {
        let Some(database) = self.assistant_database() else {
            return;
        };
        let previous = self.preferences.assistant.sharing.insert(database, sharing);
        self.preferences.normalize();
        self.store.save(&self.preferences);
        let conversation = &mut self.database.assistant;
        if let Some(running) = &conversation.running {
            running.host.set_sharing(sharing);
        }
        if previous.is_some() && previous != Some(sharing) {
            let (note, text) = match sharing {
                Sharing::Schema => (
                    "The user now shares only the schema; run_query is refused.",
                    "Now sharing only the schema",
                ),
                Sharing::Rows => (
                    "The user now also shares query results; run_query is available.",
                    "Now sharing the schema and query results",
                ),
            };
            conversation.note = Some(note.into());
            conversation.push(Entry::Notice(Intent::Neutral, text.into()));
        }
    }

    pub(super) fn assistant_focus(&mut self) {
        self.database.assistant.focus = true;
    }

    pub(super) fn poll_assistant(&mut self) {
        let Some(running) = &self.database.assistant.running else {
            return;
        };
        let updates: Vec<_> = running.agent.updates.try_iter().collect();
        let proposals: Vec<_> = running.proposals.try_iter().collect();
        for update in updates {
            self.assistant_update(update);
        }
        for proposal in proposals {
            self.load_sql_window(proposal.sql);
            self.database.assistant.push(Entry::Notice(
                Intent::Primary,
                "Opened proposed SQL in an editor tile. Review it before running.".into(),
            ));
        }
    }

    fn assistant_update(&mut self, update: Update) {
        let conversation = &mut self.database.assistant;
        match update {
            Update::Ready { model } => conversation.model = model,
            Update::Text(text) => conversation.append_reply(&text, false),
            Update::Thought(text) => conversation.append_reply(&text, true),
            Update::ToolCall { id, name, input } => conversation.push(Entry::Tool {
                id,
                name,
                input,
                result: None,
            }),
            Update::ToolResult { id, ok, text } => {
                if let Some(Entry::Tool { result, .. }) = conversation
                    .entries
                    .iter_mut()
                    .rev()
                    .find(|e| matches!(e, Entry::Tool { id: tool, .. } if *tool == id))
                {
                    *result = Some((ok, text));
                }
            }
            Update::TurnEnded { error } => {
                conversation.busy = false;
                if let Some(error) = error {
                    let hint = explain(&error, conversation.sign_in.as_deref());
                    conversation.push(Entry::Notice(Intent::Danger, hint));
                }
            }
            Update::Notice(text) => conversation.push(Entry::Notice(Intent::Pending, text)),
            Update::Exited { message } => {
                conversation.running = None;
                conversation.busy = false;
                let name = conversation
                    .label
                    .clone()
                    .unwrap_or_else(|| "The agent".into());
                let hint = explain(&message, conversation.sign_in.as_deref());
                conversation.push(Entry::Notice(
                    Intent::Danger,
                    format!("{name} stopped. {hint}"),
                ));
            }
        }
    }

    fn start_assistant(&mut self, ctx: &egui::Context, sharing: Sharing) -> Result<(), String> {
        let database = self.database.tools.database_scope.clone();
        let name = std::path::Path::new(&database)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| database.clone());
        let wake = ctx.clone();
        let wake: tursobench_assistant::agent::Wake = Arc::new(move || wake.request_repaint());
        let (tx, proposals) = mpsc::channel();
        let host = Arc::new(Host::new(
            self.database.assistant.session.clone(),
            sharing,
            tx,
            wake.clone(),
        ));
        let relay_program = std::env::current_exe()
            .map_err(|e| format!("TursoBench could not find its own program: {e}"))?;
        let endpoint = Endpoint::start(host.clone(), relay_program)
            .map_err(|e| format!("Could not start the assistant's tools: {e}"))?;
        let cwd = relay::private_dir().map_err(|e| e.to_string())?;
        let profile = profile(&self.preferences.assistant)?;
        #[cfg(test)]
        let program = self
            .database
            .assistant
            .program
            .clone()
            .unwrap_or_else(|| profile.program.clone());
        #[cfg(not(test))]
        let program = profile.program.clone();
        let model = self.preferences.assistant.model.trim().to_owned();
        let model = (!model.is_empty()).then_some(model.as_str());
        let prompt = tools::system_prompt(&name, sharing);
        let spec = match profile.protocol {
            Protocol::ClaudeStream => claude::spec(
                program,
                cwd,
                endpoint.config_path(),
                &tools::NAMES,
                &prompt,
                model,
            ),
            Protocol::CodexAppServer => {
                codex::spec(program, cwd, endpoint.launch().clone(), &prompt, model)
            }
            Protocol::Acp => Spec {
                program,
                args: profile.args.clone(),
                env: profile.env.clone(),
                cwd,
                protocol: Protocol::Acp,
                system_prompt: prompt,
                mcp: Some(endpoint.launch().clone()),
                model: model.map(str::to_owned),
            },
        };
        let agent = Agent::spawn(spec, wake).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                profile.missing.clone()
            } else {
                format!("Could not start {}: {e}", profile.name)
            }
        })?;
        self.database.assistant.label = Some(profile.name.clone());
        self.database.assistant.sign_in = Some(profile.sign_in.clone());
        self.database.assistant.running = Some(Running {
            agent,
            host,
            proposals,
            _endpoint: endpoint,
        });
        Ok(())
    }

    pub(super) fn send_assistant(&mut self, ctx: &egui::Context) {
        let text = self.database.assistant.draft.trim().to_owned();
        if text.is_empty() || self.database.assistant.busy {
            return;
        }
        let Some(sharing) = self.assistant_sharing() else {
            return;
        };
        if self.database.session.is_none() {
            self.database.assistant.push(Entry::Notice(
                Intent::Danger,
                "Reopen the database to continue the conversation".into(),
            ));
            return;
        }
        if self.database.assistant.running.is_none()
            && let Err(error) = self.start_assistant(ctx, sharing)
        {
            self.database
                .assistant
                .push(Entry::Notice(Intent::Danger, error));
            return;
        }
        let conversation = &mut self.database.assistant;
        conversation.draft.clear();
        let prompt = match conversation.note.take() {
            Some(note) => format!("[TursoBench: {note}]\n\n{text}"),
            None => text.clone(),
        };
        conversation.push(Entry::User(text));
        conversation.busy = true;
        if let Some(running) = &conversation.running {
            running.agent.prompt(prompt);
        }
    }

    pub(super) fn assistant_ui(&mut self, ui: &mut egui::Ui) {
        self.poll_assistant();
        self.assistant_header(ui);
        ui.separator();
        let Some(_) = self.assistant_database() else {
            icons::empty_state(
                ui,
                Icon::Assistant,
                "Open a database, then ask your signed-in agent about it here",
            );
            return;
        };
        let Some(sharing) = self.assistant_sharing() else {
            self.assistant_consent(ui);
            return;
        };
        egui::Panel::bottom("assistant-input")
            .resizable(false)
            .show(ui, |ui| self.assistant_input(ui));
        let mut actions = vec![];
        egui::ScrollArea::vertical()
            .id_salt("assistant-conversation")
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                if self.database.assistant.entries.is_empty() {
                    let hint = match sharing {
                        Sharing::Schema => "Ask about tables, keys or how to write a query",
                        Sharing::Rows => "Ask about the schema or the data",
                    };
                    icons::empty_state(ui, Icon::Assistant, hint);
                }
                for (index, entry) in self.database.assistant.entries.iter().enumerate() {
                    ui.push_id(index, |ui| entry_ui(ui, entry, &mut actions));
                    ui.add_space(6.0);
                }
                if self.database.assistant.busy {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.weak("Working…");
                    });
                }
            });
        for action in actions {
            match action {
                Action::OpenSql(sql) => self.load_sql_window(sql),
                Action::Copy(text) => ui.ctx().copy_text(text),
            }
        }
    }

    fn assistant_header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            let (rect, _) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::hover());
            icons::paint(
                ui.painter(),
                rect,
                Icon::Assistant,
                ui.visuals().text_color(),
            );
            let model = self.database.assistant.model.clone();
            let name = self
                .database
                .assistant
                .label
                .clone()
                .or_else(|| profile(&self.preferences.assistant).ok().map(|p| p.name))
                .unwrap_or_else(|| "Assistant".into());
            ui.strong(&name).on_hover_text(match &model {
                Some(model) => format!("Your own signed-in {name} · {model}"),
                None => format!(
                    "Your own signed-in {name}. TursoBench never sees your sign-in. Choose the agent in Settings → Assistant."
                ),
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icons::control(ui, Icon::Close, "Hide assistant", "Ctrl I", false, false)
                    .clicked()
                {
                    self.toggle_assistant();
                }
                if let Some(sharing) = self.assistant_sharing() {
                    let (icon, name, next) = match sharing {
                        Sharing::Schema => (
                            Icon::Structure,
                            "Sharing the schema only · click to also share query results",
                            Sharing::Rows,
                        ),
                        Sharing::Rows => (
                            Icon::Data,
                            "Sharing the schema and query results · click to share the schema only",
                            Sharing::Schema,
                        ),
                    };
                    if icons::control(ui, icon, name, "", sharing == Sharing::Rows, false).clicked()
                    {
                        self.set_assistant_sharing(next);
                    }
                }
                let conversation = &self.database.assistant;
                if ui
                    .add_enabled_ui(
                        !conversation.entries.is_empty() || conversation.running.is_some(),
                        |ui| icons::control(ui, Icon::Plus, "New conversation", "", false, false),
                    )
                    .inner
                    .clicked()
                {
                    self.database.assistant.reset();
                    self.database.assistant.focus = true;
                }
            });
        });
    }

    fn assistant_consent(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        let text = match profile(&self.preferences.assistant) {
            Ok(profile) => {
                let mut text = format!(
                    "The assistant runs your own signed-in {} and uses your plan. What it \
                     reads from this database is sent to {}. It cannot change the \
                     database; it proposes SQL for you to review.",
                    profile.name, profile.vendor
                );
                match profile.protocol {
                    Protocol::Acp => text.push_str(
                        " This agent also has tools of its own: TursoBench declines its \
                         requests to run commands or edit files, but some agents can read \
                         local files without asking.",
                    ),
                    Protocol::CodexAppServer => text.push_str(
                        " Codex's shell and web search are switched off and it runs in its \
                         read-only sandbox; TursoBench declines its requests to run \
                         commands or edit files.",
                    ),
                    Protocol::ClaudeStream => {}
                }
                text
            }
            Err(error) => error,
        };
        presentation::notice(ui, Icon::Lock, Intent::Neutral, &text);
        ui.add_space(8.0);
        ui.label("Choose what this database shares:");
        ui.add_space(4.0);
        if ui
            .add(presentation::action(ui, "Schema only", Intent::Primary))
            .on_hover_text("Table and column names, keys, indexes and CREATE statements")
            .clicked()
        {
            self.set_assistant_sharing(Sharing::Schema);
            self.database.assistant.focus = true;
        }
        ui.add_space(4.0);
        if ui
            .add(presentation::action(
                ui,
                "Schema and query results",
                Intent::Neutral,
            ))
            .on_hover_text(
                "Also lets it run read-only SELECT queries of at most 200 rows and read the results",
            )
            .clicked()
        {
            self.set_assistant_sharing(Sharing::Rows);
            self.database.assistant.focus = true;
        }
        ui.add_space(8.0);
        ui.weak("Change it later with the sharing icon above.");
    }

    fn assistant_input(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        let id = ui.make_persistent_id("assistant-input");
        // Enter sends; Shift Enter keeps its newline.
        let send = ui.memory(|m| m.has_focus(id))
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
        let busy = self.database.assistant.busy;
        let edit = egui::TextEdit::multiline(&mut self.database.assistant.draft)
            .id(id)
            .desired_rows(2)
            .desired_width(f32::INFINITY)
            .hint_text("Ask about this database · Enter sends");
        let response = ui.add(edit);
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::Role::MultilineTextInput,
                true,
                "Message the assistant",
            )
        });
        if std::mem::take(&mut self.database.assistant.focus) {
            response.request_focus();
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            ui.weak("Reads only · proposes SQL for you to run")
                .on_hover_text("Tools: list tables, describe a table, read-only SELECT when shared, and open SQL in an editor tile");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if busy {
                    if icons::control(ui, Icon::Stop, "Stop", "", false, false).clicked()
                        && let Some(running) = &self.database.assistant.running
                    {
                        running.agent.cancel();
                    }
                } else {
                    let empty = self.database.assistant.draft.trim().is_empty();
                    if ui
                        .add_enabled_ui(!empty, |ui| {
                            icons::control(ui, Icon::Send, "Send", "Enter", false, false)
                        })
                        .inner
                        .clicked()
                    {
                        self.send_assistant(ui.ctx());
                    }
                }
            });
        });
        if send && !busy {
            self.send_assistant(ui.ctx());
        }
    }
}

/// Add a pointer to the fix for the errors people hit most.
fn explain(error: &str, sign_in: Option<&str>) -> String {
    let lower = error.to_lowercase();
    let asks = ["login", "log in", "sign in", "authenticat", "unauthori"];
    if asks.iter().any(|w| lower.contains(w)) && !lower.contains("then try again") {
        let command = sign_in.unwrap_or("the agent's CLI");
        format!("{error}\nSign in by running `{command}` in a terminal, then send again.")
    } else if error.trim().is_empty() {
        "The agent gave no reason.".into()
    } else {
        error.into()
    }
}

fn entry_ui(ui: &mut egui::Ui, entry: &Entry, actions: &mut Vec<Action>) {
    let tokens = crate::appearance::Tokens::of(ui.ctx());
    match entry {
        Entry::User(text) => {
            egui::Frame::new()
                .fill(tokens.raised)
                .corner_radius(8)
                .inner_margin(8.0)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.add(egui::Label::new(text).wrap());
                });
        }
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

fn tool_ui(
    ui: &mut egui::Ui,
    name: &str,
    input: &Json,
    result: Option<&(bool, String)>,
    actions: &mut Vec<Action>,
) {
    let tokens = crate::appearance::Tokens::of(ui.ctx());
    let argument = |key: &str| input.get(key).and_then(Json::as_str).unwrap_or("");
    let (icon, summary) = match name {
        tools::LIST_TABLES => (Icon::Data, "Listed tables".to_owned()),
        tools::DESCRIBE_TABLE => (Icon::Structure, format!("Described {}", argument("name"))),
        tools::RUN_QUERY => (Icon::Run, first_line(argument("sql"))),
        tools::PROPOSE_SQL => (Icon::Sql, "Proposed SQL".to_owned()),
        other => (Icon::Command, other.to_owned()),
    };
    let color = match result {
        None => tokens.muted,
        Some((true, _)) => tokens.primary,
        Some((false, _)) => ui.visuals().error_fg_color,
    };
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(2, 0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (rect, response) =
                    ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
                icons::paint(ui.painter(), rect, icon, color);
                response.on_hover_text(match result {
                    None => format!("{name} · running"),
                    Some((true, _)) => format!("{name} · done"),
                    Some((false, _)) => format!("{name} · failed"),
                });
                let mono = name == tools::RUN_QUERY;
                let text = if mono {
                    egui::RichText::new(summary).monospace().small()
                } else {
                    egui::RichText::new(summary).small()
                };
                egui::CollapsingHeader::new(text)
                    .id_salt("tool")
                    .default_open(false)
                    .show(ui, |ui| {
                        let sql = argument("sql");
                        if !sql.is_empty() {
                            code_block(ui, sql, true, actions);
                        }
                        match result {
                            None => {
                                ui.spinner();
                            }
                            Some((_, text)) => {
                                let shown: String = text.chars().take(MAX_TOOL_TEXT).collect();
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(shown).monospace().small(),
                                    )
                                    .wrap(),
                                );
                            }
                        }
                    });
                if result.is_none() {
                    ui.spinner();
                }
            });
        });
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
    let mut lines = text.lines().peekable();
    let mut paragraph: Vec<&str> = vec![];
    let flush = |ui: &mut egui::Ui, paragraph: &mut Vec<&str>| {
        if !paragraph.is_empty() {
            inline(ui, &paragraph.join("\n"), false);
            paragraph.clear();
        }
    };
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        if let Some(language) = trimmed.strip_prefix("```") {
            flush(ui, &mut paragraph);
            let mut code = vec![];
            for next in lines.by_ref() {
                if next.trim_start().starts_with("```") {
                    break;
                }
                code.push(next);
            }
            let language = language.trim().to_lowercase();
            let sql = language.is_empty() || language == "sql" || language == "sqlite";
            code_block(ui, &code.join("\n"), sql, actions);
        } else if let Some(heading) = trimmed
            .strip_prefix("### ")
            .or_else(|| trimmed.strip_prefix("## "))
            .or_else(|| trimmed.strip_prefix("# "))
        {
            flush(ui, &mut paragraph);
            inline(ui, heading, true);
        } else if let Some(item) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            flush(ui, &mut paragraph);
            ui.horizontal_top(|ui| {
                ui.label("•");
                inline(ui, item, false);
            });
        } else if trimmed.is_empty() {
            flush(ui, &mut paragraph);
            ui.add_space(4.0);
        } else {
            paragraph.push(line);
        }
    }
    flush(ui, &mut paragraph);
}

fn inline(ui: &mut egui::Ui, text: &str, heading: bool) {
    let mut job = egui::text::LayoutJob::default();
    let body = crate::appearance::proportional(ui.ctx(), if heading { 15.0 } else { 14.0 });
    let mono = crate::appearance::monospace(ui.ctx(), 13.0);
    let color = ui.visuals().text_color();
    let strong = ui.visuals().strong_text_color();
    let code_fill = crate::appearance::Tokens::of(ui.ctx()).raised;
    let mut bold = heading;
    for (index, part) in text.split('`').enumerate() {
        if index % 2 == 1 {
            job.append(
                part,
                0.0,
                egui::TextFormat {
                    font_id: mono.clone(),
                    color,
                    background: code_fill,
                    ..Default::default()
                },
            );
            continue;
        }
        for (piece, segment) in part.split("**").enumerate() {
            if piece > 0 {
                bold = !bold;
            }
            job.append(
                segment,
                0.0,
                egui::TextFormat {
                    font_id: body.clone(),
                    color: if bold { strong } else { color },
                    ..Default::default()
                },
            );
        }
    }
    job.wrap.max_width = ui.available_width();
    ui.add(egui::Label::new(job).wrap());
}

fn code_block(ui: &mut egui::Ui, code: &str, sql: bool, actions: &mut Vec<Action>) {
    let tokens = crate::appearance::Tokens::of(ui.ctx());
    egui::Frame::new()
        .fill(tokens.raised)
        .corner_radius(6)
        .inner_margin(6.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    if icons::control(ui, Icon::Copy, "Copy", "", false, false).clicked() {
                        actions.push(Action::Copy(code.into()));
                    }
                    if sql
                        && icons::control(
                            ui,
                            Icon::Sql,
                            "Open in a new SQL editor tile (not run)",
                            "",
                            false,
                            false,
                        )
                        .clicked()
                    {
                        actions.push(Action::OpenSql(code.into()));
                    }
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                        ui.add(
                            egui::Label::new(egui::RichText::new(code).monospace().small()).wrap(),
                        );
                    });
                });
            });
        });
}

/// How to start one kind of agent and what to tell the user about it.
pub(super) struct Profile {
    pub name: String,
    /// Who receives what the agent reads.
    pub vendor: &'static str,
    program: PathBuf,
    args: Vec<String>,
    env: Vec<(String, String)>,
    protocol: Protocol,
    /// Shown when the program is not installed.
    missing: String,
    /// The command that signs in, for error hints.
    sign_in: String,
}

pub(super) fn profile(
    preferences: &crate::storage::AssistantPreferences,
) -> Result<Profile, String> {
    use crate::storage::Provider;
    Ok(match preferences.provider {
        Provider::Claude => Profile {
            name: "Claude Code".into(),
            vendor: "Anthropic",
            program: "claude".into(),
            args: vec![],
            env: vec![],
            protocol: Protocol::ClaudeStream,
            missing: "Claude Code (`claude`) was not found on PATH. Install it, run `claude` \
                      once in a terminal to sign in, then try again."
                .into(),
            sign_in: "claude".into(),
        },
        Provider::Copilot => Profile {
            name: "Copilot CLI".into(),
            vendor: "GitHub",
            program: "copilot".into(),
            args: vec!["--acp".into()],
            env: vec![],
            protocol: Protocol::Acp,
            missing: "GitHub Copilot CLI (`copilot`) was not found on PATH. Install it, run \
                      `copilot` once in a terminal to sign in, then try again."
                .into(),
            sign_in: "copilot".into(),
        },
        Provider::Codex => Profile {
            name: "Codex".into(),
            vendor: "OpenAI",
            program: "codex".into(),
            args: vec![],
            env: vec![],
            protocol: Protocol::CodexAppServer,
            missing: "Codex (`codex`) was not found on PATH. Install the Codex CLI, sign in \
                      with `codex login`, then try again."
                .into(),
            sign_in: "codex login".into(),
        },
        Provider::Custom => {
            let mut words = preferences.custom_command.split_whitespace();
            let Some(program) = words.next() else {
                return Err(
                    "Enter the agent's command in Settings → Assistant, for example \
                     `opencode acp`."
                        .into(),
                );
            };
            Profile {
                name: std::path::Path::new(program)
                    .file_name()
                    .map_or_else(|| program.into(), |n| n.to_string_lossy().into_owned()),
                vendor: "the agent's provider",
                program: program.into(),
                args: words.map(str::to_owned).collect(),
                env: vec![],
                protocol: Protocol::Acp,
                missing: format!("`{program}` was not found. Check the command in Settings."),
                sign_in: program.into(),
            }
        }
    })
}

impl App {
    pub(super) fn assistant_settings(&mut self, ui: &mut egui::Ui) {
        use crate::storage::Provider;
        ui.spacing_mut().item_spacing.y = 4.0;
        super::panels::section(ui, "Agent", |ui| {
            for (provider, label, detail) in [
                (
                    Provider::Claude,
                    "Claude Code",
                    "claude · Anthropic · its built-in tools are removed",
                ),
                (Provider::Copilot, "Copilot CLI", "copilot --acp · GitHub"),
                (
                    Provider::Codex,
                    "Codex",
                    "codex app-server · OpenAI · read-only sandbox, no shell",
                ),
                (
                    Provider::Custom,
                    "Other ACP agent",
                    "Any Agent Client Protocol command",
                ),
            ] {
                ui.horizontal(|ui| {
                    let response =
                        ui.radio_value(&mut self.preferences.assistant.provider, provider, label);
                    self.setting_response(response);
                    ui.weak(detail);
                });
            }
            if self.preferences.assistant.provider == Provider::Custom {
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.preferences.assistant.custom_command)
                        .hint_text("opencode acp")
                        .desired_width(f32::INFINITY),
                );
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Agent command")
                });
                self.setting_response(response);
            }
            ui.horizontal(|ui| {
                ui.label("Model")
                    .on_hover_text("Passed to the agent; leave empty for its own default");
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.preferences.assistant.model)
                        .hint_text("Agent default")
                        .desired_width(f32::INFINITY),
                );
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Assistant model")
                });
                self.setting_response(response);
            });
            ui.weak(
                "Applies to the next conversation. Each CLI signs in by itself; TursoBench \
                 never reads or stores sign-ins.",
            );
        });
        super::panels::section(ui, "Sharing", |ui| {
            if self.preferences.assistant.sharing.is_empty() {
                ui.weak("No database shares anything yet. Each one asks first.");
                return;
            }
            let mut forget = None;
            for (path, sharing) in &self.preferences.assistant.sharing {
                ui.horizontal(|ui| {
                    let (icon, name) = match sharing {
                        Sharing::Schema => (Icon::Structure, "Schema only"),
                        Sharing::Rows => (Icon::Data, "Schema and query results"),
                    };
                    let (rect, response) =
                        ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
                    icons::paint(ui.painter(), rect, icon, ui.visuals().text_color());
                    response.on_hover_text(name);
                    let file = std::path::Path::new(path)
                        .file_name()
                        .map_or_else(|| path.clone(), |n| n.to_string_lossy().into_owned());
                    ui.label(file).on_hover_text(path);
                    ui.weak(name);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if icons::control(ui, Icon::Trash, "Forget; ask again", "", false, false)
                            .clicked()
                        {
                            forget = Some(path.clone());
                        }
                    });
                });
            }
            if let Some(path) = forget {
                self.preferences.assistant.sharing.remove(&path);
                if path == self.database.tools.database_scope {
                    self.database.assistant.reset();
                }
                self.preferences.normalize();
                self.store.save(&self.preferences);
            }
        });
    }
}
