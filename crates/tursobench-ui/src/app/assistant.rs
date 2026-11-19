//! The assistant panel sits on the right; it hides independently of navigation.
//! It drives the user's own signed-in agent CLI (Claude Code by default, or an
//! Agent Client Protocol agent) and offers it read-only tools for the database
//! in this workspace. TursoBench never handles credentials.
use super::{
    App,
    assistant_accounts::{AgentGrid, Column, status_dot},
    assistant_export,
    icons::{self, Icon},
    presentation::{self, Intent},
};
use crate::assistant_tools::{self as tools, Host, Proposal, Sharing};
use eframe::egui;
use serde_json::Value as Json;
use std::sync::{Arc, Mutex, mpsc};
use transcript::{Action, entry_ui};
use tursobench_assistant::{
    agent::{Agent, Protocol, Spec, Update, Wake},
    claude, codex,
    relay::{self, Endpoint},
};

mod transcript;

const MAX_ENTRIES: usize = 400;

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
    pub(super) program: Option<std::path::PathBuf>,
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
    /// Use `effort` from the running agent's next message.
    pub(super) fn set_effort(&self, effort: Option<String>) {
        if let Some(running) = &self.running {
            running.agent.set_effort(effort);
        }
    }
    #[cfg(test)]
    pub(super) fn host(&self) -> Option<Arc<Host>> {
        self.running.as_ref().map(|r| r.host.clone())
    }
}

/// Apply a sharing change to a conversation's agent and tell it with the next prompt.
fn sharing_changed(conversation: &mut Conversation, previous: Sharing, sharing: Sharing) {
    if let Some(running) = &conversation.running {
        running.host.set_sharing(sharing);
    }
    if previous == sharing || (conversation.entries.is_empty() && conversation.running.is_none()) {
        return;
    }
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

/// The two sharing choices, as the ribbon and settings show them.
fn sharing_choice(sharing: Sharing) -> (Icon, &'static str, &'static str) {
    match sharing {
        Sharing::Schema => (
            Icon::Structure,
            "Share the schema only",
            "Table and column names, keys, indexes and CREATE statements",
        ),
        Sharing::Rows => (
            Icon::Data,
            "Share the schema and query results",
            "Also lets it run read-only SELECT queries of at most 200 rows and read the results",
        ),
    }
}

/// The two default choices; returns one when clicked.
fn default_sharing_ui(ui: &mut egui::Ui, current: Sharing) -> Option<Sharing> {
    let mut default = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        ui.label("Default").on_hover_text(
            "What each database shares until you change it with the icons in the \
             chat panel",
        );
        for sharing in [Sharing::Schema, Sharing::Rows] {
            let (icon, name, detail) = sharing_choice(sharing);
            let name = format!("Default: {}", name.to_lowercase());
            if icons::control(ui, icon, &name, "", sharing == current, false)
                .on_hover_text(detail)
                .clicked()
            {
                default = Some(sharing);
            }
        }
        ui.weak(sharing_choice(current).1);
    });
    default
}

/// One database's choice for one agent; true when it should be forgotten.
fn grant_row(ui: &mut egui::Ui, path: &str, agent: &str, sharing: Sharing) -> bool {
    ui.horizontal(|ui| {
        let (icon, name, _) = sharing_choice(sharing);
        let (rect, response) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
        icons::paint(ui.painter(), rect, icon, ui.visuals().text_color());
        response.on_hover_text(name);
        let file = std::path::Path::new(path)
            .file_name()
            .map_or_else(|| path.to_owned(), |n| n.to_string_lossy().into_owned());
        ui.label(file).on_hover_text(path);
        ui.weak(format!("{agent} · {name}"));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            icons::control(ui, Icon::Trash, "Forget; use the default", "", false, false).clicked()
        })
        .inner
    })
    .inner
}

impl App {
    pub(super) fn assistant_database(&self) -> Option<String> {
        (!self.database.tools.database_scope.is_empty()
            && (self.database.session.is_some() || !self.database.assistant.entries.is_empty()))
        .then(|| self.database.tools.database_scope.clone())
    }
    fn assistant_sharing(&self) -> Option<Sharing> {
        let database = self.assistant_database()?;
        Some(self.preferences.assistant.sharing_for(&database))
    }
    /// Remember a choice for this database and selected agent, and apply it at once.
    pub(super) fn set_assistant_sharing(&mut self, sharing: Sharing) {
        let Some(database) = self.assistant_database() else {
            return;
        };
        let previous = self.preferences.assistant.sharing_for(&database);
        self.preferences
            .assistant
            .grants
            .entry(database)
            .or_default()
            .insert(self.preferences.assistant.provider, sharing);
        self.preferences.normalize();
        self.store.save(&self.preferences);
        sharing_changed(&mut self.database.assistant, previous, sharing);
    }
    /// Change what databases without their own choice share, in every workspace.
    fn set_default_assistant_sharing(&mut self, sharing: Sharing) {
        let previous = self.preferences.assistant.default_sharing;
        if previous == sharing {
            return;
        }
        self.preferences.assistant.default_sharing = sharing;
        self.store.save(&self.preferences);
        let assistant = &self.preferences.assistant;
        for database in std::iter::once(&mut self.database).chain(self.parked.values_mut()) {
            if assistant
                .grant_for(&database.tools.database_scope)
                .is_none()
            {
                sharing_changed(&mut database.assistant, previous, sharing);
            }
        }
    }
    /// How the chat's copy buttons write text.
    pub(super) fn chat_settings_ui(&mut self, ui: &mut egui::Ui) {
        let mut format = self.preferences.assistant.copy_format;
        AgentGrid::new(ui).field(ui, "Copy as", Column::Rest, |ui| {
            for (choice, name) in assistant_export::FORMATS {
                ui.selectable_value(&mut format, choice, name)
                    .on_hover_text("How the chat's copy buttons write a reply or the conversation");
            }
        });
        if format != self.preferences.assistant.copy_format {
            self.preferences.assistant.copy_format = format;
            self.store.save(&self.preferences);
        }
    }

    /// The default sharing, then each database's own choice, which can be forgotten.
    pub(super) fn sharing_settings_ui(&mut self, ui: &mut egui::Ui) {
        if let Some(sharing) = default_sharing_ui(ui, self.preferences.assistant.default_sharing) {
            self.set_default_assistant_sharing(sharing);
        }
        if self.preferences.assistant.grants.is_empty() {
            return;
        }
        ui.add_space(4.0);
        let mut forget = None;
        for (path, grants) in &self.preferences.assistant.grants {
            for (provider, sharing) in grants {
                if grant_row(ui, path, provider.vendor().name, *sharing) {
                    forget = Some((path.clone(), *provider));
                }
            }
        }
        let Some((path, provider)) = forget else {
            return;
        };
        let assistant = &mut self.preferences.assistant;
        let previous = assistant
            .grants
            .get_mut(&path)
            .and_then(|g| g.remove(&provider));
        // Conversations with that agent about it go on with the default, and are told so.
        if let Some(previous) = previous
            && provider == assistant.provider
        {
            let default = assistant.default_sharing;
            for database in std::iter::once(&mut self.database).chain(self.parked.values_mut()) {
                if path == database.tools.database_scope {
                    sharing_changed(&mut database.assistant, previous, default);
                }
            }
        }
        self.preferences.normalize();
        self.store.save(&self.preferences);
    }

    pub(super) fn reset_assistant_conversations(&mut self) {
        for database in std::iter::once(&mut self.database).chain(self.parked.values_mut()) {
            database.assistant.reset();
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
        let wake: Wake = Arc::new(move || wake.request_repaint());
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
        let profile = self.assistant_profile()?;
        #[cfg(test)]
        let program = self
            .database
            .assistant
            .program
            .clone()
            .unwrap_or_else(|| profile.program.clone());
        #[cfg(not(test))]
        let program = profile.program.clone();
        let model = self
            .preferences
            .assistant
            .selected_model()
            .trim()
            .to_owned();
        let model = (!model.is_empty()).then_some(model.as_str());
        let effort = self.assistant_effort();
        let privacy = self
            .preferences
            .assistant
            .privacy
            .of(self.preferences.assistant.provider);
        let prompt =
            tools::system_prompt(privacy.share_file_name.then_some(name.as_str()), sharing);
        let mut spec = match profile.protocol {
            Protocol::ClaudeStream => claude::spec(
                program,
                cwd,
                endpoint.config_path(),
                &tools::NAMES,
                &prompt,
                model,
                privacy.agent(),
            ),
            Protocol::CodexAppServer => codex::spec(
                program,
                cwd,
                endpoint.launch().clone(),
                &prompt,
                model,
                privacy.agent(),
            ),
            Protocol::Acp => Spec {
                program,
                args: profile.args.clone(),
                env: profile.env.clone(),
                cwd,
                protocol: Protocol::Acp,
                system_prompt: prompt,
                mcp: Some(endpoint.launch().clone()),
                model: model.map(str::to_owned),
                privacy: privacy.agent(),
                effort: None,
            },
        };
        // Every protocol applies the level in its driver, not on the command line.
        spec.effort = effort;
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
        if text.is_empty() || self.database.assistant.busy || self.assistant_account_pending() {
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

    /// The assistant as a side panel, beside the database chooser.
    pub(super) fn assistant_ui(&mut self, ui: &mut egui::Ui) {
        self.assistant_header(ui, false);
        ui.separator();
        self.assistant_body(ui);
    }
    /// The conversation and its input, below the header.
    pub(super) fn assistant_body(&mut self, ui: &mut egui::Ui) {
        self.poll_assistant();
        self.check_selected_assistant(ui.ctx());
        let Some(_) = self.assistant_database() else {
            icons::empty_state(
                ui,
                Icon::Assistant,
                "Open a database, then ask your signed-in agent about it here",
            );
            return;
        };
        let Some(sharing) = self.assistant_sharing() else {
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
                    self.assistant_intro(ui);
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

    /// `tile`: the header of the assistant tile, which minimizes and leaves
    /// room for the grip like every tile's.
    pub(super) fn assistant_header(&mut self, ui: &mut egui::Ui, tile: bool) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            if tile {
                ui.set_min_height(icons::RIBBON_BUTTON_SIZE);
                ui.add_space(presentation::BAR_INSET);
            }
            // The icons claim their room first; the agent picker gets the rest.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if tile {
                    ui.add_space(presentation::BAR_INSET);
                }
                self.header_window_controls(ui, tile);
                ui.separator();
                self.header_sharing(ui);
                self.header_copy(ui);
                self.header_new_conversation(ui);
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    self.assistant_agent_picker(ui);
                });
            });
        });
    }

    /// Expand, and in a tile minimize: the same controls, place and wording as
    /// a tile's.
    fn header_window_controls(&mut self, ui: &mut egui::Ui, tile: bool) {
        let expanded = self.assistant_expanded();
        let (name, keys) = if expanded {
            ("Restore workspace", "Ctrl Alt Enter")
        } else {
            ("Expand chat to workspace", "")
        };
        let expand = ui
            .add_enabled_ui(self.assistant_database().is_some(), |ui| {
                icons::control(ui, Icon::Fit, name, keys, expanded, false)
            })
            .inner;
        if expand.clicked() {
            self.toggle_assistant_expanded();
        }
        if tile
            && icons::control(ui, Icon::Minimize, "Minimize to header", "", false, false).clicked()
        {
            self.set_assistant_shown(false);
        }
    }

    /// What the agent may read, once a database is open.
    fn header_sharing(&mut self, ui: &mut egui::Ui) {
        let Some(current) = self.assistant_sharing() else {
            return;
        };
        // Right to left: reads schema, then schema and results.
        for sharing in [Sharing::Rows, Sharing::Schema] {
            let (icon, name, detail) = sharing_choice(sharing);
            if icons::control(ui, icon, name, "", sharing == current, false)
                .on_hover_text(detail)
                .clicked()
                && sharing != current
            {
                self.set_assistant_sharing(sharing);
            }
        }
        ui.separator();
    }

    /// Right to left: the whole conversation, then the last reply.
    fn header_copy(&self, ui: &mut egui::Ui) {
        let format = self.preferences.assistant.copy_format;
        let tip = format!(
            "As {}; change in Settings → Assistant",
            assistant_export::format_name(format)
        );
        let entries = &self.database.assistant.entries;
        let reply = assistant_export::last_reply(entries, format);
        let copy = |ui: &mut egui::Ui, icon, name| {
            ui.add_enabled_ui(reply.is_some(), |ui| {
                icons::control(ui, icon, name, "", false, false)
            })
            .inner
            .on_hover_text(&tip)
            .clicked()
        };
        if copy(ui, Icon::CopyAll, "Copy conversation") {
            let agent = self.preferences.assistant.provider.vendor().name;
            let text = assistant_export::conversation(entries, agent, format);
            ui.ctx().copy_text(text);
        }
        if copy(ui, Icon::Copy, "Copy last reply")
            && let Some(reply) = reply
        {
            ui.ctx().copy_text(reply);
        }
    }

    fn header_new_conversation(&mut self, ui: &mut egui::Ui) {
        let conversation = &mut self.database.assistant;
        let started = !conversation.entries.is_empty() || conversation.running.is_some();
        if ui
            .add_enabled_ui(started, |ui| {
                icons::control(ui, Icon::Plus, "New conversation", "", false, false)
            })
            .inner
            .clicked()
        {
            conversation.reset();
            conversation.focus = true;
        }
    }

    /// The header's icon, model picker and status dot, narrowed to fit.
    fn assistant_agent_picker(&mut self, ui: &mut egui::Ui) {
        let provider = self.preferences.assistant.provider;
        let name = provider.vendor().name;
        // The status dot, its gap and spacing take 20 points; the title icon
        // only shows when there is room to spare beside the picker.
        if ui.available_width() >= 240.0 {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::hover());
            icons::paint(
                ui.painter(),
                rect,
                Icon::Assistant,
                ui.visuals().text_color(),
            );
        }
        self.assistant_model_menu(ui, ui.available_width() - 20.0);
        let (state, intent, _) = self.agent_state(provider);
        status_dot(ui, intent, &format!("{name} · {state}"));
        ui.add_space(6.0);
    }

    /// Who receives what, above an empty conversation: the first time an agent is
    /// used, then once every few sessions. Decided once per session and agent.
    fn assistant_intro(&mut self, ui: &mut egui::Ui) {
        let id = egui::Id::unique(("assistant-intro", self.preferences.assistant.provider));
        let show = match ui.ctx().data(|d| d.get_temp::<bool>(id)) {
            Some(show) => show,
            None => {
                let show = self.preferences.assistant.take_intro();
                self.store.save(&self.preferences);
                ui.ctx().data_mut(|d| d.insert_temp(id, show));
                show
            }
        };
        if !show {
            return;
        }
        ui.add_space(8.0);
        let text = match self.assistant_profile() {
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
        ui.add_space(4.0);
        if ui
            .link("Privacy settings")
            .on_hover_text("Telemetry, history and what this agent may learn")
            .clicked()
        {
            self.open_assistant_privacy();
        }
    }

    /// Settings → Assistant, scrolled to the selected agent's privacy choices.
    pub(super) fn open_assistant_privacy(&mut self) {
        self.open_assistant_settings();
        self.settings.reveal_privacy = true;
        // The link is about the agent this chat uses.
        self.accounts.configuring = Some(self.preferences.assistant.provider);
    }

    fn assistant_input(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        let id = ui.make_persistent_id("assistant-input");
        // Enter sends; Shift Enter keeps its newline.
        let send = ui.memory(|m| m.has_focus(id))
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
        let busy = self.database.assistant.busy;
        let account_pending = self.assistant_account_pending();
        // Send, or Stop while the agent works, sits inside the box's corner;
        // the right margin keeps text from running under it.
        const BUTTON: f32 = 32.0;
        let edit = egui::TextEdit::multiline(&mut self.database.assistant.draft)
            .id(id)
            .desired_rows(2)
            .desired_width(f32::INFINITY)
            .margin(egui::Margin {
                left: 4,
                right: (BUTTON + 10.0) as i8,
                top: 2,
                bottom: 2,
            })
            .hint_text("Ask about this database · Enter sends");
        let response = ui.add(edit);
        let corner = egui::Rect::from_min_size(
            response.rect.right_bottom() - egui::vec2(BUTTON + 5.0, BUTTON + 5.0),
            egui::vec2(BUTTON, BUTTON),
        );
        if busy {
            if stop_button(ui, corner, id.with("stop"))
                && let Some(running) = &self.database.assistant.running
            {
                running.agent.cancel();
            }
        } else {
            let empty = self.database.assistant.draft.trim().is_empty();
            if send_button(ui, corner, !empty && !account_pending) {
                self.send_assistant(ui.ctx());
            }
        }
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
        if account_pending {
            ui.small("Finish or cancel the account change in Settings to send a message.");
        }
        if send && !busy {
            self.send_assistant(ui.ctx());
        }
    }
}

/// Stop, in red, in the input's corner while the agent works.
fn stop_button(ui: &mut egui::Ui, corner: egui::Rect, id: egui::Id) -> bool {
    let stop = ui.interact(corner, id, egui::Sense::click());
    let red = ui.visuals().error_fg_color;
    let fill = if stop.hovered() {
        red
    } else {
        red.gamma_multiply(0.85)
    };
    ui.painter().rect_filled(corner, 8.0, fill);
    icons::paint(
        ui.painter(),
        egui::Rect::from_center_size(corner.center(), egui::vec2(18.0, 18.0)),
        Icon::Stop,
        egui::Color32::WHITE,
    );
    stop.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Stop"));
    stop.on_hover_text("Stop the agent's turn")
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}

/// Send, in the input's corner while the agent waits.
fn send_button(ui: &mut egui::Ui, corner: egui::Rect, enabled: bool) -> bool {
    let layout = egui::Layout::centered_and_justified(egui::Direction::LeftToRight);
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(corner).layout(layout),
        |ui| {
            ui.add_enabled_ui(enabled, |ui| {
                icons::control(ui, Icon::Send, "Send", "Enter", false, false)
            })
            .inner
            .clicked()
        },
    )
    .inner
}

/// Add a pointer to the fix for the errors people hit most.
fn explain(error: &str, sign_in: Option<&str>) -> String {
    let lower = error.to_lowercase();
    let asks = ["login", "log in", "sign in", "authenticat", "unauthori"];
    if asks.iter().any(|w| lower.contains(w)) && !lower.contains("then try again") {
        let command = sign_in.unwrap_or("the agent's CLI");
        format!(
            "{error}\nSign in in Settings → Assistant, or run `{command}` in a terminal, then send again."
        )
    } else if error.trim().is_empty() {
        "The agent gave no reason.".into()
    } else {
        error.into()
    }
}
