//! App-wide discovery/account jobs; never share the conversation's tool host.
use super::{
    App,
    assistant::profile,
    icons::{self, Icon},
    presentation::{self, Intent},
};
use crate::storage::Provider;
use eframe::egui;
use std::{collections::BTreeMap, sync::Arc};
use tursobench_assistant::{
    account::{Event, Job, Operation, SignIn, Status, Target},
    vendor::VENDORS,
};

#[derive(Default)]
pub(super) struct Row {
    pub status: Option<Status>,
    pub job: Option<Job>,
    pub error: Option<String>,
    pub browser: Option<String>,
    pub method: Option<String>,
    pub activity: Option<&'static str>,
    pub changing_account: bool,
}
#[derive(Default)]
pub(super) struct State {
    pub rows: BTreeMap<Provider, Row>,
    pub view_open: bool,
    pub executable: String,
    pub editing: Option<Provider>,
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
    fn account_target(&self, provider: Provider) -> Result<Target, String> {
        let mut preferences = self.preferences.assistant.clone();
        preferences.provider = provider;
        let profile = profile(&preferences)?;
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
        if !matches!(operation, Operation::Check) {
            self.reset_assistant_conversations();
        }
        let target = self.account_target(provider);
        let row = self.accounts.rows.entry(provider).or_default();
        row.changing_account = !matches!(operation, Operation::Check);
        row.activity = Some(match &operation {
            Operation::Check => "Checking…",
            Operation::Login { .. } => "Signing in…",
            Operation::Logout => "Signing out…",
        });
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
        let mut account_changed = false;
        if !self.settings.open || self.settings.page != super::settings::Page::Assistant {
            self.accounts.view_open = false;
        }
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
                    Event::Finished(result) => {
                        account_changed |= row.changing_account;
                        row.changing_account = false;
                        row.job = None;
                        row.activity = None;
                        row.browser = None;
                        match result {
                            Ok(status) => {
                                row.error = None;
                                row.status = Some(status);
                            }
                            Err(error) => {
                                row.error = Some(error);
                                row.status = None;
                            }
                        }
                    }
                }
            }
        }
        if account_changed {
            self.reset_assistant_conversations();
        }
    }
    pub(super) fn assistant_account_pending(&self) -> bool {
        self.accounts
            .rows
            .values()
            .any(|row| row.changing_account && row.job.is_some())
    }
    pub(super) fn assistant_agents_ui(&mut self, ui: &mut egui::Ui) {
        if !self.accounts.view_open {
            self.accounts.view_open = true;
            if !cfg!(test) && std::env::var_os("TURSOBENCH_ASSISTANT_CAPTURE").is_none() {
                self.scan_assistant(ui.ctx());
            }
        }
        let mut selected = self.preferences.assistant.provider;
        let mut action = None;
        ui.horizontal(|ui| {
            if ui.button("Scan again").on_hover_text("Search PATH and common installation folders; ask each CLI for sign-in status. An ACP check may create an empty session in its history.").clicked() { self.scan_assistant(ui.ctx()); }
            ui.weak("Local agents");
        });
        for vendor in VENDORS {
            ui.push_id(vendor.id, |ui| {
                let row = self.accounts.rows.entry(vendor.id).or_default();
                let state = row
                    .status
                    .as_ref()
                    .map(|s| s.state)
                    .unwrap_or(SignIn::Unknown);
                let busy = row.job.is_some();
                let (label, intent) = if busy {
                    (row.activity.unwrap_or("Checking…"), Intent::Pending)
                } else {
                    match state {
                        SignIn::SignedIn => ("Signed in", Intent::Primary),
                        SignIn::SignedOut => ("Signed out", Intent::Pending),
                        SignIn::Missing => ("Not installed", Intent::Neutral),
                        SignIn::Unknown
                            if row.error.is_some()
                                || row.status.as_ref().is_some_and(|s| s.issue.is_some()) =>
                        {
                            ("Check failed", Intent::Danger)
                        }
                        SignIn::Unknown if row.status.is_some() => {
                            ("Status unavailable", Intent::Neutral)
                        }
                        SignIn::Unknown => ("Not checked", Intent::Neutral),
                    }
                };
                ui.horizontal(|ui| {
                    ui.radio_value(&mut selected, vendor.id, vendor.name);
                    let color = presentation::intent_color(ui, intent);
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                    ui.painter().circle_filled(rect.center(), 3.0, color);
                    let details = row
                        .status
                        .as_ref()
                        .map(|s| {
                            [
                                s.version.as_deref(),
                                s.method.as_deref(),
                                s.plan.as_deref(),
                                s.issue.as_deref(),
                            ]
                            .into_iter()
                            .flatten()
                            .collect::<Vec<_>>()
                            .join(" · ")
                        })
                        .unwrap_or_default();
                    ui.small(label)
                        .on_hover_text(if let Some(error) = &row.error {
                            error.clone()
                        } else {
                            details
                        });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if busy {
                            if ui.small_button("Cancel").clicked()
                                && let Some(job) = &row.job
                            {
                                job.cancel();
                            }
                        } else if state == SignIn::Missing {
                            ui.hyperlink_to("Install", vendor.help);
                        } else {
                            if state == SignIn::SignedIn
                                && matches!(vendor.id, Provider::Claude | Provider::Codex)
                            {
                                if ui.small_button("Sign out").clicked() {
                                    action = Some((vendor.id, Operation::Logout));
                                }
                            } else if state != SignIn::SignedIn
                                && ui.small_button("Sign in").clicked()
                            {
                                action = Some((
                                    vendor.id,
                                    Operation::Login {
                                        method: row.method.clone(),
                                    },
                                ));
                            }
                            if icons::control(ui, Icon::Refresh, "Check agent", "", false, false)
                                .clicked()
                            {
                                action = Some((vendor.id, Operation::Check));
                            }
                        }
                    });
                });
            });
        }
        if selected != self.preferences.assistant.provider {
            self.select_assistant_provider(selected);
        }
        if let Some((provider, operation)) = action {
            self.agent_account_job(ui.ctx(), provider, operation);
        }
        let provider = self.preferences.assistant.provider;
        if let Some(row) = self.accounts.rows.get_mut(&provider) {
            if row
                .status
                .as_ref()
                .is_some_and(|s| s.state == SignIn::SignedIn)
                && matches!(
                    provider,
                    Provider::Antigravity | Provider::Copilot | Provider::Custom
                )
            {
                ui.small("Use the agent's own terminal command to sign out.");
            }
            if let Some(error) = &row.error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            if let Some(status) = &row.status {
                if let Some(issue) = &status.issue {
                    ui.small(issue);
                }
                if status.auth_methods.len() > 1 {
                    let mut choice = row
                        .method
                        .clone()
                        .unwrap_or_else(|| status.auth_methods[0].id.clone());
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
                    row.method = Some(choice);
                }
            }
            if let Some(url) = &row.browser {
                ui.hyperlink_to("Open sign-in page again", url);
            }
        }
        let profile = profile(&self.preferences.assistant).ok();
        egui::CollapsingHeader::new("Executable and terminal sign-in")
            .id_salt("agent-executable")
            .show(ui, |ui| {
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
                if let Some(profile) = &profile {
                    ui.small(profile.program.to_string_lossy()).on_hover_text(
                        "Resolved executable; use an absolute path below to override it",
                    );
                    let command = shell_command(&profile.program, provider.vendor().login);
                    ui.horizontal_wrapped(|ui| {
                        ui.monospace(&command);
                        if ui.small_button("Copy command").clicked() {
                            ui.ctx().copy_text(command.clone());
                        }
                    });
                }
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.accounts.executable)
                            .hint_text("Absolute executable path (optional)")
                            .desired_width(230.0),
                    );
                    if ui.button("Apply path").clicked() {
                        let path = std::path::PathBuf::from(self.accounts.executable.trim());
                        if self.accounts.executable.trim().is_empty() || path.is_absolute() {
                            if path.as_os_str().is_empty() {
                                self.preferences.assistant.executables.remove(&provider);
                            } else {
                                self.preferences
                                    .assistant
                                    .executables
                                    .insert(provider, path);
                            }
                            if provider == Provider::Custom {
                                self.preferences.assistant.clear_custom_grants();
                            }
                            self.reset_assistant_conversations();
                            self.preferences.normalize();
                            self.store.save(&self.preferences);
                            self.agent_account_job(ui.ctx(), provider, Operation::Check);
                        } else {
                            self.accounts.rows.entry(provider).or_default().error =
                                Some("Use an absolute executable path".into());
                        }
                    }
                });
            });
    }
}
fn shell_command(program: &std::path::Path, args: &[&str]) -> String {
    let quote = |word: &str| format!("'{}'", word.replace('\'', "'\\''"));
    std::iter::once(quote(&program.to_string_lossy()))
        .chain(args.iter().map(|s| quote(s)))
        .collect::<Vec<_>>()
        .join(" ")
}
