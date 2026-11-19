//! The local agents: their discovery and account jobs, which never share the
//! conversation's tool host, and Settings → Assistant's agent rows, settings
//! and privacy switches.
use super::{
    App,
    icons::{self, Icon},
    presentation::{self, Intent},
};
use crate::storage::Provider;
use eframe::egui;
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};
use tursobench_assistant::{
    account::{Event, Job, Operation, SignIn, Status, Target},
    agent::Protocol,
    vendor::{self, VENDORS, Vendor, command_words, locate},
};

#[derive(Default)]
pub(super) struct Row {
    pub status: Option<Status>,
    pub job: Option<Job>,
    pub error: Option<String>,
    pub browser: Option<String>,
    pub method: Option<String>,
    pub activity: Option<&'static str>,
    /// What to call `error`: the check, sign-in or sign-out that failed.
    pub failure: Option<&'static str>,
    pub changing_account: bool,
}
impl Row {
    /// A short state and its colour; `None` until something is known.
    pub fn describe(&self) -> Option<(&'static str, Intent)> {
        if self.job.is_some() {
            return Some((self.activity.unwrap_or("Checking…"), Intent::Pending));
        }
        if self.error.is_some() {
            return Some((self.failure.unwrap_or("Check failed"), Intent::Danger));
        }
        let status = self.status.as_ref()?;
        Some(match status.state {
            SignIn::SignedIn => ("Signed in", Intent::Primary),
            SignIn::SignedOut => ("Signed out", Intent::Pending),
            SignIn::Missing => ("Not installed", Intent::Neutral),
            SignIn::Unknown if status.issue.is_some() => ("Check failed", Intent::Danger),
            SignIn::Unknown => ("Status unavailable", Intent::Neutral),
        })
    }
}
/// A small status dot; `state` is its tooltip and accessible name.
pub(super) fn status_dot(ui: &mut egui::Ui, intent: Intent, state: &str) {
    let color = presentation::intent_color(ui, intent);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 3.0, color);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, state));
    response.on_hover_text(state);
}
#[derive(Default)]
pub(super) struct State {
    pub rows: BTreeMap<Provider, Row>,
    pub view_open: bool,
    pub executable: String,
    pub editing: Option<Provider>,
    /// The agent whose settings the settings area shows; independent of
    /// the agent the chat uses. Starts on that agent.
    pub configuring: Option<Provider>,
    /// The executable path field is showing.
    pub path_open: bool,
    /// Check agents without a click: when Settings → Assistant opens, and
    /// for the selected vendor when the panel first needs its status.
    pub automatic: bool,
    /// The chat's model picker while it is open.
    pub menu: super::assistant_picker::Menu,
}
impl App {
    pub(super) fn scan_assistant(&mut self, ctx: &egui::Context) {
        for vendor in VENDORS {
            if vendor.id != Provider::Custom
                || !self.preferences.assistant.custom_command.trim().is_empty()
            {
                // Never replace a sign-in with an automatic status check.
                if self
                    .accounts
                    .rows
                    .get(&vendor.id)
                    .is_none_or(|row| row.job.is_none())
                {
                    self.agent_account_job(ctx, vendor.id, Operation::Check);
                }
            }
        }
    }
    /// The panel's pickers need the selected vendor's status and models; check
    /// it once when nothing is known yet, without opening Settings.
    pub(super) fn check_selected_assistant(&mut self, ctx: &egui::Context) {
        let provider = self.preferences.assistant.provider;
        if !self.accounts.automatic
            || (provider == Provider::Custom
                && self.preferences.assistant.custom_command.trim().is_empty())
        {
            return;
        }
        let unknown = self
            .accounts
            .rows
            .get(&provider)
            .is_none_or(|row| row.status.is_none() && row.job.is_none() && row.error.is_none());
        if unknown {
            self.agent_account_job(ctx, provider, Operation::Check);
        }
    }
    fn account_target(&self, provider: Provider) -> Result<Target, String> {
        let profile = self.profile_for(provider)?;
        Ok(Target {
            provider,
            program: profile.program,
            args: profile.args,
            env: profile.env,
            cwd: tursobench_assistant::relay::private_dir()
                .map_err(|_| "Could not create the private agent directory")?,
        })
    }
    pub(super) fn agent_account_job(
        &mut self,
        ctx: &egui::Context,
        provider: Provider,
        operation: Operation,
    ) {
        // Every conversation runs the selected vendor (choosing another starts
        // afresh), so only a change to that vendor's account ends them.
        if !matches!(operation, Operation::Check) && provider == self.preferences.assistant.provider
        {
            self.reset_assistant_conversations();
        }
        let target = self.account_target(provider);
        let row = self.accounts.rows.entry(provider).or_default();
        row.changing_account = !matches!(operation, Operation::Check);
        let (activity, failure) = match &operation {
            Operation::Check => ("Checking…", "Check failed"),
            Operation::Login { .. } => ("Signing in…", "Sign-in failed"),
            Operation::Logout => ("Signing out…", "Sign-out failed"),
        };
        row.activity = Some(activity);
        row.failure = Some(failure);
        row.job = None;
        row.error = None;
        row.browser = None;
        match target {
            Ok(target) => {
                let ctx = ctx.clone();
                row.job = Some(Job::start(
                    target,
                    operation,
                    Arc::new(move || ctx.request_repaint()),
                ));
            }
            Err(error) => row.error = Some(error),
        }
    }
    pub(super) fn poll_assistant_accounts(&mut self, ctx: &egui::Context) {
        for row in self.accounts.rows.values_mut() {
            let events: Vec<_> = row
                .job
                .as_ref()
                .map(|job| job.events.try_iter().collect())
                .unwrap_or_default();
            for event in events {
                match event {
                    Event::Browser(url) => {
                        ctx.open_url(egui::OpenUrl::new_tab(&url));
                        row.browser = Some(url);
                    }
                    Event::Finished(_) | Event::Cancelled => {
                        row.changing_account = false;
                        row.job = None;
                        row.activity = None;
                        row.browser = None;
                        // Cancelling or failing tells nothing new about the
                        // account, so the last status and models stay.
                        match event {
                            Event::Finished(Ok(status)) => {
                                row.error = None;
                                row.status = Some(status);
                            }
                            Event::Finished(Err(error)) => row.error = Some(error),
                            _ => row.error = None,
                        }
                    }
                }
            }
        }
    }
    /// The selected vendor's sign-in or sign-out is still running. Sending
    /// waits for it; other vendors' account changes never block the chat.
    pub(super) fn assistant_account_pending(&self) -> bool {
        self.accounts
            .rows
            .get(&self.preferences.assistant.provider)
            .is_some_and(|row| row.changing_account && row.job.is_some())
    }
    /// The agent the settings area edits; the chat's agent until one is chosen.
    pub(super) fn configured_agent(&mut self) -> Provider {
        *self
            .accounts
            .configuring
            .get_or_insert(self.preferences.assistant.provider)
    }
    /// How `provider` would be started, whichever agent the chat uses.
    pub(super) fn profile_for(&self, provider: Provider) -> Result<Profile, String> {
        let preferences = &self.preferences.assistant;
        let vendor = provider.vendor();
        let mut words: Vec<String> = if provider == Provider::Custom {
            command_words(&preferences.custom_command)?
        } else {
            std::iter::once(vendor.command.to_owned())
                .chain(vendor.args.iter().map(|s| (*s).to_owned()))
                .collect()
        };
        let command = words.remove(0);
        let requested = preferences
            .executables
            .get(&provider)
            .cloned()
            .unwrap_or_else(|| command.clone().into());
        let program = locate(&requested).unwrap_or(requested);
        if provider == Provider::Antigravity && preferences.privacy.of(provider).telemetry_off {
            words.push("--disable-telemetry".into());
        }
        Ok(Profile {
            name: vendor.name.into(),
            vendor: vendor.company,
            program,
            args: words,
            env: vec![],
            protocol: vendor.protocol,
            missing: format!(
                "{} was not found. Scan for agents or set its executable in Settings → Assistant.",
                vendor.name
            ),
            sign_in: std::iter::once(command.as_str())
                .chain(vendor.login.iter().copied())
                .collect::<Vec<_>>()
                .join(" "),
        })
    }
    /// How the chat's agent would be started.
    pub(super) fn assistant_profile(&self) -> Result<Profile, String> {
        self.profile_for(self.preferences.assistant.provider)
    }
    /// What the header knows about one agent: a state, its colour, and whether
    /// it can be chosen. Before any check, a cached lookup says if it is installed.
    pub(super) fn agent_state(&self, provider: Provider) -> (&'static str, Intent, bool) {
        let row = self.accounts.rows.get(&provider);
        let installed = match row.and_then(|r| r.status.as_ref()) {
            Some(status) => status.state != SignIn::Missing,
            None if provider == Provider::Custom => {
                command_words(&self.preferences.assistant.custom_command)
                    .ok()
                    .and_then(|words| locate(std::path::Path::new(&words[0])))
                    .is_some()
            }
            None => {
                let program = self
                    .preferences
                    .assistant
                    .executables
                    .get(&provider)
                    .cloned()
                    .unwrap_or_else(|| provider.vendor().command.into());
                locate(&program).is_some()
            }
        };
        let (state, intent) = row.and_then(|r| r.describe()).unwrap_or(if installed {
            ("Installed", Intent::Neutral)
        } else {
            ("Not installed", Intent::Neutral)
        });
        (state, intent, installed)
    }

    /// Every agent with its status, a check, and its sign-in action.
    pub(super) fn assistant_agents_ui(&mut self, ui: &mut egui::Ui) {
        if !self.accounts.view_open {
            self.accounts.view_open = true;
            if self.accounts.automatic {
                self.scan_assistant(ui.ctx());
            }
        }
        let grid = AgentGrid::new(ui);
        let mut scan = false;
        grid.row(ui, |ui, cell| {
            cell(ui, Column::Name, &mut |ui| {
                ui.weak("Local agents");
            });
            cell(ui, Column::Check, &mut |ui| {
                scan = icons::control(ui, Icon::Refresh, "Scan again", "", false, false)
                    .on_hover_text("Search PATH and common installation folders; ask each CLI for sign-in status. An ACP check may create an empty session in its history.")
                    .clicked();
            });
        });
        if scan {
            vendor::forget_locations();
            self.scan_assistant(ui.ctx());
        }
        let mut selected = self.preferences.assistant.provider;
        let mut action = None;
        for vendor in VENDORS {
            let row = self.accounts.rows.entry(vendor.id).or_default();
            let clicked = ui.push_id(vendor.id, |ui| {
                agent_row(ui, grid, vendor, row, &mut selected)
            });
            if let Some(operation) = clicked.inner {
                action = Some((vendor.id, operation));
            }
        }
        if selected != self.preferences.assistant.provider {
            self.select_assistant_provider(selected);
        }
        if let Some((provider, operation)) = action {
            self.agent_account_job(ui.ctx(), provider, operation);
        }
    }

    /// Settings for any agent, whichever one the chat uses.
    pub(super) fn agent_settings_ui(&mut self, ui: &mut egui::Ui) {
        let provider = self.configured_agent();
        let grid = AgentGrid::new(ui);
        self.settings_for_ui(ui, grid, provider);
        self.assistant_agent_status(ui, grid);
        if provider == Provider::Custom {
            self.custom_command_ui(ui, grid);
        }
        self.assistant_agent_program(ui, grid);
        self.agent_model_ui(ui, grid, provider);
        self.assistant_privacy(ui, grid, provider);
    }

    /// Which agent the rows below edit; the chat's agent is marked.
    fn settings_for_ui(&mut self, ui: &mut egui::Ui, grid: AgentGrid, provider: Provider) {
        let active = self.preferences.assistant.provider;
        let named = |p: Provider| {
            let name = p.vendor().name;
            if p == active {
                format!("{name} · in use")
            } else {
                name.to_owned()
            }
        };
        let mut chosen = provider;
        grid.field(ui, "Settings for", Column::Status, |ui| {
            egui::ComboBox::from_id_salt("agent-settings-for")
                .selected_text(named(provider))
                .show_ui(ui, |ui| {
                    for vendor in VENDORS {
                        ui.selectable_value(&mut chosen, vendor.id, named(vendor.id));
                    }
                })
                .response
                .widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::ComboBox, true, "Settings for")
                });
        });
        if chosen != provider {
            self.accounts.configuring = Some(chosen);
            self.accounts.path_open = false;
        }
    }

    /// The command line of a custom ACP agent; a new one forgets its grants.
    fn custom_command_ui(&mut self, ui: &mut egui::Ui, grid: AgentGrid) {
        let before = self.preferences.assistant.custom_command.clone();
        let response = text_setting(
            ui,
            grid,
            ("Command", "Agent command"),
            "opencode acp",
            &mut self.preferences.assistant.custom_command,
        );
        if before != self.preferences.assistant.custom_command {
            self.preferences.assistant.clear_custom_grants();
            if self.preferences.assistant.provider == Provider::Custom {
                self.reset_assistant_conversations();
            }
            self.accounts.rows.remove(&Provider::Custom);
        }
        if let Some(response) = response {
            self.setting_response(response);
        }
    }

    /// A model typed for the agent; changing the chat agent's starts afresh.
    fn agent_model_ui(&mut self, ui: &mut egui::Ui, grid: AgentGrid, provider: Provider) {
        let previous = self.preferences.assistant.selected_model().to_owned();
        let model = self
            .preferences
            .assistant
            .models
            .entry(provider)
            .or_default();
        let response = text_setting(
            ui,
            grid,
            ("Model", "Assistant model"),
            "Agent default",
            model,
        );
        if let Some(response) = response {
            self.setting_response(response.on_hover_text(
                "Passed to the agent; leave empty for its own default. Applies to the \
                 next conversation.",
            ));
        }
        if previous != self.preferences.assistant.selected_model() {
            self.reset_assistant_conversations();
        }
    }

    /// The configured agent's sign-in problems and choices, in its settings area.
    fn assistant_agent_status(&mut self, ui: &mut egui::Ui, grid: AgentGrid) {
        let provider = self.configured_agent();
        let Some(row) = self.accounts.rows.get_mut(&provider) else {
            return;
        };
        if let Some(error) = &row.error {
            grid.field(ui, "", Column::Rest, |ui| {
                ui.add(
                    egui::Label::new(egui::RichText::new(error).color(ui.visuals().error_fg_color))
                        .truncate(),
                )
                .on_hover_text(error);
            });
        }
        if let Some(status) = &row.status {
            if let Some(issue) = &status.issue {
                grid.field(ui, "", Column::Rest, |ui| {
                    ui.add(egui::Label::new(egui::RichText::new(issue).small()).truncate())
                        .on_hover_text(issue);
                });
            }
            if status.auth_methods.len() > 1 {
                row.method = Some(sign_in_method_ui(ui, grid, status, row.method.as_deref()));
            }
        }
        if let Some(url) = &row.browser {
            grid.field(ui, "", Column::Rest, |ui| {
                ui.hyperlink_to("Open sign-in page again", url);
            });
        }
    }

    /// The configured agent's executable, terminal sign-in and path override.
    fn assistant_agent_program(&mut self, ui: &mut egui::Ui, grid: AgentGrid) {
        let provider = self.configured_agent();
        let profile = self.profile_for(provider).ok();
        if self.accounts.editing != Some(provider) {
            self.accounts.editing = Some(provider);
            self.accounts.executable = self
                .preferences
                .assistant
                .executables
                .get(&provider)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
        }
        let custom = self
            .preferences
            .assistant
            .executables
            .contains_key(&provider);
        if let Some(profile) = &profile
            && program_rows(
                ui,
                grid,
                (&profile.program, provider.vendor().login),
                custom,
                self.accounts.path_open,
            )
        {
            self.accounts.path_open = !self.accounts.path_open;
        }
        if (self.accounts.path_open || custom) && self.executable_field(ui, grid) {
            self.apply_executable(ui.ctx(), provider);
        }
    }

    /// The path override; true when Enter or Apply asks to use it.
    fn executable_field(&mut self, ui: &mut egui::Ui, grid: AgentGrid) -> bool {
        let mut apply = false;
        grid.row(ui, |ui, cell| {
            cell(ui, Column::Field, &mut |ui| {
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.accounts.executable)
                        .hint_text("Absolute path; empty finds it automatically")
                        .desired_width(f32::INFINITY),
                );
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Executable path")
                });
                apply |= response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            });
            cell(ui, Column::Action, &mut |ui| {
                apply |= ui
                    .add(egui::Button::new("Apply").small().min_size(grid.action))
                    .clicked();
            });
        });
        apply
    }

    /// Use the typed path, or find the program again when it is empty, and check it.
    fn apply_executable(&mut self, ctx: &egui::Context, provider: Provider) {
        let path = std::path::PathBuf::from(self.accounts.executable.trim());
        if !(path.as_os_str().is_empty() || path.is_absolute()) {
            self.accounts.rows.entry(provider).or_default().error =
                Some("Use an absolute executable path".into());
            return;
        }
        let executables = &mut self.preferences.assistant.executables;
        if path.as_os_str().is_empty() {
            executables.remove(&provider);
        } else {
            executables.insert(provider, path);
        }
        if provider == Provider::Custom {
            self.preferences.assistant.clear_custom_grants();
        }
        if provider == self.preferences.assistant.provider {
            self.reset_assistant_conversations();
        }
        self.preferences.normalize();
        self.store.save(&self.preferences);
        // The old executable's status says nothing about the new one.
        vendor::forget_locations();
        self.accounts.rows.remove(&provider);
        self.accounts.path_open = false;
        self.agent_account_job(ctx, provider, Operation::Check);
    }

    /// The configured agent's privacy switches, on its settings grid; the fixed promises are in the docs.
    fn assistant_privacy(&mut self, ui: &mut egui::Ui, grid: AgentGrid, provider: Provider) {
        let grid = grid.compact();
        let mut label = Some("Privacy");
        let mut next = move || label.take().unwrap_or("");
        let mut changed = false;
        let mut privacy = self.preferences.assistant.privacy.of(provider);
        let mut switch =
            |ui: &mut egui::Ui, label: &str, value: &mut bool, text: &str, tip: &str| {
                grid.field(ui, label, Column::Rest, |ui| {
                    changed |= presentation::checkbox(ui, value, text)
                        .on_hover_text(format!("{tip}\nApplies to the next conversation."))
                        .changed();
                });
            };
        let note = |ui: &mut egui::Ui, label: &str, text: &str, tip: &str| {
            grid.field(ui, label, Column::Rest, |ui| {
                guarantee(ui, Icon::Settings, text, tip);
            });
        };
        let [telemetry, history] = privacy_switches(provider);
        if let Some((text, tip)) = telemetry {
            switch(ui, next(), &mut privacy.telemetry_off, text, tip);
        }
        match (telemetry, history) {
            (_, Some((text, tip))) => switch(ui, next(), &mut privacy.keep_history, text, tip),
            (Some(_), None) => note(
                ui,
                next(),
                "History follows Antigravity's own settings",
                "ACP does not offer a per-conversation history switch. Empty status sessions may appear in CLI history.",
            ),
            (None, None) => {
                let name = self
                    .profile_for(provider)
                    .map_or_else(|_| "The agent".into(), |profile| profile.name);
                note(
                    ui,
                    next(),
                    &format!("Telemetry and history follow {name}'s own settings"),
                    "TursoBench cannot change them for Agent Client Protocol agents; see \
                     the agent's documentation.",
                );
            }
        }
        switch(
            ui,
            next(),
            &mut privacy.share_file_name,
            "Tell the agent the database's file name",
            "When off, the agent is only told that a database is open. Its folder is \
             never sent.",
        );
        if changed {
            *self.preferences.assistant.privacy.of_mut(provider) = privacy;
            self.preferences.normalize();
            self.store.save(&self.preferences);
        }
    }
}

/// How to start one kind of agent and what to tell the user about it.
pub(super) struct Profile {
    pub name: String,
    /// Who receives what the agent reads.
    pub vendor: &'static str,
    pub(super) program: PathBuf,
    pub(super) args: Vec<String>,
    pub(super) env: Vec<(String, String)>,
    pub(super) protocol: Protocol,
    /// Shown when the program is not installed.
    pub(super) missing: String,
    /// The command that signs in, for error hints.
    pub(super) sign_in: String,
}

/// One agent's row: choose it, its status, a check, and its account action.
fn agent_row(
    ui: &mut egui::Ui,
    grid: AgentGrid,
    vendor: &Vendor,
    row: &Row,
    selected: &mut Provider,
) -> Option<Operation> {
    let state = row.status.as_ref().map_or(SignIn::Unknown, |s| s.state);
    let (label, intent) = row.describe().unwrap_or(("Not checked", Intent::Neutral));
    let details = row
        .status
        .as_ref()
        .map(|s| {
            [&s.version, &s.method, &s.plan, &s.issue]
                .into_iter()
                .flatten()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(" · ")
        })
        .unwrap_or_default();
    let mut action = None;
    grid.row(ui, |ui, cell| {
        cell(ui, Column::Name, &mut |ui| {
            ui.radio_value(&mut *selected, vendor.id, vendor.name);
        });
        cell(ui, Column::Status, &mut |ui| {
            status_dot(ui, intent, label);
            ui.add(egui::Label::new(egui::RichText::new(label).small()).truncate())
                .on_hover_text(row.error.clone().unwrap_or_else(|| details.clone()));
        });
        if row.job.is_none() && state != SignIn::Missing {
            cell(ui, Column::Check, &mut |ui| {
                if icons::control(ui, Icon::Refresh, "Check agent", "", false, false).clicked() {
                    action = Some(Operation::Check);
                }
            });
        }
        cell(ui, Column::Action, &mut |ui| {
            if let Some(operation) = account_action(ui, grid, vendor, row, state) {
                action = Some(operation);
            }
        });
    });
    action
}

/// Cancel a running job, or install, sign out or sign in.
fn account_action(
    ui: &mut egui::Ui,
    grid: AgentGrid,
    vendor: &Vendor,
    row: &Row,
    state: SignIn,
) -> Option<Operation> {
    let mut button = |text: &str, enabled: bool| {
        ui.add_enabled(
            enabled,
            egui::Button::new(text).small().min_size(grid.action),
        )
    };
    if let Some(job) = &row.job {
        if button("Cancel", true).clicked() {
            job.cancel();
        }
        return None;
    }
    match state {
        SignIn::Missing => {
            if button("Install", true).on_hover_text(vendor.help).clicked() {
                ui.ctx().open_url(egui::OpenUrl::new_tab(vendor.help));
            }
            None
        }
        // ACP has no sign-out; keep the slot so the grid stays whole.
        SignIn::SignedIn => button(
            "Sign out",
            matches!(vendor.id, Provider::Claude | Provider::Codex),
        )
        .on_disabled_hover_text("Use the agent's own terminal command to sign out")
        .clicked()
        .then_some(Operation::Logout),
        _ => button("Sign in", true).clicked().then(|| Operation::Login {
            method: row.method.clone(),
        }),
    }
}

/// The sign-in method to use, when the agent offers several.
fn sign_in_method_ui(
    ui: &mut egui::Ui,
    grid: AgentGrid,
    status: &Status,
    method: Option<&str>,
) -> String {
    let mut choice = method.map_or_else(|| status.auth_methods[0].id.clone(), str::to_owned);
    grid.field(ui, "Sign in with", Column::Status, |ui| {
        egui::ComboBox::from_id_salt("agent-auth-method")
            .selected_text(
                status
                    .auth_methods
                    .iter()
                    .find(|m| m.id == choice)
                    .map_or("Sign-in method", |m| m.name.as_str()),
            )
            .show_ui(ui, |ui| {
                for method in &status.auth_methods {
                    ui.selectable_value(&mut choice, method.id.clone(), &method.name);
                }
            });
    });
    choice
}

/// The program and its terminal sign-in command; true when the path field
/// should open or close.
fn program_rows(
    ui: &mut egui::Ui,
    grid: AgentGrid,
    (program, login): (&std::path::Path, &[&str]),
    custom: bool,
    path_open: bool,
) -> bool {
    let full = program.to_string_lossy();
    let short = program
        .file_name()
        .map_or_else(|| full.clone(), |n| n.to_string_lossy());
    let found = if custom {
        "Your path"
    } else {
        "Found automatically"
    };
    let mut toggle = false;
    command_row(
        ui,
        grid,
        "Program",
        &short,
        &format!("{full}\n{found}"),
        &mut |ui| {
            toggle =
                icons::control(ui, Icon::Edit, "Choose executable", "", path_open, false).clicked();
        },
    );
    let shown = std::iter::once(short.as_ref())
        .chain(login.iter().copied())
        .collect::<Vec<_>>()
        .join(" ");
    let tip = "Run this in a terminal to sign in there instead";
    command_row(ui, grid, "Terminal", &shown, tip, &mut |ui| {
        if icons::control(ui, Icon::Copy, "Copy sign-in command", "", false, false)
            .on_hover_text("Copies the full path, quoted for a shell")
            .clicked()
        {
            ui.ctx().copy_text(shell_command(program, login));
        }
    });
    toggle
}

/// A named command in monospace, with one icon beside it.
fn command_row(
    ui: &mut egui::Ui,
    grid: AgentGrid,
    label: &str,
    command: &str,
    tip: &str,
    icon: &mut dyn FnMut(&mut egui::Ui),
) {
    grid.row(ui, |ui, cell| {
        cell(ui, Column::Name, &mut |ui| {
            ui.weak(label);
        });
        cell(ui, Column::Status, &mut |ui| {
            ui.add(egui::Label::new(egui::RichText::new(command).monospace()).truncate())
                .on_hover_text(tip);
        });
        cell(ui, Column::Check, icon);
    });
}

fn shell_command(program: &std::path::Path, args: &[&str]) -> String {
    let quote = |word: &str| format!("'{}'", word.replace('\'', "'\\''"));
    std::iter::once(quote(&program.to_string_lossy()))
        .chain(args.iter().map(|s| quote(s)))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A one-line text setting; `(label, name)` are its row label and accessible name.
fn text_setting(
    ui: &mut egui::Ui,
    grid: AgentGrid,
    (label, name): (&str, &str),
    hint: &str,
    value: &mut String,
) -> Option<egui::Response> {
    let mut response = None;
    grid.field(ui, label, Column::Rest, |ui| {
        let field = ui.add(
            egui::TextEdit::singleline(value)
                .hint_text(hint)
                .desired_width(f32::INFINITY),
        );
        field.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, name));
        response = Some(field);
    });
    response
}

/// The telemetry and history switches TursoBench can set for an agent, with
/// their tips, in the user's words; `None` follows the agent's own settings.
fn privacy_switches(provider: Provider) -> [Option<(&'static str, &'static str)>; 2] {
    match provider {
        Provider::Claude => [
            Some((
                "Block telemetry and error reports",
                "Starts Claude Code with CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC, \
                 DISABLE_TELEMETRY and DISABLE_ERROR_REPORTING, so only the \
                 conversation reaches Anthropic.",
            )),
            Some((
                "Save conversations in Claude Code's history",
                "When off, Claude Code runs with --no-session-persistence and \
                 writes nothing to its session history.",
            )),
        ],
        Provider::Codex => [
            Some((
                "Block analytics and telemetry",
                "Starts Codex with analytics.enabled=false, every OpenTelemetry \
                 exporter set to none and feedback.enabled=false, so only the \
                 conversation reaches OpenAI. Your config.toml is not changed.",
            )),
            Some((
                "Save conversations in Codex's history",
                "When off, the thread is ephemeral and history.persistence is none, \
                 so Codex writes nothing to its history.",
            )),
        ],
        Provider::Antigravity => [
            Some(("Block telemetry", "Starts agy acp with --disable-telemetry")),
            None,
        ],
        Provider::Copilot | Provider::Custom => [None, None],
    }
}

/// A quiet note in place of a switch: an icon and a short line, details on hover.
fn guarantee(ui: &mut egui::Ui, icon: Icon, label: &str, details: &str) {
    ui.horizontal(|ui| {
        let color = presentation::intent_color(ui, Intent::Neutral);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
        icons::paint(ui.painter(), rect, icon, color);
        let text = egui::RichText::new(label).color(color);
        let response = ui
            .add(
                egui::Label::new(text)
                    .sense(egui::Sense::hover())
                    .truncate(),
            )
            .on_hover_text(details);
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, label));
    });
}

#[derive(Clone, Copy)]
pub(super) enum Column {
    Name,
    Status,
    Check,
    Action,
    /// Status through Check: a field beside an action.
    Field,
    /// Status to the right edge: a field or line with no action.
    Rest,
}
/// Fixed columns shared by every agent row, so names, status dots, check
/// icons and actions line up whether or not a row has each control.
#[derive(Clone, Copy)]
pub(super) struct AgentGrid {
    height: f32,
    gap: f32,
    name: f32,
    check: f32,
    action: egui::Vec2,
}
impl AgentGrid {
    pub(super) fn new(ui: &egui::Ui) -> Self {
        let spacing = ui.spacing();
        let measure = |text: &str, style: egui::TextStyle| {
            ui.fonts_mut(|f| {
                f.layout_no_wrap(text.into(), style.resolve(ui.style()), egui::Color32::WHITE)
                    .size()
                    .x
            })
        };
        let radio = spacing.icon_width + spacing.icon_spacing;
        let name = VENDORS
            .iter()
            .map(|v| radio + measure(v.name, egui::TextStyle::Body))
            .fold(0.0, f32::max);
        let action = ["Sign out", "Sign in", "Install", "Cancel"]
            .into_iter()
            .map(|t| measure(t, egui::TextStyle::Button))
            .fold(0.0, f32::max)
            + 2.0 * spacing.button_padding.x;
        Self {
            height: 28.0,
            gap: 12.0,
            name: name.ceil(),
            check: 28.0,
            action: egui::vec2(action.ceil(), 22.0),
        }
    }
    /// The same columns with shorter rows, for checkboxes and notes.
    fn compact(self) -> Self {
        Self {
            height: 24.0,
            ..self
        }
    }
    /// A row with `label` in the name column and `add` in `column`.
    pub(super) fn field(
        &self,
        ui: &mut egui::Ui,
        label: &str,
        column: Column,
        add: impl FnOnce(&mut egui::Ui),
    ) {
        let mut add = Some(add);
        self.row(ui, |ui, cell| {
            if !label.is_empty() {
                cell(ui, Column::Name, &mut |ui| {
                    ui.weak(label);
                });
            }
            cell(ui, column, &mut |ui| {
                if let Some(add) = add.take() {
                    add(ui);
                }
            });
        });
    }
    /// One row of `height`; `add` fills any of its cells.
    fn row(
        &self,
        ui: &mut egui::Ui,
        add: impl FnOnce(
            &mut egui::Ui,
            &mut dyn FnMut(&mut egui::Ui, Column, &mut dyn FnMut(&mut egui::Ui)),
        ),
    ) {
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), self.height),
            egui::Sense::hover(),
        );
        let action_left = rect.right() - self.action.x;
        let check_left = action_left - self.gap / 2.0 - self.check;
        let status_left = rect.left() + self.name + self.gap;
        let mut cell = |ui: &mut egui::Ui,
                        column: Column,
                        contents: &mut dyn FnMut(&mut egui::Ui)| {
            let (left, right) = match column {
                Column::Name => (rect.left(), status_left - self.gap),
                Column::Status => (status_left, check_left - self.gap / 2.0),
                Column::Check => (check_left, check_left + self.check),
                Column::Action => (action_left, rect.right()),
                Column::Field => (status_left, action_left - self.gap / 2.0),
                Column::Rest => (status_left, rect.right()),
            };
            let layout = match column {
                Column::Check => egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
                Column::Action => egui::Layout::right_to_left(egui::Align::Center),
                _ => egui::Layout::left_to_right(egui::Align::Center),
            };
            let area = egui::Rect::from_x_y_ranges(left..=right.max(left), rect.y_range());
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .id_salt(column as u8)
                    .max_rect(area)
                    .layout(layout),
            );
            child.set_clip_rect(area.intersect(ui.clip_rect()));
            // Keep each widget inside its row, so neighbouring rows never overlap.
            let fit = child.spacing().interact_size.y.min(self.height);
            child.spacing_mut().interact_size.y = fit;
            contents(&mut child);
        };
        add(ui, &mut cell);
    }
}
