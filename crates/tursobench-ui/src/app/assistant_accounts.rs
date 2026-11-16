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
    pub(super) fn assistant_agents_ui(&mut self, ui: &mut egui::Ui) {
        if !self.accounts.view_open {
            self.accounts.view_open = true;
            if self.accounts.automatic {
                self.scan_assistant(ui.ctx());
            }
        }
        let mut selected = self.preferences.assistant.provider;
        let mut action = None;
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
            tursobench_assistant::vendor::forget_locations();
            self.scan_assistant(ui.ctx());
        }
        for vendor in VENDORS {
            ui.push_id(vendor.id, |ui| {
                let row = self.accounts.rows.entry(vendor.id).or_default();
                let state = row
                    .status
                    .as_ref()
                    .map(|s| s.state)
                    .unwrap_or(SignIn::Unknown);
                let busy = row.job.is_some();
                let (label, intent) = row.describe().unwrap_or(("Not checked", Intent::Neutral));
                grid.row(ui, |ui, cell| {
                    cell(ui, Column::Name, &mut |ui| {
                        ui.radio_value(&mut selected, vendor.id, vendor.name);
                    });
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
                    cell(ui, Column::Status, &mut |ui| {
                        status_dot(ui, intent, label);
                        ui.add(egui::Label::new(egui::RichText::new(label).small()).truncate())
                            .on_hover_text(row.error.clone().unwrap_or_else(|| details.clone()));
                    });
                    if !busy && state != SignIn::Missing {
                        cell(ui, Column::Check, &mut |ui| {
                            if icons::control(ui, Icon::Refresh, "Check agent", "", false, false)
                                .clicked()
                            {
                                action = Some((vendor.id, Operation::Check));
                            }
                        });
                    }
                    cell(ui, Column::Action, &mut |ui| {
                        let button = |ui: &mut egui::Ui, text: &str| {
                            ui.add(egui::Button::new(text).small().min_size(grid.action))
                        };
                        if busy {
                            if button(ui, "Cancel").clicked()
                                && let Some(job) = &row.job
                            {
                                job.cancel();
                            }
                        } else if state == SignIn::Missing {
                            if button(ui, "Install").on_hover_text(vendor.help).clicked() {
                                ui.ctx().open_url(egui::OpenUrl::new_tab(vendor.help));
                            }
                        } else if state == SignIn::SignedIn {
                            // ACP has no sign-out; keep the slot so the grid stays whole.
                            let own = matches!(vendor.id, Provider::Claude | Provider::Codex);
                            if ui
                                .add_enabled(
                                    own,
                                    egui::Button::new("Sign out").small().min_size(grid.action),
                                )
                                .on_disabled_hover_text(
                                    "Use the agent's own terminal command to sign out",
                                )
                                .clicked()
                            {
                                action = Some((vendor.id, Operation::Logout));
                            }
                        } else if button(ui, "Sign in").clicked() {
                            action = Some((
                                vendor.id,
                                Operation::Login {
                                    method: row.method.clone(),
                                },
                            ));
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
    }
    /// The agent the settings area edits; the chat's agent until one is chosen.
    pub(super) fn configured_agent(&mut self) -> Provider {
        *self
            .accounts
            .configuring
            .get_or_insert(self.preferences.assistant.provider)
    }
    /// How `provider` would be started, whichever agent the chat uses.
    pub(super) fn profile_for(
        &self,
        provider: Provider,
    ) -> Result<super::assistant::Profile, String> {
        let mut preferences = self.preferences.assistant.clone();
        preferences.provider = provider;
        profile(&preferences)
    }
    /// The configured agent's sign-in problems and choices, in its settings area.
    pub(super) fn assistant_agent_status(&mut self, ui: &mut egui::Ui) {
        let grid = AgentGrid::new(ui);
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
                let mut choice = row
                    .method
                    .clone()
                    .unwrap_or_else(|| status.auth_methods[0].id.clone());
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
                row.method = Some(choice);
            }
        }
        if let Some(url) = &row.browser {
            grid.field(ui, "", Column::Rest, |ui| {
                ui.hyperlink_to("Open sign-in page again", url);
            });
        }
    }
    /// The configured agent's executable, terminal sign-in and path override.
    pub(super) fn assistant_agent_program(&mut self, ui: &mut egui::Ui) {
        let provider = self.configured_agent();
        let profile = self.profile_for(provider).ok();
        {
            {
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
                let grid = AgentGrid::new(ui);
                let mut toggle = false;
                if let Some(profile) = &profile {
                    let full = profile.program.to_string_lossy();
                    let short = profile
                        .program
                        .file_name()
                        .map_or_else(|| full.clone(), |n| n.to_string_lossy());
                    grid.row(ui, |ui, cell| {
                        cell(ui, Column::Name, &mut |ui| {
                            ui.weak("Program");
                        });
                        cell(ui, Column::Status, &mut |ui| {
                            ui.add(
                                egui::Label::new(egui::RichText::new(short.as_ref()).monospace())
                                    .truncate(),
                            )
                            .on_hover_text(if custom {
                                format!("{full}\nYour path")
                            } else {
                                format!("{full}\nFound automatically")
                            });
                        });
                        cell(ui, Column::Check, &mut |ui| {
                            toggle = icons::control(
                                ui,
                                Icon::Edit,
                                "Choose executable",
                                "",
                                self.accounts.path_open,
                                false,
                            )
                            .clicked();
                        });
                    });
                    let login = provider.vendor().login;
                    let shown = std::iter::once(short.as_ref())
                        .chain(login.iter().copied())
                        .collect::<Vec<_>>()
                        .join(" ");
                    grid.row(ui, |ui, cell| {
                        cell(ui, Column::Name, &mut |ui| {
                            ui.weak("Terminal");
                        });
                        cell(ui, Column::Status, &mut |ui| {
                            ui.add(
                                egui::Label::new(egui::RichText::new(&shown).monospace())
                                    .truncate(),
                            )
                            .on_hover_text("Run this in a terminal to sign in there instead");
                        });
                        cell(ui, Column::Check, &mut |ui| {
                            if icons::control(
                                ui,
                                Icon::Copy,
                                "Copy sign-in command",
                                "",
                                false,
                                false,
                            )
                            .on_hover_text("Copies the full path, quoted for a shell")
                            .clicked()
                            {
                                ui.ctx().copy_text(shell_command(&profile.program, login));
                            }
                        });
                    });
                }
                if toggle {
                    self.accounts.path_open = !self.accounts.path_open;
                }
                if !(self.accounts.path_open || custom) {
                    return;
                }
                let mut apply = false;
                grid.row(ui, |ui, cell| {
                    cell(ui, Column::Field, &mut |ui| {
                        let response = ui.add(
                            egui::TextEdit::singleline(&mut self.accounts.executable)
                                .hint_text("Absolute path; empty finds it automatically")
                                .desired_width(f32::INFINITY),
                        );
                        response.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::Role::TextInput,
                                true,
                                "Executable path",
                            )
                        });
                        apply |=
                            response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    });
                    cell(ui, Column::Action, &mut |ui| {
                        apply |= ui
                            .add(egui::Button::new("Apply").small().min_size(grid.action))
                            .clicked();
                    });
                });
                {
                    if apply {
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
                            if provider == self.preferences.assistant.provider {
                                self.reset_assistant_conversations();
                            }
                            self.preferences.normalize();
                            self.store.save(&self.preferences);
                            // The old executable's status says nothing about the new one.
                            tursobench_assistant::vendor::forget_locations();
                            self.accounts.rows.remove(&provider);
                            self.accounts.path_open = false;
                            self.agent_account_job(ui.ctx(), provider, Operation::Check);
                        } else {
                            self.accounts.rows.entry(provider).or_default().error =
                                Some("Use an absolute executable path".into());
                        }
                    }
                }
            }
        }
    }
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
    pub(super) fn compact(self) -> Self {
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
        let span = |left: f32, right: f32| {
            egui::Rect::from_x_y_ranges(left..=right.max(left), rect.y_range())
        };
        let mut cell =
            |ui: &mut egui::Ui, column: Column, contents: &mut dyn FnMut(&mut egui::Ui)| {
                let (area, layout) = match column {
                    Column::Name => (
                        span(rect.left(), status_left - self.gap),
                        egui::Layout::left_to_right(egui::Align::Center),
                    ),
                    Column::Status => (
                        span(status_left, check_left - self.gap / 2.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                    ),
                    Column::Check => (
                        span(check_left, check_left + self.check),
                        egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
                    ),
                    Column::Action => (
                        span(action_left, rect.right()),
                        egui::Layout::right_to_left(egui::Align::Center),
                    ),
                    Column::Field => (
                        span(status_left, action_left - self.gap / 2.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                    ),
                    Column::Rest => (
                        span(status_left, rect.right()),
                        egui::Layout::left_to_right(egui::Align::Center),
                    ),
                };
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
fn shell_command(program: &std::path::Path, args: &[&str]) -> String {
    let quote = |word: &str| format!("'{}'", word.replace('\'', "'\\''"));
    std::iter::once(quote(&program.to_string_lossy()))
        .chain(args.iter().map(|s| quote(s)))
        .collect::<Vec<_>>()
        .join(" ")
}
