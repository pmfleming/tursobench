//! The assistant panel sits on the right; it hides independently of navigation.
//! It drives the user's own signed-in agent CLI (Claude Code by default, or an
//! Agent Client Protocol agent) and offers it read-only tools for the database
//! in this workspace. TursoBench never handles credentials.
use super::{
    App, assistant_export,
    icons::{self, Icon},
    presentation::{self, Intent},
};
use crate::assistant_tools::{self as tools, AgentAccess, Host, Proposal, Sharing};
use eframe::egui;
use serde_json::Value as Json;
use std::sync::{Arc, Mutex, mpsc};
use transcript::{Action, entry_ui};
use tursobench_assistant::{
    agent::{Agent, Protocol, Spec, Update, Wake},
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
    endpoint: Endpoint,
    /// Whether the chat already said this agent never reached the tools.
    told_unconnected: bool,
    /// The user stopped the current turn, which proves nothing about the tools.
    stopped: bool,
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
    label: Option<&'static str>,
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
    /// How the running agent was told to start its relay.
    #[cfg(test)]
    pub(super) fn relay_launch(&self) -> Option<relay::Launch> {
        self.running.as_ref().map(|r| r.endpoint.launch().clone())
    }
}

/// Apply an access change to a conversation's agent; a new sharing level is
/// told to it with the next prompt.
fn access_changed(conversation: &mut Conversation, previous: AgentAccess, access: AgentAccess) {
    if let Some(running) = &conversation.running {
        running.host.set_access(access);
    }
    if previous.sharing == access.sharing
        || (conversation.entries.is_empty() && conversation.running.is_none())
    {
        return;
    }
    let text = match access.sharing {
        Sharing::Schema => "Now sharing only the schema",
        Sharing::Limited => "Now sharing the schema and limited results",
        Sharing::Complete => "Now sharing the schema and complete results",
    };
    conversation.note = Some(format!(
        "Changed by the user. {}",
        tools::shared_text(access)
    ));
    conversation.push(Entry::Notice(Intent::Neutral, text.into()));
}

/// The three sharing choices, as the chat panel, the Agent panel and
/// settings show them: icon, name and what it lets the agent read.
pub(super) fn sharing_choice(sharing: Sharing) -> (Icon, &'static str, &'static str) {
    match sharing {
        Sharing::Schema => (
            Icon::Structure,
            "Share the schema only",
            "Table and column names, keys, indexes and CREATE statements",
        ),
        Sharing::Limited => (
            Icon::Data,
            "Share the schema and limited results",
            "Also lets it run read-only SELECT queries of at most 200 rows and read the results",
        ),
        Sharing::Complete => (
            Icon::Database,
            "Share the schema and complete results",
            "Also lets it run read-only SELECT queries of up to this database's row limit \
             (its Limits panel) and read the results",
        ),
    }
}

/// The three default choices; returns a new one when clicked.
fn default_sharing_ui(ui: &mut egui::Ui, current: Sharing) -> Option<Sharing> {
    let mut default = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        ui.label("Default").on_hover_text(
            "What each database shares with every agent until you change it in its \
             Agent panel or the chat panel",
        );
        for sharing in Sharing::ALL {
            let (icon, name, detail) = sharing_choice(sharing);
            let name = format!("Default: {}", name.to_lowercase());
            if icons::control(ui, icon, &name, "", sharing == current, false)
                .on_hover_text(detail)
                .clicked()
                && sharing != current
            {
                default = Some(sharing);
            }
        }
        ui.weak(sharing_choice(current).1);
    });
    default
}

/// A database's file name, or its whole path when it has none.
fn file_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map_or_else(|| path.to_owned(), |n| n.to_string_lossy().into_owned())
}

impl App {
    pub(super) fn assistant_database(&self) -> Option<String> {
        (!self.database.tools.database_scope.is_empty()
            && (self.database.session.is_some() || !self.database.assistant.entries.is_empty()))
        .then(|| self.database.tools.database_scope.clone())
    }
    fn assistant_access(&self) -> Option<AgentAccess> {
        let database = self.assistant_database()?;
        Some(self.preferences.access(&database))
    }
    /// What every agent may read in the selected database.
    pub(super) fn database_sharing(&self) -> Sharing {
        self.preferences.sharing_for(&self.database_key())
    }
    /// Remember what every agent may read in the selected database, and apply it at once.
    pub(super) fn set_assistant_sharing(&mut self, sharing: Sharing) {
        if self.database.path.is_empty() {
            return;
        }
        let database = self.database_key();
        self.change_access(|app| app.preferences.database_mut(&database).sharing = Some(sharing));
    }
    /// Change what databases share, or their row limits, and save it. Each
    /// workspace's agent applies its database's access at once, and is told
    /// of a new sharing level with its next prompt.
    pub(super) fn change_access(&mut self, change: impl FnOnce(&mut Self)) {
        let before: Vec<_> = std::iter::once(&self.database)
            .chain(self.parked.values())
            .map(|d| self.preferences.access(&d.tools.database_scope))
            .collect();
        change(self);
        self.preferences.normalize();
        self.store.save(&self.preferences);
        let preferences = &self.preferences;
        let databases = std::iter::once(&mut self.database).chain(self.parked.values_mut());
        for (database, before) in databases.zip(before) {
            let after = preferences.access(&database.tools.database_scope);
            access_changed(&mut database.assistant, before, after);
        }
    }

    /// Give every running agent its database's access as it stands, as after
    /// a row limit changed; the agent learns of the cap from its next result.
    pub(super) fn refresh_access(&mut self) {
        let preferences = &self.preferences;
        for database in std::iter::once(&mut self.database).chain(self.parked.values_mut()) {
            if let Some(running) = &database.assistant.running {
                running
                    .host
                    .set_access(preferences.access(&database.tools.database_scope));
            }
        }
    }

    /// The level every database shares until it chooses its own.
    pub(super) fn sharing_settings_ui(&mut self, ui: &mut egui::Ui) {
        let current = self.preferences.assistant.default_sharing;
        if let Some(sharing) = default_sharing_ui(ui, current) {
            self.change_access(|app| app.preferences.assistant.default_sharing = sharing);
        }
    }

    /// The selected database's Agent panel: what every agent may read here.
    pub(super) fn database_agent_ui(&mut self, ui: &mut egui::Ui) {
        let key = self.database_key();
        let own = self.preferences.database(&key).sharing.is_some();
        let default = sharing_choice(self.preferences.assistant.default_sharing).1;
        let reset = format!("Use the default: {}", default.to_lowercase());
        let mut use_default = false;
        ui.horizontal(|ui| {
            ui.strong("Agent");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                icons::hint(
                    ui,
                    Icon::Assistant,
                    16.0,
                    "Every agent reads the same here; the agent, model and effort are in \
                     Settings → Assistant",
                );
                // Back to the level set in Settings → Assistant → Sharing.
                use_default = ui
                    .add_enabled_ui(own, |ui| {
                        icons::line_control(ui, Icon::Undo, &reset, "", None)
                    })
                    .inner
                    .clicked();
            });
        });
        if use_default {
            self.change_access(|app| app.preferences.database_mut(&key).sharing = None);
        }
        let current = self.database_sharing();
        let mut chosen = None;
        for sharing in Sharing::ALL {
            let (icon, name, detail) = sharing_choice(sharing);
            ui.horizontal(|ui| {
                if icons::control(ui, icon, name, "", sharing == current, false)
                    .on_hover_text(detail)
                    .clicked()
                    && sharing != current
                {
                    chosen = Some(sharing);
                }
                let label = ui.add(egui::Label::new(name).sense(egui::Sense::click()));
                if label.on_hover_text(detail).clicked() && sharing != current {
                    chosen = Some(sharing);
                }
            });
        }
        if let Some(sharing) = chosen {
            self.set_assistant_sharing(sharing);
        }
        ui.add_space(4.0);
        ui.weak(sharing_choice(current).2);
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
            self.open_assistant_proposal(proposal.sql);
        }
    }

    /// Open SQL the agent proposed in a new editor tile, and say so in the chat.
    pub(super) fn open_assistant_proposal(&mut self, sql: String) {
        self.load_sql_window(sql);
        self.database.assistant.push(Entry::Notice(
            Intent::Primary,
            "Opened proposed SQL in an editor tile. Review it before running.".into(),
        ));
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
                // An agent whose relay never started answers without the
                // database and may reach for its own tools instead.
                let tell = conversation.running.as_mut().is_some_and(|running| {
                    let stopped = std::mem::take(&mut running.stopped);
                    let tell =
                        !stopped && !running.endpoint.connected() && !running.told_unconnected;
                    running.told_unconnected |= tell;
                    tell
                });
                if tell {
                    let name = conversation.label.unwrap_or("The agent");
                    conversation.push(Entry::Notice(
                        Intent::Danger,
                        format!(
                            "{name} did not start TursoBench's tools, so it could not read \
                             this database. Start a new conversation to try again."
                        ),
                    ));
                }
            }
            Update::Notice(text) => conversation.push(Entry::Notice(Intent::Pending, text)),
            Update::Exited { message } => {
                conversation.running = None;
                conversation.busy = false;
                let name = conversation.label.unwrap_or("The agent");
                let hint = explain(&message, conversation.sign_in.as_deref());
                conversation.push(Entry::Notice(
                    Intent::Danger,
                    format!("{name} stopped. {hint}"),
                ));
            }
        }
    }

    fn start_assistant(&mut self, ctx: &egui::Context, access: AgentAccess) -> Result<(), String> {
        let profile = self.assistant_profile()?;
        let program = profile.program;
        #[cfg(test)]
        let program = self.database.assistant.program.clone().unwrap_or(program);
        let wake = ctx.clone();
        let wake: Wake = Arc::new(move || wake.request_repaint());
        let (tx, proposals) = mpsc::channel();
        let host = Arc::new(Host::new(
            self.database.assistant.session.clone(),
            access,
            tx,
            wake.clone(),
        ));
        let relay_program = relay::relay_program()
            .map_err(|e| format!("TursoBench could not find its own program: {e}"))?;
        let endpoint = Endpoint::start(host.clone(), relay_program)
            .map_err(|e| format!("Could not start the assistant's tools: {e}"))?;
        let assistant = &self.preferences.assistant;
        let privacy = assistant.privacy.of(assistant.provider);
        let database = privacy
            .share_file_name
            .then(|| file_name(&self.database.tools.database_scope));
        let model = assistant.selected_model().trim();
        let spec = Spec {
            program,
            args: profile.args,
            cwd: relay::private_dir().map_err(|e| e.to_string())?,
            protocol: profile.vendor.protocol,
            system_prompt: tools::system_prompt(database.as_deref(), access),
            model: (!model.is_empty()).then(|| model.to_owned()),
            // Every protocol applies the level in its driver, not on the command line.
            effort: self.assistant_effort(),
            privacy: privacy.agent(),
            ..Spec::default()
        };
        let name = profile.vendor.name;
        let agent = Agent::spawn(spec.serve(&endpoint, &tools::NAMES), wake).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                format!(
                    "{name} was not found. Scan for agents or set its executable in Settings → \
                     Assistant."
                )
            } else {
                format!("Could not start {name}: {e}")
            }
        })?;
        self.database.assistant.label = Some(name);
        self.database.assistant.sign_in = Some(profile.sign_in);
        self.database.assistant.running = Some(Running {
            agent,
            host,
            proposals,
            endpoint,
            told_unconnected: false,
            stopped: false,
        });
        Ok(())
    }

    pub(super) fn send_assistant(&mut self, ctx: &egui::Context) {
        let text = self.database.assistant.draft.trim().to_owned();
        if text.is_empty() || self.database.assistant.busy || self.assistant_account_pending() {
            return;
        }
        let Some(access) = self.assistant_access() else {
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
            && let Err(error) = self.start_assistant(ctx, access)
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
        let Some(access) = self.assistant_access() else {
            icons::empty_state(
                ui,
                Icon::Assistant,
                "Open a database, then ask your signed-in agent about it here",
            );
            return;
        };
        egui::Panel::bottom("assistant-input")
            .resizable(false)
            .show(ui, |ui| self.assistant_input(ui));
        let actions = egui::ScrollArea::vertical()
            .id_salt("assistant-conversation")
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| self.assistant_conversation(ui, access.sharing))
            .inner;
        for action in actions {
            match action {
                Action::OpenSql(sql) => self.load_sql_window(sql),
                Action::Copy(text) => ui.ctx().copy_text(text),
            }
        }
    }

    /// The messages, or before the first one who receives what and what to ask;
    /// returns what their clicks ask for.
    fn assistant_conversation(&mut self, ui: &mut egui::Ui, sharing: Sharing) -> Vec<Action> {
        if self.database.assistant.entries.is_empty() {
            self.assistant_intro(ui);
            let hint = match sharing {
                Sharing::Schema => "Ask about tables, keys or how to write a query",
                Sharing::Limited | Sharing::Complete => "Ask about the schema or the data",
            };
            icons::empty_state(ui, Icon::Assistant, hint);
        }
        let mut actions = vec![];
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
        actions
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
        let Some(current) = self.assistant_access().map(|a| a.sharing) else {
            return;
        };
        // Right to left: complete results, limited results, then the schema only.
        for sharing in Sharing::ALL.into_iter().rev() {
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
                    profile.vendor.name, profile.vendor.company
                );
                match profile.vendor.protocol {
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
        let enter = ui.memory(|m| m.has_focus(id))
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
        let send = if busy {
            if stop_button(ui, corner, id.with("stop"))
                && let Some(running) = &mut self.database.assistant.running
            {
                running.stopped = true;
                running.agent.cancel();
            }
            false
        } else {
            let ready = !self.database.assistant.draft.trim().is_empty() && !account_pending;
            send_button(ui, corner, ready) || enter
        };
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
        if send {
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

#[cfg(test)]
mod tests {
    #[test]
    fn sign_in_errors_point_to_settings_once() {
        let claude = Some("claude auth login");
        let hint = "Sign in in Settings → Assistant, or run `claude auth login` in a terminal";
        assert!(super::explain("Not logged in · Please run /login", claude).contains(hint));
        assert!(super::explain("Unauthorized", None).contains("run `the agent's CLI`"));
        // An agent that already explains how to sign in is not told twice.
        let told = "Authentication required. Sign in, then try again.";
        assert_eq!(super::explain(told, claude), told);
        assert_eq!(super::explain(" ", claude), "The agent gave no reason.");
        assert_eq!(super::explain("Rate limited", claude), "Rate limited");
    }
}
