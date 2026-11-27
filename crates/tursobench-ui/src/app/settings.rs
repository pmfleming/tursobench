//! Application preferences live outside any database workspace.
use super::{
    App,
    icons::{self, Icon},
    panels::{card, section},
};
use crate::appearance::Mode;
use eframe::egui;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Page {
    #[default]
    Appearance,
    Discovery,
    Resources,
    Queries,
    Assistant,
}

#[derive(Default)]
pub(super) struct State {
    pub open: bool,
    pub page: Page,
    /// Scroll the assistant page to its privacy section on the next frame.
    pub reveal_privacy: bool,
    dirty: bool,
    displayed_font: Option<u16>,
}

impl App {
    pub(super) fn settings_window(&mut self, ctx: &egui::Context) {
        // Settings → Assistant checks the agents again each time it opens.
        self.accounts.view_open &= self.settings.open && self.settings.page == Page::Assistant;
        if !self.settings.open {
            self.save_settings();
            return;
        }
        let mut open = true;
        let scale = f32::from(self.preferences.font_size) / 14.0;
        let size = (egui::vec2(640.0, 540.0) * scale)
            .min(ctx.content_rect().size() - egui::vec2(32.0, 56.0));
        let previous_font = self
            .settings
            .displayed_font
            .replace(self.preferences.font_size);
        let mut window = egui::Window::new("Settings")
            .id(egui::Id::unique("application-settings"))
            .frame(super::presentation::window_frame(ctx))
            .default_pos(ctx.content_rect().center() - size * 0.5)
            .default_size(size)
            .min_size(egui::vec2(280.0, 240.0))
            .max_size(ctx.content_rect().size() - egui::vec2(24.0, 48.0))
            .open(&mut open);
        if let Some(previous) =
            previous_font.filter(|previous| *previous != self.preferences.font_size)
            && let Some(rect) =
                ctx.memory(|m| m.area_rect(egui::Id::unique("application-settings")))
        {
            let scaled = (rect.size()
                * (f32::from(self.preferences.font_size) / f32::from(previous)))
            .min(ctx.content_rect().size() - egui::vec2(32.0, 56.0));
            window = window.fixed_size(scaled);
        }
        window.show(ctx, |ui| self.settings_ui(ui));
        self.settings.open = open;
        if !open {
            self.save_settings();
        }
    }

    pub(super) fn settings_ui(&mut self, ui: &mut egui::Ui) {
        let previous_page = self.settings.page;
        let scale = f32::from(self.preferences.font_size) / 14.0;
        if ui.available_width() < 480.0 {
            ui.horizontal(|ui| page_buttons(ui, &mut self.settings.page));
            ui.separator();
            self.settings_page(ui);
        } else {
            ui.horizontal_top(|ui| {
                let height = ui.available_height();
                // Icons only, named on hover, as everywhere else in the app.
                ui.allocate_ui_with_layout(
                    egui::vec2(36.0 * scale, height),
                    egui::Layout::top_down(egui::Align::Center),
                    |ui| page_buttons(ui, &mut self.settings.page),
                );
                ui.separator();
                ui.allocate_ui_with_layout(
                    ui.available_size(),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| self.settings_page(ui),
                );
            });
        }
        if previous_page != self.settings.page {
            self.save_settings();
        }
    }

    fn settings_page(&mut self, ui: &mut egui::Ui) {
        if self.settings.page == Page::Discovery {
            // Discovery manages its own folder-list scroll area.
            self.discovery_settings_ui(ui);
            return;
        }
        egui::ScrollArea::vertical()
            .id_salt(("settings-page", self.settings.page as u8))
            .auto_shrink([false, false])
            .show(ui, |ui| match self.settings.page {
                Page::Appearance => self.appearance_settings(ui),
                Page::Resources => self.resource_settings(ui),
                Page::Queries => self.query_settings(ui),
                Page::Assistant => self.assistant_settings(ui),
                Page::Discovery => unreachable!(),
            });
    }

    /// Settings → Assistant.
    pub(super) fn open_assistant_settings(&mut self) {
        self.settings.open = true;
        self.settings.page = Page::Assistant;
    }

    fn assistant_settings(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.y = 4.0;
        section(ui, "Agents", |ui| self.assistant_agents_ui(ui));
        // The chat panel's privacy link opens on the chat agent's settings.
        if std::mem::take(&mut self.settings.reveal_privacy) {
            ui.scroll_to_cursor(Some(egui::Align::TOP));
        }
        section(ui, "Agent settings", |ui| self.agent_settings_ui(ui));
        section(ui, "Chat", |ui| self.chat_settings_ui(ui));
        section(ui, "Sharing", |ui| self.sharing_settings_ui(ui));
    }

    pub(super) fn save_settings(&mut self) {
        if std::mem::take(&mut self.settings.dirty) {
            self.preferences.normalize();
            self.store.save(&self.preferences);
        }
    }

    pub(super) fn setting_response(&mut self, response: egui::Response) {
        self.settings.dirty |= response.changed();
        if response.lost_focus()
            || response.drag_stopped()
            || response.changed() && !response.has_focus() && !response.dragged()
        {
            self.save_settings();
        }
    }

    fn appearance_settings(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("Theme");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.weak("Aa").on_hover_text(&self.database.ui.font_label);
                    self.theme_buttons(ui);
                });
            });
            let before = self.preferences.font_size;
            let response = preference_row(
                ui,
                "Font size",
                "Type or drag to change text size",
                false,
                |ui| font_size_controls(ui, &mut self.preferences.font_size),
            );
            if before != self.preferences.font_size {
                self.apply_theme(ui.ctx());
                self.refit_all_columns();
                self.settings.dirty = true;
                if !response.changed() {
                    self.save_settings();
                }
            }
            self.setting_response(response);
            let response = preference_row(
                ui,
                "NULL marker",
                "Display marker only; stored values are unchanged",
                false,
                |ui| {
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut self.preferences.null_display)
                            .char_limit(32)
                            .desired_width(90.0),
                    );
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::TextInput, true, "NULL marker")
                    });
                    response
                },
            );
            self.setting_response(response);
        });
    }

    fn theme_buttons(&mut self, ui: &mut egui::Ui) {
        for (mode, icon, name) in [
            (Mode::Dark, Icon::Moon, "Dark theme"),
            (Mode::Light, Icon::Sun, "Light theme"),
            (Mode::System, Icon::Desktop, "Follow system"),
        ] {
            let chosen = self.preferences.appearance.unwrap_or_default() == mode;
            if icons::control(ui, icon, name, "", chosen, false).clicked() {
                self.preferences.appearance = Some(mode);
                self.apply_theme(ui.ctx());
                self.settings.dirty = true;
                self.save_settings();
            }
        }
    }

    /// Font size is global; refit columns in every retained workspace.
    fn refit_all_columns(&mut self) {
        for database in std::iter::once(&mut self.database).chain(self.parked.values_mut()) {
            for document in
                std::iter::once(&mut database.document).chain(database.documents.values_mut())
            {
                document.tools.columns.refit = true;
                for columns in document.tools.column_layouts.values_mut() {
                    columns.refit = true;
                }
            }
        }
    }

    fn resource_settings(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            let automatic =
                self.memory.automatic_budget_mib() / crate::resources::DATABASE_RESERVATION_MIB;
            let mut count = self.memory.limits(&self.preferences).database_limit;
            let maximum = usize::try_from(
                self.memory
                    .available_mib
                    .or(self.memory.total_mib)
                    .unwrap_or_else(|| self.memory.automatic_budget_mib().saturating_mul(4))
                    / crate::resources::DATABASE_RESERVATION_MIB,
            )
            .unwrap_or(usize::MAX)
            .max(count)
            .max(1);
            ui.horizontal(|ui| {
                ui.strong(format!("{count} databases"));
                icons::hint(
                    ui,
                    Icon::Refresh,
                    13.0,
                    "Changes apply to new opens. Existing work stays open.",
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled_ui(
                            self.preferences.database_limit.is_some()
                                || self.preferences.memory_budget_mib.is_some(),
                            |ui| icons::button(ui, Icon::Undo, "Reset database resources", ""),
                        )
                        .inner
                        .on_hover_text(format!(
                            "Restore the system recommendation: {automatic} databases"
                        ))
                        .clicked()
                    {
                        self.preferences.database_limit = None;
                        self.preferences.memory_budget_mib = None;
                        count = self.memory.limits(&self.preferences).database_limit;
                        self.settings.dirty = true;
                        self.save_settings();
                    }
                });
            });
            let minimum = usize::from(count > 0);
            let width = ui.available_width();
            let response = themed_slider(
                ui,
                width,
                egui::Slider::new(&mut count, minimum..=maximum)
                    .show_value(false)
                    .trailing_fill(true),
            );
            response.widget_info(|| {
                egui::WidgetInfo::slider(response.enabled(), count as f64, "Maximum open databases")
            });
            let detected = match (self.memory.available_mib, self.memory.total_mib) {
                (Some(available), Some(total)) => format!(
                    "At startup: {} available / {} total.{}",
                    crate::resources::format_mib(available),
                    crate::resources::format_mib(total),
                    if self.memory.process_limited {
                        " Process memory limits included."
                    } else {
                        ""
                    }
                ),
                _ => format!(
                    "Available memory could not be detected. Fallback budget: {}.",
                    crate::resources::format_mib(self.memory.automatic_budget_mib())
                ),
            };
            let recommendation = format!(
                "{detected} System recommendation: {automatic} databases. Uses 25% of memory available at startup. 512 MiB per database; actual use varies."
            );
            let track = response
                .rect
                .shrink2(egui::vec2(response.rect.height() / 2.0, 0.0));
            let fraction = ((automatic as f32 - minimum as f32)
                / (maximum.saturating_sub(minimum).max(1)) as f32)
                .clamp(0.0, 1.0);
            let x = egui::lerp(track.x_range(), fraction);
            ui.painter().vline(
                x,
                response.rect.y_range(),
                egui::Stroke::new(2.0, crate::appearance::Tokens::of(ui.ctx()).muted),
            );
            ui.interact(
                egui::Rect::from_center_size(
                    egui::pos2(x, response.rect.center().y),
                    egui::vec2(8.0, response.rect.height()),
                ),
                ui.scope_id().with("recommendation"),
                egui::Sense::hover(),
            )
            .on_hover_text(recommendation);
            if response.changed() {
                self.preferences.database_limit = Some(count.max(1));
                self.preferences.memory_budget_mib = Some(
                    (count as u64)
                        .saturating_mul(crate::resources::DATABASE_RESERVATION_MIB)
                        .max(1),
                );
            }
            self.setting_response(response);
            let estimated = (self.memory.limits(&self.preferences).database_limit as u64)
                .saturating_mul(crate::resources::DATABASE_RESERVATION_MIB);
            ui.horizontal_wrapped(|ui| {
                ui.strong(format!(
                    "{} × {} MiB ≈ {}",
                    self.memory.limits(&self.preferences).database_limit,
                    crate::resources::DATABASE_RESERVATION_MIB,
                    crate::resources::format_mib(estimated)
                ));
                ui.weak("estimated")
                    .on_hover_text("512 MiB per database; actual use varies.");
            });
            ui.small("Changes apply to new opens. Existing work stays open.");
        });
    }

    fn query_settings(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.y = 3.0;
        section(ui, "Limits", |ui| {
            let columns = LimitColumns::measure(ui, &Limit::ALL.map(|l| l.slider().label));
            for limit in Limit::ALL {
                let mut value = limit.get(&self.preferences.limits(""));
                let (response, _) = limit_row(ui, &columns, limit.slider(), &mut value, None);
                limit.set_default(&mut self.preferences, value);
                self.setting_response(response);
            }
            ui.small("Defaults for every database; each can set its own in its Limits panel.");
        });
        section(ui, "SQL recovery", |ui| self.recovery_settings(ui));
        section(ui, "All databases", |ui| {
            if clear_queries_row(
                ui,
                Icon::History,
                "History",
                self.preferences.query_history.len(),
                "Clear history for all databases",
            ) {
                self.preferences.query_history.clear();
                self.store.save(&self.preferences);
            }
            if clear_queries_row(
                ui,
                Icon::Saved,
                "Saved queries",
                self.preferences.saved_queries.len(),
                "Clear saved queries for all databases",
            ) {
                self.preferences.saved_queries.clear();
                self.store.save(&self.preferences);
            }
        });
    }
}

fn clear_queries_row(ui: &mut egui::Ui, icon: Icon, title: &str, count: usize, name: &str) -> bool {
    ui.push_id(title, |ui| {
        ui.horizontal_wrapped(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
            icons::paint(ui.painter(), rect, icon, ui.visuals().weak_text_color());
            ui.label(format!("{title} · {count}"));
            let response = ui.add_enabled(count > 0, egui::Button::new("Clear for all databases"));
            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, count > 0, name));
            response.on_hover_text(name).clicked()
        })
        .inner
    })
    .inner
}

fn pages() -> [(Page, Icon, &'static str, &'static str); 5] {
    [
        (Page::Appearance, Icon::Sun, "Appearance", "Appearance"),
        (Page::Discovery, Icon::Search, "Discovery", "Discovery"),
        (
            Page::Resources,
            Icon::Database,
            "Resources",
            "Database resources",
        ),
        (
            Page::Queries,
            Icon::Sql,
            "Queries",
            "Queries and SQL recovery",
        ),
        (
            Page::Assistant,
            Icon::Assistant,
            "Assistant",
            "Assistant agent and sharing",
        ),
    ]
}

/// One icon per settings page; the chosen page reads as selected.
fn page_buttons(ui: &mut egui::Ui, current: &mut Page) {
    for (page, icon, label, name) in pages() {
        if icons::control(ui, icon, name, "", *current == page, false)
            .on_hover_text(label)
            .clicked()
        {
            *current = page;
        }
    }
}

/// The font size field between its step buttons, with a reset to 14 pt.
fn font_size_controls(ui: &mut egui::Ui, font_size: &mut u16) -> egui::Response {
    let increase = ui
        .add_enabled_ui(*font_size < 20, |ui| {
            icons::button(ui, Icon::Plus, "Increase font size", "")
        })
        .inner
        .clicked();
    let size = ui.add(egui::DragValue::new(font_size).range(10..=20).suffix(" pt"));
    size.widget_info(|| egui::WidgetInfo::labeled(egui::Role::SpinButton, true, "Font size"));
    let decrease = ui
        .add_enabled_ui(*font_size > 10, |ui| {
            icons::button(ui, Icon::Minimize, "Decrease font size", "")
        })
        .inner
        .clicked();
    let reset = icons::button(ui, Icon::Undo, "Reset font size", "").clicked();
    if increase {
        *font_size += 1;
    }
    if decrease {
        *font_size -= 1;
    }
    if reset {
        *font_size = 14;
    }
    size
}

fn preference_row(
    ui: &mut egui::Ui,
    label: &str,
    tooltip: &str,
    reopen: bool,
    control: impl FnOnce(&mut egui::Ui) -> egui::Response,
) -> egui::Response {
    ui.horizontal(|ui| {
        let label = ui.label(label).on_hover_text(tooltip);
        if reopen {
            icons::hint(ui, Icon::Refresh, 13.0, tooltip);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            control(ui).labelled_by(label.id).on_hover_text(tooltip)
        })
        .inner
    })
    .inner
}

fn themed_slider(ui: &mut egui::Ui, width: f32, slider: egui::Slider<'_>) -> egui::Response {
    ui.scope(|ui| {
        ui.spacing_mut().slider_width = width;
        let tokens = crate::appearance::Tokens::of(ui.ctx());
        ui.visuals_mut().widgets.inactive.bg_fill = tokens.line_strong;
        ui.visuals_mut().selection.bg_fill = tokens.primary;
        ui.add(slider)
    })
    .inner
}

/// Shared label and value widths so the limit sliders line up in one column.
struct LimitColumns {
    label: f32,
    value: f32,
}

impl LimitColumns {
    fn measure(ui: &egui::Ui, labels: &[&str]) -> Self {
        let font = egui::TextStyle::Body.resolve(ui.style());
        let width = |text: &str| {
            ui.painter()
                .layout_no_wrap(text.to_owned(), font.clone(), egui::Color32::WHITE)
                .size()
                .x
        };
        let label = labels.iter().map(|label| width(label)).fold(0.0, f32::max);
        Self {
            // Room for the reopen mark that follows some labels.
            label: label + 13.0 + ui.spacing().item_spacing.x * 2.0,
            value: width("9999 ms"),
        }
    }
}

struct LimitSlider {
    label: &'static str,
    tooltip: &'static str,
    reopen: bool,
    range: std::ops::RangeInclusive<u64>,
    /// Lowest positive stop on the logarithmic track; zero sits at the far left.
    smallest: f64,
    format: fn(u64) -> String,
}

/// One of the query limits, set by default for every database and by any one for itself.
#[derive(Clone, Copy)]
enum Limit {
    Rows,
    Slow,
    Busy,
    Deadline,
}
impl Limit {
    const ALL: [Self; 4] = [Self::Rows, Self::Slow, Self::Busy, Self::Deadline];
    fn slider(self) -> LimitSlider {
        match self {
            Self::Rows => LimitSlider {
                label: "Rows",
                tooltip: "Maximum retained query rows",
                reopen: false,
                range: 1..=10_000,
                smallest: 1.0,
                format: |rows| rows.to_string(),
            },
            Self::Slow => LimitSlider {
                label: "Slow ≥",
                tooltip: "Slow query threshold",
                reopen: false,
                range: 1..=60_000,
                smallest: 1.0,
                format: format_ms,
            },
            Self::Busy => LimitSlider {
                label: "Busy wait",
                tooltip: "Busy timeout; applies after reopening the database. Far left disables.",
                reopen: true,
                range: 0..=5000,
                smallest: 10.0,
                format: format_ms,
            },
            Self::Deadline => LimitSlider {
                label: "Deadline",
                tooltip: "Statement deadline after reopen; far left disables. Prior committed work is not rolled back.",
                reopen: true,
                range: 0..=3_600_000,
                smallest: 100.0,
                format: format_ms,
            },
        }
    }
    fn get(self, limits: &crate::storage::Limits) -> u64 {
        match self {
            Self::Rows => limits.row_limit as u64,
            Self::Slow => limits.slow_query_ms,
            Self::Busy => limits.busy_timeout_ms,
            Self::Deadline => limits.query_timeout_ms,
        }
    }
    fn set_default(self, preferences: &mut crate::storage::Preferences, value: u64) {
        match self {
            Self::Rows => preferences.row_limit = value as usize,
            Self::Slow => preferences.slow_query_ms = value,
            Self::Busy => preferences.busy_timeout_ms = value,
            Self::Deadline => preferences.query_timeout_ms = value,
        }
    }
    /// The database's own value, if it set one.
    fn own(self, database: &crate::storage::DatabasePreferences) -> Option<u64> {
        match self {
            Self::Rows => database.row_limit.map(|rows| rows as u64),
            Self::Slow => database.slow_query_ms,
            Self::Busy => database.busy_timeout_ms,
            Self::Deadline => database.query_timeout_ms,
        }
    }
    fn set_own(self, database: &mut crate::storage::DatabasePreferences, value: Option<u64>) {
        match self {
            Self::Rows => database.row_limit = value.map(|rows| rows as usize),
            Self::Slow => database.slow_query_ms = value,
            Self::Busy => database.busy_timeout_ms = value,
            Self::Deadline => database.query_timeout_ms = value,
        }
    }
}

impl App {
    /// The selected database's own limits, each with a reset to the
    /// default, and whether its large values load without asking.
    pub(super) fn database_limits_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong("Limits");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                icons::hint(
                    ui,
                    Icon::Database,
                    16.0,
                    "This database only; the defaults are in Settings → Queries",
                );
            });
        });
        let key = self.database_key();
        let limits = self.limits();
        let defaults = self.preferences.limits("");
        let own = self.preferences.database(&key);
        let columns = LimitColumns::measure(ui, &Limit::ALL.map(|l| l.slider().label));
        for limit in Limit::ALL {
            let slider = limit.slider();
            let reset = format!("Use the default: {}", (slider.format)(limit.get(&defaults)));
            let mut value = limit.get(&limits);
            let set = limit.own(&own).is_some();
            let (response, reset) =
                limit_row(ui, &columns, slider, &mut value, Some((set, &reset)));
            if response.changed() {
                limit.set_own(self.preferences.database_mut(&key), Some(value));
            }
            if reset {
                limit.set_own(self.preferences.database_mut(&key), None);
                self.settings.dirty = true;
                self.save_settings();
            }
            self.setting_response(response);
        }
        ui.separator();
        let mut auto_load = own.auto_load;
        let response = ui
            .checkbox(&mut auto_load, "Load large values")
            .on_hover_text(
                "Load a value over 64 KiB, up to 16 MiB, as soon as its field is selected. \
             Off, its preview offers a button.",
            );
        if response.changed() {
            self.preferences.database_mut(&key).auto_load = auto_load;
            self.settings.dirty = true;
            self.save_settings();
        }
    }
}

/// Label, logarithmic slider and current value on one line; with `reset`,
/// an undo button, enabled while it is true and named by its text, which
/// returns true when clicked.
fn limit_row(
    ui: &mut egui::Ui,
    columns: &LimitColumns,
    limit: LimitSlider,
    value: &mut u64,
    reset: Option<(bool, &str)>,
) -> (egui::Response, bool) {
    let reset_width = if reset.is_some() {
        24.0 + ui.spacing().item_spacing.x
    } else {
        0.0
    };
    let mut reset_clicked = false;
    let response = ui
        .horizontal(|ui| {
            let height = ui.spacing().interact_size.y;
            let label = ui
                .allocate_ui_with_layout(
                    egui::vec2(columns.label, height),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.set_min_width(columns.label);
                        let label = ui.label(limit.label).on_hover_text(limit.tooltip);
                        if limit.reopen {
                            icons::hint(ui, Icon::Refresh, 13.0, limit.tooltip);
                        }
                        label
                    },
                )
                .inner;
            let width =
                (ui.available_width() - columns.value - ui.spacing().item_spacing.x - reset_width)
                    .max(40.0);
            let (low, high) = (*limit.range.start(), *limit.range.end());
            let response = themed_slider(
                ui,
                width,
                egui::Slider::new(value, limit.range)
                    .logarithmic(true)
                    .smallest_positive(limit.smallest)
                    .show_value(false)
                    .trailing_fill(true),
            );
            if response.changed() {
                *value = round_limit(*value).clamp(low, high);
            }
            ui.allocate_ui_with_layout(
                egui::vec2(columns.value, height),
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| ui.label((limit.format)(*value)),
            );
            if let Some((enabled, name)) = reset {
                reset_clicked = ui
                    .add_enabled_ui(enabled, |ui| {
                        icons::line_control(ui, Icon::Undo, name, "", None)
                    })
                    .inner
                    .clicked();
            }
            response.labelled_by(label.id).on_hover_text(limit.tooltip)
        })
        .inner;
    (response, reset_clicked)
}

/// Two significant figures, so a logarithmic drag lands on readable values.
fn round_limit(value: u64) -> u64 {
    let mut scale = 1;
    while value / scale >= 100 {
        scale *= 10;
    }
    (value + scale / 2) / scale * scale
}

fn format_ms(ms: u64) -> String {
    let trim = |value: f64| {
        let text = format!("{value:.1}");
        text.strip_suffix(".0").map(str::to_owned).unwrap_or(text)
    };
    match ms {
        0 => "Off".to_owned(),
        1..1000 => format!("{ms} ms"),
        1000..60_000 => format!("{} s", trim(ms as f64 / 1000.0)),
        _ => format!("{} min", trim(ms as f64 / 60_000.0)),
    }
}

/// Every button, text field, slider and spin button lies inside `width`.
#[cfg(test)]
pub(super) fn assert_controls_fit(output: &egui::FullOutput, width: f32, case: &str) {
    for (_, node) in super::test_ui::nodes(output) {
        if matches!(
            node.role(),
            egui::Role::Button
                | egui::Role::TextInput
                | egui::Role::Slider
                | egui::Role::SpinButton
        ) && let Some(bounds) = node.bounds()
        {
            assert!(
                bounds.x0 >= 0.0 && bounds.x1 <= f64::from(width) + 1.0,
                "{case}, {:?}: {bounds:?}",
                node.label()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Mode, Page, assert_controls_fit, egui, format_ms, round_limit};
    use crate::app::test_ui::{self, control, nodes, pointer, render};

    const PAGES: [Page; 4] = [
        Page::Appearance,
        Page::Discovery,
        Page::Resources,
        Page::Queries,
    ];
    fn settings(
        ctx: &egui::Context,
        app: &mut crate::app::App,
        size: egui::Vec2,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        render(ctx, size, events, |ui| app.settings_ui(ui))
    }

    #[test]
    fn settings_pages_fit_narrow_and_wide_windows_with_scaled_text() {
        for dark in [false, true] {
            for font in [14, 20] {
                let (ctx, mut app) = test_ui::app();
                app.preferences.appearance = Some(Mode::from_dark(dark));
                app.preferences.font_size = font;
                app.apply_theme(&ctx);
                app.preferences.query_timeout_ms = 3_600_000;
                app.preferences.null_display = "n".repeat(32);
                for width in [280.0, 440.0, 640.0, 900.0] {
                    for page in PAGES {
                        app.settings.page = page;
                        for _ in 0..2 {
                            let output = settings(&ctx, &mut app, egui::vec2(width, 600.0), vec![]);
                            assert_controls_fit(
                                &output,
                                width,
                                &format!("{page:?}, {width}, {font}"),
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn query_limits_are_sliders_with_readable_values() {
        let (ctx, mut app) = test_ui::app();
        app.settings.page = Page::Queries;
        let output = settings(&ctx, &mut app, egui::vec2(440.0, 600.0), vec![]);
        let sliders = nodes(&output)
            .iter()
            .filter(|(_, node)| node.role() == egui::Role::Slider)
            .count();
        assert_eq!(sliders, 4);
        assert_eq!(round_limit(1037), 1000);
        assert_eq!(round_limit(9_999), 10_000);
        assert_eq!(round_limit(47), 47);
        assert_eq!(format_ms(0), "Off");
        assert_eq!(format_ms(250), "250 ms");
        assert_eq!(format_ms(1500), "1.5 s");
        assert_eq!(format_ms(30_000), "30 s");
        assert_eq!(format_ms(3_600_000), "60 min");
    }

    #[test]
    fn editing_a_preference_saves_when_switching_pages() {
        let (ctx, mut app) = test_ui::app();
        let size = egui::vec2(640.0, 540.0);
        let output = settings(&ctx, &mut app, size, vec![]);
        let bounds = nodes(&output)
            .iter()
            .find_map(|(_, node)| {
                (node.role() == egui::Role::TextInput)
                    .then(|| node.bounds())
                    .flatten()
            })
            .unwrap();
        let pos = egui::pos2(
            (bounds.x0 + bounds.x1) as f32 / 2.0,
            (bounds.y0 + bounds.y1) as f32 / 2.0,
        );
        let click = |app: &mut crate::app::App, pos| {
            for pressed in [true, false] {
                settings(&ctx, app, size, pointer(pos, pressed));
            }
        };
        click(&mut app, pos);
        settings(
            &ctx,
            &mut app,
            size,
            vec![egui::Event::Text("marker".into())],
        );
        assert!(app.preferences.null_display.contains("marker"));
        assert!(app.settings.dirty);
        let output = settings(&ctx, &mut app, size, vec![]);
        click(
            &mut app,
            control(&output, "Queries and SQL recovery").center(),
        );
        assert_eq!(app.settings.page, Page::Queries);
        assert!(!app.settings.dirty);
    }
    #[test]
    fn settings_window_keeps_its_title_and_scales_when_font_changes() {
        let (ctx, mut app) = test_ui::app();
        let size = egui::vec2(1280.0, 720.0);
        let window = || {
            ctx.memory(|m| m.area_rect(egui::Id::unique("application-settings")))
                .unwrap()
        };
        app.settings.open = true;
        for _ in 0..3 {
            render(&ctx, size, vec![], |ui| app.render(ui));
        }
        for page in PAGES.into_iter().chain([Page::Discovery, Page::Queries]) {
            app.settings.page = page;
            for _ in 0..3 {
                let output = render(&ctx, size, vec![], |ui| app.render(ui));
                let rect = window();
                assert!(output.shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.job.text == "Settings" && rect.contains(t.pos))), "missing window title on {page:?}");
            }
        }
        let before = window();
        app.preferences.font_size = 20;
        app.apply_theme(&ctx);
        for _ in 0..3 {
            render(&ctx, size, vec![], |ui| app.render(ui));
        }
        let after = window();
        assert!(after.width() > before.width());
        assert!(after.width() <= size.x && after.height() <= size.y);
    }
}
