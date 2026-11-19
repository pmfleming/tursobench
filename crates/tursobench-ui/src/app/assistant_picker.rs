//! The chat's model picker: one popup chooses the agent, its model and how
//! hard it reasons. A rail of agents and favourites on the left, the models
//! with search in the middle, and the reasoning levels on the right.
use super::{
    App,
    icons::{self, Icon},
    presentation::{self, Intent},
};
use crate::storage::{FavouriteModel, Provider};
use eframe::egui;
use tursobench_assistant::{
    account::{self, Effort, Model},
    vendor::VENDORS,
};

/// Rows listed before "Show more".
const SHORT_LIST: usize = 4;
const RAIL: f32 = 46.0;
const LIST: f32 = 262.0;
const REASONING: f32 = 172.0;
const HEIGHT: f32 = 292.0;
const ROW: f32 = 30.0;
const NUMBER_KEYS: [egui::Key; 9] = [
    egui::Key::Num1,
    egui::Key::Num2,
    egui::Key::Num3,
    egui::Key::Num4,
    egui::Key::Num5,
    egui::Key::Num6,
    egui::Key::Num7,
    egui::Key::Num8,
    egui::Key::Num9,
];

/// What the rail shows in the list.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Shelf {
    Favourites,
    Agent(Provider),
}

/// The picker's own state while it is open.
#[derive(Default)]
pub(super) struct Menu {
    pub shelf: Option<Shelf>,
    pub query: String,
    pub more: bool,
    focus: bool,
}

/// One row of the model list.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Choice {
    pub provider: Provider,
    /// The model id; empty is the agent's own default.
    pub model: String,
    pub name: String,
    /// The agent uses this model when none is chosen.
    pub default: bool,
}

pub(super) fn menu_id() -> egui::Id {
    egui::Id::unique("chat-model-menu")
}

/// A small square standing for an agent: its initials on its own colour.
/// Not a logo; the agent's name is in every tooltip beside it.
pub(super) fn paint_badge(ui: &egui::Ui, rect: egui::Rect, provider: Provider, dim: bool) {
    let (initials, color) = match provider {
        Provider::Claude => ("Cl", egui::Color32::from_rgb(196, 104, 72)),
        Provider::Codex => ("Cx", egui::Color32::from_rgb(52, 132, 108)),
        Provider::Antigravity => ("Ag", egui::Color32::from_rgb(64, 114, 204)),
        Provider::Copilot => ("Co", egui::Color32::from_rgb(124, 88, 196)),
        Provider::Custom => ("", egui::Color32::from_gray(110)),
    };
    let color = if dim {
        color.gamma_multiply(0.45)
    } else {
        color
    };
    let ink = if dim {
        egui::Color32::from_white_alpha(150)
    } else {
        egui::Color32::WHITE
    };
    let painter = ui.painter();
    painter.rect_filled(rect, rect.height() * 0.28, color);
    if initials.is_empty() {
        icons::paint(painter, rect.shrink(rect.width() * 0.2), Icon::Command, ink);
    } else {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            initials,
            egui::FontId::proportional(rect.height() * 0.52),
            ink,
        );
    }
}

/// Text cut to `width` with an ellipsis, on one line.
fn truncated(
    ui: &egui::Ui,
    text: &str,
    size: f32,
    color: egui::Color32,
    width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(
        text.to_owned(),
        egui::FontId::proportional(size),
        color,
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(1.0));
    ui.painter().layout_job(job)
}

/// Remember `value` for an agent; empty forgets it, so the agent's own applies.
fn remember(
    choices: &mut std::collections::BTreeMap<Provider, String>,
    provider: Provider,
    value: String,
) {
    if value.is_empty() {
        choices.remove(&provider);
    } else {
        choices.insert(provider, value);
    }
}

/// A reasoning level as the picker names it.
fn effort_name(id: &str) -> String {
    match id {
        "xhigh" => "Extra high".into(),
        _ => {
            let mut chars = id.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(chars).collect()
            })
        }
    }
}

impl App {
    /// The reasoning levels an agent and its chosen model offer, and the level
    /// the agent uses when none is chosen, when it says.
    fn efforts_for(&self, provider: Provider) -> (Vec<Effort>, Option<String>) {
        let status = self
            .accounts
            .rows
            .get(&provider)
            .and_then(|r| r.status.as_ref());
        let named = |ids: Vec<String>| -> Vec<Effort> {
            ids.into_iter()
                .map(|id| Effort {
                    name: effort_name(&id),
                    id,
                })
                .collect()
        };
        match provider {
            Provider::Claude => (
                named(
                    account::claude_efforts()
                        .into_iter()
                        .map(|e| e.id)
                        .collect(),
                ),
                None,
            ),
            // Codex lists levels per model; the default model stands in for none.
            Provider::Codex => {
                let selected = self.model_for(provider);
                status
                    .and_then(|s| {
                        s.models.iter().find(|m| {
                            if selected.is_empty() {
                                m.default
                            } else {
                                m.id == selected
                            }
                        })
                    })
                    .map(|m| (named(m.efforts.clone()), m.default_effort.clone()))
                    .unwrap_or_default()
            }
            _ => (status.map(|s| s.efforts.clone()).unwrap_or_default(), None),
        }
    }

    /// The chosen reasoning level, when the chat's agent offers it.
    pub(super) fn assistant_effort(&self) -> Option<String> {
        let chosen = self.preferences.assistant.selected_effort();
        let (efforts, _) = self.efforts_for(self.preferences.assistant.provider);
        (!chosen.is_empty() && efforts.iter().any(|e| e.id == chosen)).then(|| chosen.to_owned())
    }

    /// The level remembered for an agent; empty is its own.
    fn effort_for(&self, provider: Provider) -> &str {
        self.preferences
            .assistant
            .efforts
            .get(&provider)
            .map_or("", String::as_str)
    }

    /// Remember a reasoning level for an agent; empty restores its own. When it
    /// is the agent in use, running conversations take it from their next message.
    pub(super) fn select_effort_for(&mut self, provider: Provider, effort: String) {
        if effort == self.effort_for(provider) {
            return;
        }
        remember(&mut self.preferences.assistant.efforts, provider, effort);
        self.preferences.normalize();
        self.store.save(&self.preferences);
        if provider != self.preferences.assistant.provider {
            return;
        }
        let effort = self.assistant_effort();
        for database in std::iter::once(&self.database).chain(self.parked.values()) {
            database.assistant.set_effort(effort.clone());
        }
    }

    /// Chat with another agent: every conversation starts afresh.
    pub(super) fn select_assistant_provider(&mut self, provider: Provider) {
        if self.preferences.assistant.provider == provider {
            return;
        }
        self.preferences.assistant.provider = provider;
        self.reset_assistant_conversations();
        self.store.save(&self.preferences);
    }

    /// The models an agent offers: what its check listed, or Claude Code's aliases.
    pub(super) fn models_for(&self, provider: Provider) -> Vec<Model> {
        self.accounts
            .rows
            .get(&provider)
            .and_then(|r| r.status.as_ref())
            .map(|s| s.models.clone())
            .filter(|models| !models.is_empty() || provider != Provider::Claude)
            .unwrap_or_else(|| {
                if provider == Provider::Claude {
                    account::claude_models()
                } else {
                    vec![]
                }
            })
    }

    /// The model chosen for an agent; empty is its own default.
    pub(super) fn model_for(&self, provider: Provider) -> &str {
        self.preferences
            .assistant
            .models
            .get(&provider)
            .map_or("", String::as_str)
    }

    /// The name of the model an agent would use, unless it is its own default.
    fn current_model_name(&self, provider: Provider) -> Option<String> {
        self.agent_choices(provider)
            .into_iter()
            .find(|c| self.is_current(c))
            .filter(|c| !c.model.is_empty())
            .map(|c| c.name)
    }

    /// Whether a row is the model its agent would use.
    pub(super) fn is_current(&self, choice: &Choice) -> bool {
        let chosen = self.model_for(choice.provider);
        choice.model == chosen || chosen.is_empty() && choice.default
    }

    /// Every model row for one agent: its default, its models, and a model
    /// typed in Settings that the agent did not list.
    pub(super) fn agent_choices(&self, provider: Provider) -> Vec<Choice> {
        let models = self.models_for(provider);
        let mut rows = vec![];
        if !models.iter().any(|m| m.default) {
            rows.push(Choice {
                provider,
                model: String::new(),
                name: "Agent default".into(),
                default: true,
            });
        }
        rows.extend(models.iter().map(|m| Choice {
            provider,
            model: m.id.clone(),
            name: m.name.clone(),
            default: m.default,
        }));
        let chosen = self.model_for(provider);
        if !chosen.is_empty() && !rows.iter().any(|r| r.model == chosen) {
            rows.push(Choice {
                provider,
                model: chosen.into(),
                name: chosen.into(),
                default: false,
            });
        }
        rows
    }

    fn favourite_choices(&self) -> Vec<Choice> {
        self.preferences
            .assistant
            .favourite_models
            .iter()
            .map(|f| {
                let mut choice = self
                    .agent_choices(f.provider)
                    .into_iter()
                    .find(|c| c.model == f.model)
                    .unwrap_or_else(|| Choice {
                        provider: f.provider,
                        model: f.model.clone(),
                        name: f.model.clone(),
                        default: false,
                    });
                if choice.model.is_empty() {
                    choice.name = f.provider.vendor().name.into();
                }
                choice
            })
            .collect()
    }

    fn is_favourite(&self, choice: &Choice) -> bool {
        self.preferences
            .assistant
            .favourite_models
            .iter()
            .any(|f| f.provider == choice.provider && f.model == choice.model)
    }

    pub(super) fn toggle_favourite_model(&mut self, choice: &Choice) {
        let favourites = &mut self.preferences.assistant.favourite_models;
        let before = favourites.len();
        favourites.retain(|f| !(f.provider == choice.provider && f.model == choice.model));
        if favourites.len() == before {
            favourites.push(FavouriteModel {
                provider: choice.provider,
                model: choice.model.clone(),
            });
        }
        self.preferences.normalize();
        self.store.save(&self.preferences);
    }

    fn shelf(&self) -> Shelf {
        self.accounts
            .menu
            .shelf
            .unwrap_or(Shelf::Agent(self.preferences.assistant.provider))
    }

    /// The rows the list shows now, and how many "Show more" hides.
    pub(super) fn menu_rows(&self) -> (Vec<Choice>, usize) {
        let rows = match self.shelf() {
            Shelf::Favourites => self.favourite_choices(),
            Shelf::Agent(provider) => self.agent_choices(provider),
        };
        let query = self.accounts.menu.query.trim().to_lowercase();
        if !query.is_empty() {
            let matching = rows
                .into_iter()
                .filter(|r| {
                    r.name.to_lowercase().contains(&query)
                        || r.model.to_lowercase().contains(&query)
                })
                .collect();
            return (matching, 0);
        }
        if self.accounts.menu.more || rows.len() <= SHORT_LIST + 1 {
            return (rows, 0);
        }
        let hidden = rows.len() - SHORT_LIST;
        (rows.into_iter().take(SHORT_LIST).collect(), hidden)
    }

    /// Use a model, switching agent when it belongs to another one. Changing
    /// either starts a new conversation.
    pub(super) fn choose_assistant_model(&mut self, provider: Provider, model: String) {
        if provider == self.preferences.assistant.provider && model == self.model_for(provider) {
            return;
        }
        remember(&mut self.preferences.assistant.models, provider, model);
        self.preferences.assistant.provider = provider;
        self.reset_assistant_conversations();
        self.preferences.normalize();
        self.store.save(&self.preferences);
    }

    fn choose_row(&mut self, choice: &Choice) {
        // A listed default is kept as "the agent's default", so it follows
        // the agent when its default changes.
        let model = if choice.default {
            String::new()
        } else {
            choice.model.clone()
        };
        self.choose_assistant_model(choice.provider, model);
    }

    /// Ctrl 1 to 9 choose a listed model while the picker is open.
    pub(super) fn model_menu_keys(&mut self, ctx: &egui::Context) -> bool {
        if !egui::Popup::is_id_open(ctx, menu_id()) {
            return false;
        }
        for (index, key) in NUMBER_KEYS.into_iter().enumerate() {
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, key)) {
                if let Some(row) = self.menu_rows().0.get(index) {
                    let row = row.clone();
                    self.choose_row(&row);
                }
                return true;
            }
        }
        false
    }

    /// The header's button: the agent's badge, model and reasoning level.
    pub(super) fn assistant_model_menu(&mut self, ui: &mut egui::Ui, max_width: f32) {
        let provider = self.preferences.assistant.provider;
        let vendor = provider.vendor().name;
        let model = self
            .current_model_name(provider)
            .unwrap_or_else(|| vendor.to_owned());
        let effort = self.assistant_effort().map(|e| effort_name(&e));
        let text = match &effort {
            Some(effort) => format!("{model} · {effort}"),
            None => model.clone(),
        };
        let size = 13.0;
        let chrome = 6.0 + 16.0 + 7.0 + 6.0 + 10.0 + 7.0;
        let full = truncated(ui, &text, size, egui::Color32::WHITE, f32::INFINITY)
            .size()
            .x
            + chrome;
        let width = full.min(max_width).max(72.0);
        let height = ui.spacing().interact_size.y.max(24.0);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
        let response = ui.interact(rect, menu_id().with("button"), egui::Sense::click());
        let open = egui::Popup::is_id_open(ui.ctx(), menu_id());
        if response.clicked() && !open {
            self.accounts.menu = Menu {
                shelf: Some(Shelf::Agent(provider)),
                focus: true,
                ..Menu::default()
            };
        }
        let visuals = ui.style().interact_selectable(&response, open);
        ui.painter().rect(
            rect,
            visuals.corner_radius,
            visuals.weak_bg_fill,
            visuals.bg_stroke,
            egui::StrokeKind::Inside,
        );
        let badge = badge_at(rect.left(), rect.center().y);
        paint_badge(ui, badge, provider, false);
        let galley = truncated(ui, &text, size, visuals.text_color(), width - chrome);
        ui.painter().galley(
            egui::pos2(badge.right() + 7.0, rect.center().y - galley.size().y / 2.0),
            galley,
            visuals.text_color(),
        );
        icons::paint(
            ui.painter(),
            egui::Rect::from_center_size(
                egui::pos2(rect.right() - 12.0, rect.center().y),
                egui::Vec2::splat(11.0),
            ),
            Icon::Down,
            visuals.text_color(),
        );
        let level = effort.clone().unwrap_or_else(|| "Default".into());
        response.widget_info(|| {
            let mut info =
                egui::WidgetInfo::labeled(egui::Role::ComboBox, true, "Assistant model picker");
            info.current_text_value = Some(text.clone());
            info
        });
        let response = response.on_hover_text(format!(
            "{vendor} · {model} · reasoning {level}\nChoose the agent, model and reasoning level"
        ));
        egui::Popup::from_toggle_button_response(&response)
            .id(menu_id())
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .align(egui::RectAlign::BOTTOM_START)
            .gap(4.0)
            .frame(egui::Frame::popup(ui.style()).inner_margin(0.0))
            .show(|ui| self.model_menu_ui(ui));
    }

    fn model_menu_ui(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
        ui.horizontal_top(|ui| {
            let column = |ui: &mut egui::Ui, width: f32, add: &mut dyn FnMut(&mut egui::Ui)| {
                ui.allocate_ui_with_layout(
                    egui::vec2(width, HEIGHT),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_min_size(egui::vec2(width, HEIGHT));
                        ui.set_max_width(width);
                        add(ui);
                    },
                );
            };
            let divider = |ui: &mut egui::Ui| {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(1.0, HEIGHT), egui::Sense::hover());
                let tokens = crate::appearance::Tokens::of(ui.ctx());
                ui.painter().rect_filled(rect, 0.0, tokens.line_soft);
            };
            column(ui, RAIL, &mut |ui| self.menu_rail(ui));
            divider(ui);
            column(ui, LIST, &mut |ui| self.menu_models(ui));
            divider(ui);
            column(ui, REASONING, &mut |ui| self.menu_reasoning(ui));
        });
    }

    fn menu_rail(&mut self, ui: &mut egui::Ui) {
        let shelf = self.shelf();
        ui.add_space(6.0);
        let favourites = rail_item(ui, shelf == Shelf::Favourites, "Favourites", |ui, rect| {
            icons::paint(
                ui.painter(),
                rect.shrink(1.0),
                Icon::Star,
                ui.visuals().text_color(),
            );
        });
        if favourites.on_hover_text("Favourite models").clicked() {
            self.accounts.menu.shelf = Some(Shelf::Favourites);
            self.accounts.menu.more = false;
        }
        let tokens = crate::appearance::Tokens::of(ui.ctx());
        let (line, _) = ui.allocate_exact_size(egui::vec2(RAIL, 9.0), egui::Sense::hover());
        ui.painter().hline(
            line.x_range().shrink(10.0),
            line.center().y,
            egui::Stroke::new(1.0, tokens.line_soft),
        );
        for vendor in VENDORS {
            let (state, intent, installed) = self.agent_state(vendor.id);
            let name = format!("{} · {state}", vendor.name);
            let item = rail_item(ui, shelf == Shelf::Agent(vendor.id), &name, |ui, rect| {
                paint_badge(ui, rect, vendor.id, !installed);
                if intent != Intent::Neutral {
                    let dot = rect.right_bottom() - egui::vec2(1.0, 1.0);
                    ui.painter()
                        .circle_filled(dot, 4.0, ui.visuals().window_fill);
                    ui.painter()
                        .circle_filled(dot, 2.6, presentation::intent_color(ui, intent));
                }
            });
            if item.on_hover_text(&name).clicked() {
                self.accounts.menu.shelf = Some(Shelf::Agent(vendor.id));
                self.accounts.menu.more = false;
            }
        }
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            ui.add_space(6.0);
            let manage = rail_item(ui, false, "Manage agents", |ui, rect| {
                icons::paint(ui.painter(), rect.shrink(1.0), Icon::Settings, tokens.muted);
            });
            if manage
                .on_hover_text("Manage agents, sign-in and custom models in Settings")
                .clicked()
            {
                self.open_assistant_settings();
                if let Shelf::Agent(provider) = shelf {
                    self.accounts.configuring = Some(provider);
                }
                egui::Popup::close_id(ui.ctx(), menu_id());
            }
        });
    }

    fn menu_models(&mut self, ui: &mut egui::Ui) {
        let shelf = self.shelf();
        let checking = match shelf {
            Shelf::Agent(provider) => self
                .accounts
                .rows
                .get(&provider)
                .is_some_and(|r| r.job.is_some()),
            Shelf::Favourites => false,
        };
        self.models_search(ui, shelf, checking);
        ui.add_space(4.0);
        let (rows, hidden) = self.menu_rows();
        let active = self.preferences.assistant.provider;
        // In favourites only the agent in use has a current model.
        let marks: Vec<_> = rows
            .iter()
            .map(|row| {
                let checked =
                    self.is_current(row) && (shelf != Shelf::Favourites || row.provider == active);
                (checked, self.is_favourite(row))
            })
            .collect();
        let note = if !self.accounts.menu.query.trim().is_empty() {
            "No models match"
        } else if shelf == Shelf::Favourites {
            "Star a model to keep it here"
        } else if checking {
            "Finding models…"
        } else {
            "No models listed yet"
        };
        let (clicked, more) = egui::ScrollArea::vertical()
            .id_salt("chat-model-list")
            .auto_shrink([false, false])
            .max_height(HEIGHT - 44.0)
            .show(ui, |ui| model_list(ui, &rows, &marks, hidden, note))
            .inner;
        self.accounts.menu.more |= more;
        match clicked {
            Some((RowClick::Star, row)) => self.toggle_favourite_model(row),
            Some((RowClick::Row, row)) => self.choose_row(row),
            _ => {}
        }
    }

    /// Search, and a refresh for agents that list their models.
    fn models_search(&mut self, ui: &mut egui::Ui, shelf: Shelf, checking: bool) {
        let muted = crate::appearance::Tokens::of(ui.ctx()).muted;
        let bar = bar(ui, LIST);
        icons::paint(
            ui.painter(),
            egui::Rect::from_center_size(
                egui::pos2(bar.left() + 18.0, bar.center().y),
                egui::Vec2::splat(15.0),
            ),
            Icon::Search,
            muted,
        );
        let field = egui::Rect::from_min_max(
            egui::pos2(bar.left() + 32.0, bar.top() + 7.0),
            egui::pos2(bar.right() - 36.0, bar.bottom() - 7.0),
        );
        let search = ui.put(
            field,
            egui::TextEdit::singleline(&mut self.accounts.menu.query)
                .id(menu_id().with("search"))
                .hint_text("Search models…")
                .frame(egui::Frame::NONE)
                .desired_width(field.width()),
        );
        search.widget_info(|| {
            egui::WidgetInfo::text_edit(true, "", &self.accounts.menu.query, "Search models")
        });
        if std::mem::take(&mut self.accounts.menu.focus) {
            search.request_focus();
        }
        let Shelf::Agent(provider) = shelf else {
            return;
        };
        let refresh = egui::Rect::from_center_size(
            egui::pos2(bar.right() - 20.0, bar.center().y),
            egui::Vec2::splat(26.0),
        );
        if checking {
            ui.put(refresh.shrink(5.0), egui::Spinner::new())
                .on_hover_text("Finding models…");
        } else if provider != Provider::Claude
            && ui
                .scope_builder(egui::UiBuilder::new().max_rect(refresh), |ui| {
                    icons::control(ui, Icon::Refresh, "Find available models", "", false, false)
                })
                .inner
                .clicked()
        {
            self.agent_account_job(ui.ctx(), provider, account::Operation::Check);
        }
    }

    fn menu_reasoning(&mut self, ui: &mut egui::Ui) {
        let provider = match self.shelf() {
            Shelf::Agent(provider) => provider,
            Shelf::Favourites => self.preferences.assistant.provider,
        };
        let (efforts, default) = self.efforts_for(provider);
        // A remembered level this model lacks acts as the default.
        let chosen = Some(self.effort_for(provider))
            .filter(|chosen| efforts.iter().any(|e| e.id == *chosen))
            .unwrap_or_default()
            .to_owned();
        let model = self
            .current_model_name(provider)
            .unwrap_or_else(|| "Default".to_owned());
        let current = match (chosen.as_str(), &default) {
            ("", None) => "Default".to_owned(),
            ("", Some(level)) => format!("Default · {}", effort_name(level)),
            (chosen, _) => effort_name(chosen),
        };
        reasoning_heading(ui, &model, &current);
        if efforts.is_empty() {
            let muted = crate::appearance::Tokens::of(ui.ctx()).muted;
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new("This agent sets its own level").color(muted),
                    )
                    .wrap(),
                );
            });
        } else if let Some(effort) = level_rows(ui, &efforts, default.as_deref(), &chosen) {
            self.select_effort_for(provider, effort);
        }
    }
}

/// The top bar of a picker column, with its line below; the rows follow it.
fn bar(ui: &mut egui::Ui, width: f32) -> egui::Rect {
    let (bar, _) = ui.allocate_exact_size(egui::vec2(width, 38.0), egui::Sense::hover());
    paint_bar_line(ui, bar);
    bar
}

fn paint_bar_line(ui: &egui::Ui, bar: egui::Rect) {
    let line = crate::appearance::Tokens::of(ui.ctx()).line_soft;
    ui.painter()
        .hline(bar.x_range(), bar.bottom(), egui::Stroke::new(1.0, line));
}

/// "Reasoning", the model it applies to, and the current level for assistive tech.
fn reasoning_heading(ui: &mut egui::Ui, model: &str, current: &str) {
    let muted = crate::appearance::Tokens::of(ui.ctx()).muted;
    let (bar, heading) = ui.allocate_exact_size(egui::vec2(REASONING, 38.0), egui::Sense::hover());
    ui.painter().text(
        egui::pos2(bar.left() + 12.0, bar.center().y),
        egui::Align2::LEFT_CENTER,
        "Reasoning",
        egui::FontId::proportional(13.0),
        muted,
    );
    let label = truncated(ui, model, 12.0, muted, REASONING - 96.0);
    ui.painter().galley(
        egui::pos2(
            bar.right() - 12.0 - label.size().x,
            bar.center().y - label.size().y / 2.0,
        ),
        label,
        muted,
    );
    heading.widget_info(|| {
        let mut info =
            egui::WidgetInfo::labeled(egui::Role::RadioGroup, true, "Assistant reasoning level");
        info.current_text_value = Some(current.to_owned());
        info
    });
    heading.on_hover_text(
        "How much the agent thinks before it answers. A new level applies from \
         the next message; the conversation carries on.",
    );
    paint_bar_line(ui, bar);
    ui.add_space(4.0);
}

/// Every level row; returns the level picked, empty for the agent's own.
fn level_rows(
    ui: &mut egui::Ui,
    efforts: &[Effort],
    default: Option<&str>,
    chosen: &str,
) -> Option<String> {
    let mut picked = None;
    // Without a known default, the agent's own level is a row of its own.
    if default.is_none() && level_row(ui, "Default", "", false, chosen.is_empty()) {
        picked = Some(String::new());
    }
    for effort in efforts {
        let is_default = default == Some(effort.id.as_str());
        let checked = chosen == effort.id || chosen.is_empty() && is_default;
        if level_row(
            ui,
            &effort_name(&effort.id),
            &effort.id,
            is_default,
            checked,
        ) {
            // The default level is kept as "the agent's own".
            picked = Some(if is_default {
                String::new()
            } else {
                effort.id.clone()
            });
        }
    }
    picked
}

/// One rail entry: a 20-point glyph, a hover fill and an accent bar when shown.
fn rail_item(
    ui: &mut egui::Ui,
    selected: bool,
    name: &str,
    paint: impl FnOnce(&egui::Ui, egui::Rect),
) -> egui::Response {
    let tokens = crate::appearance::Tokens::of(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(egui::vec2(RAIL, 38.0), egui::Sense::click());
    if response.hovered() {
        ui.painter()
            .rect_filled(rect.shrink2(egui::vec2(5.0, 2.0)), 6.0, tokens.raised_hover);
    }
    paint(
        ui,
        egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(20.0)),
    );
    if selected {
        let bar = egui::Rect::from_center_size(
            egui::pos2(rect.right() - 2.0, rect.center().y),
            egui::vec2(2.5, 20.0),
        );
        ui.painter().rect_filled(bar, 1.0, tokens.primary);
    }
    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, selected, name));
    response
}

/// A badge's place: 16 points square, `left` of a row centred on `y`.
fn badge_at(left: f32, y: f32) -> egui::Rect {
    egui::Rect::from_min_size(egui::pos2(left + 6.0, y - 8.0), egui::Vec2::splat(16.0))
}

/// A full-width list row: its rect, its response and its inset.
fn list_row(ui: &mut egui::Ui) -> (egui::Rect, egui::Response, egui::Rect) {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, ROW), egui::Sense::click());
    (rect, response, rect.shrink2(egui::vec2(6.0, 1.0)))
}

fn paint_hover(ui: &egui::Ui, inner: egui::Rect) {
    let fill = crate::appearance::Tokens::of(ui.ctx()).raised_hover;
    ui.painter().rect_filled(inner, 6.0, fill);
}

/// The tick beside a chosen row.
fn paint_check(ui: &egui::Ui, center: egui::Pos2) {
    icons::paint(
        ui.painter(),
        egui::Rect::from_center_size(center, egui::Vec2::splat(13.0)),
        Icon::Check,
        ui.visuals().text_color(),
    );
}

enum RowClick {
    None,
    Row,
    Star,
}

/// The model rows with their (checked, favourite) marks, "Show more" when
/// `hidden` rows wait, or `note` when there are none. Returns the click, a
/// star before a row, and whether to show more.
fn model_list<'a>(
    ui: &mut egui::Ui,
    rows: &'a [Choice],
    marks: &[(bool, bool)],
    hidden: usize,
    note: &str,
) -> (Option<(RowClick, &'a Choice)>, bool) {
    let mut clicked = None;
    for (index, (row, &(checked, favourite))) in rows.iter().zip(marks).enumerate() {
        match model_row(ui, row, index, checked, favourite) {
            RowClick::None => {}
            RowClick::Row if matches!(clicked, Some((RowClick::Star, _))) => {}
            click => clicked = Some((click, row)),
        }
    }
    let muted = crate::appearance::Tokens::of(ui.ctx()).muted;
    let more = hidden > 0
        && ui
            .add_sized(
                egui::vec2(LIST - 12.0, 26.0),
                egui::Button::new(egui::RichText::new("Show more").color(muted)).frame(false),
            )
            .on_hover_text(format!("{hidden} more"))
            .clicked();
    if rows.is_empty() {
        ui.add_space(14.0);
        ui.vertical_centered(|ui| ui.weak(note));
    }
    (clicked, more)
}

fn model_row(
    ui: &mut egui::Ui,
    row: &Choice,
    index: usize,
    checked: bool,
    favourite: bool,
) -> RowClick {
    let tokens = crate::appearance::Tokens::of(ui.ctx());
    let (rect, response, inner) = list_row(ui);
    let star_rect = egui::Rect::from_center_size(
        egui::pos2(rect.right() - 62.0, rect.center().y),
        egui::Vec2::splat(22.0),
    );
    let star = ui.interact(star_rect, response.id.with("star"), egui::Sense::click());
    let hovered = response.hovered() || star.hovered();
    if hovered {
        paint_hover(ui, inner);
    }
    let text = ui.visuals().text_color();
    let badge = badge_at(inner.left(), rect.center().y);
    paint_badge(ui, badge, row.provider, false);
    // Right to left: the shortcut, the star, then "Default".
    if index < NUMBER_KEYS.len() {
        paint_shortcut(ui, index, inner.right() - 6.0, rect.center().y);
    }
    let mut right = star_rect.left();
    let glyph = star_rect.shrink(4.0);
    if favourite {
        icons::paint_filled_star(ui.painter(), glyph, tokens.pending);
    } else if hovered {
        let color = if star.hovered() { text } else { tokens.muted };
        icons::paint(ui.painter(), glyph, Icon::Star, color);
    }
    if row.default && !row.model.is_empty() {
        let tag = truncated(ui, "Default", 11.0, tokens.muted, 60.0);
        right -= tag.size().x + 4.0;
        ui.painter().galley(
            egui::pos2(right, rect.center().y - tag.size().y / 2.0),
            tag,
            tokens.muted,
        );
    }
    let left = badge.right() + 8.0;
    let check = if checked { 20.0 } else { 0.0 };
    let name = truncated(ui, &row.name, 13.0, text, right - left - check - 6.0);
    let name_width = name.size().x;
    ui.painter().galley(
        egui::pos2(left, rect.center().y - name.size().y / 2.0),
        name,
        text,
    );
    if checked {
        paint_check(ui, egui::pos2(left + name_width + 12.0, rect.center().y));
    }
    model_row_click(row, response, star, checked, favourite)
}

/// The Ctrl and number that choose a row, in a chip ending at `right`.
fn paint_shortcut(ui: &egui::Ui, index: usize, right: f32, y: f32) {
    let tokens = crate::appearance::Tokens::of(ui.ctx());
    let chip = truncated(ui, &format!("Ctrl+{}", index + 1), 11.0, tokens.muted, 60.0);
    let chip_rect = egui::Rect::from_min_size(
        egui::pos2(right - chip.size().x - 10.0, y - 9.0),
        egui::vec2(chip.size().x + 10.0, 18.0),
    );
    ui.painter().rect_filled(chip_rect, 4.0, tokens.raised);
    ui.painter()
        .galley(chip_rect.center() - chip.size() / 2.0, chip, tokens.muted);
}

/// A model row's and its star's names, states and tooltips, and which was clicked.
fn model_row_click(
    row: &Choice,
    response: egui::Response,
    star: egui::Response,
    checked: bool,
    favourite: bool,
) -> RowClick {
    let vendor = row.provider.vendor().name;
    response
        .widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, checked, &row.name));
    star.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::Role::Button,
            true,
            favourite,
            format!("Favourite {}", row.name),
        )
    });
    let star = star.on_hover_text(if favourite {
        "Remove from favourites"
    } else {
        "Add to favourites"
    });
    let detail = if row.model.is_empty() {
        format!("{vendor} chooses its own model")
    } else {
        format!("{vendor} · {}", row.model)
    };
    let response = response.on_hover_text(format!(
        "{detail}\nChanging model starts a new conversation"
    ));
    if star.clicked() {
        RowClick::Star
    } else if response.clicked() {
        RowClick::Row
    } else {
        RowClick::None
    }
}

/// One reasoning level: its name, a "Default" tag and a tick when chosen.
fn level_row(ui: &mut egui::Ui, name: &str, id: &str, default: bool, checked: bool) -> bool {
    let (rect, response, inner) = list_row(ui);
    if response.hovered() {
        paint_hover(ui, inner);
    }
    ui.painter().text(
        egui::pos2(inner.left() + 8.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        name,
        egui::FontId::proportional(13.0),
        ui.visuals().text_color(),
    );
    let right = inner.right() - 8.0;
    if checked {
        paint_check(ui, egui::pos2(right - 7.0, rect.center().y));
    }
    if default {
        ui.painter().text(
            egui::pos2(right - 20.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            "Default",
            egui::FontId::proportional(11.0),
            crate::appearance::Tokens::of(ui.ctx()).muted,
        );
    }
    let label = format!("Reasoning {name}");
    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, checked, &label));
    let hover = if id.is_empty() || default {
        "The agent's own level".to_owned()
    } else {
        format!("Sent to the agent as {id}")
    };
    response.on_hover_text(hover).clicked()
}
