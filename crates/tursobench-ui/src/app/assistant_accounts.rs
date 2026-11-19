//! App-wide discovery/account jobs; never share the conversation's tool host.
use super::{
    App,
    presentation::{self, Intent},
};
use crate::storage::Provider;
use eframe::egui;
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};
use tursobench_assistant::{
    account::{Event, Job, Operation, SignIn, Status, Target},
    agent::Protocol,
    vendor::{VENDORS, command_words, locate},
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
    /// The agent the settings area edits; the chat's agent until one is chosen.
    pub(super) fn configured_agent(&mut self) -> Provider {
        *self
            .accounts
            .configuring
            .get_or_insert(self.preferences.assistant.provider)
    }
    /// How `provider` would be started, whichever agent the chat uses.
    pub(super) fn profile_for(&self, provider: Provider) -> Result<Profile, String> {
        let mut preferences = self.preferences.assistant.clone();
        preferences.provider = provider;
        profile(&preferences)
    }
    /// How the chat's agent would be started.
    pub(super) fn assistant_profile(&self) -> Result<Profile, String> {
        profile(&self.preferences.assistant)
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
    /// Settings → Assistant.
    pub(super) fn open_assistant_settings(&mut self) {
        self.settings.open = true;
        self.settings.page = super::settings::Page::Assistant;
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

fn profile(preferences: &crate::storage::AssistantPreferences) -> Result<Profile, String> {
    let vendor = preferences.provider.vendor();
    let mut words: Vec<String> = if preferences.provider == Provider::Custom {
        command_words(&preferences.custom_command)?
    } else {
        std::iter::once(vendor.command.to_owned())
            .chain(vendor.args.iter().map(|s| (*s).to_owned()))
            .collect()
    };
    let command = words.remove(0);
    let requested = preferences
        .executables
        .get(&preferences.provider)
        .cloned()
        .unwrap_or_else(|| command.clone().into());
    let program = locate(&requested).unwrap_or(requested);
    if preferences.provider == Provider::Antigravity
        && preferences.privacy.of(vendor.id).telemetry_off
    {
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
