//! Borrowed schema presentation; database actions are applied after rendering.
use crate::app::{
    icons::{self, Icon},
    shell::StructureSection,
};
use eframe::egui;
use tursobench_core::{IndexInfo, Inspection, SchemaObject};

pub(super) enum Action {
    Load,
    Browse(String),
    Definition(String),
    Column(String),
}
pub(super) struct Structure<'a> {
    pub info: Option<&'a Inspection>,
    pub object: Option<&'a SchemaObject>,
    pub schema: &'a [SchemaObject],
    pub loading: bool,
    pub graph: Option<&'a tursobench_core::SchemaDiagram>,
    pub incomplete: bool,
    pub selected: Option<&'a str>,
}
impl Structure<'_> {
    fn owner(&self) -> Option<&str> {
        self.info
            .map(|i| i.table.name.as_str())
            .or_else(|| self.object.map(|o| o.table.as_str()))
    }
    fn definitions(&self, kind: &str) -> Vec<&SchemaObject> {
        self.schema
            .iter()
            .filter(|o| Some(o.table.as_str()) == self.owner() && o.kind == kind)
            .collect()
    }
    pub fn show(&self, ui: &mut egui::Ui, section: &mut StructureSection) -> Option<Action> {
        use StructureSection::*;
        if self.loading && self.info.is_none() {
            let response = ui
                .scope(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for i in 0..7 {
                        let (rect, _) = ui.allocate_exact_size(
                            egui::vec2(ui.available_width(), 22.0),
                            egui::Sense::hover(),
                        );
                        let color = crate::appearance::Tokens::of(ui.ctx()).raised;
                        for (x, width) in [
                            (4.0, 13.0),
                            (25.0, 60.0 + (i % 3) as f32 * 16.0),
                            (rect.width() - 52.0, 44.0),
                        ] {
                            ui.painter().rect_filled(
                                egui::Rect::from_min_size(
                                    rect.min + egui::vec2(x, 6.0),
                                    egui::vec2(width, 9.0),
                                ),
                                3.0,
                                color,
                            );
                        }
                    }
                })
                .response;
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::ProgressIndicator, true, "Loading structure")
            });
            return None;
        }
        let is_view = self.info.is_some_and(|i| !i.table.is_table)
            || self.object.is_some_and(|o| o.kind == "view");
        let definition_only = self
            .object
            .is_some_and(|o| !matches!(o.kind.as_str(), "table" | "view"));
        if definition_only || is_view && matches!(*section, Indexes | ForeignKeys) {
            *section = if definition_only { Ddl } else { Columns };
        }
        let wide = ui.available_width() >= 720.0;
        if !wide {
            egui::ScrollArea::horizontal()
                .id_salt("structure-sections")
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        let full = ui.available_width() >= 480.0;
                        for (candidate, icon, label, count) in [
                            (
                                Columns,
                                Icon::Columns,
                                "Columns",
                                self.info.map(|i| i.table.columns.len()),
                            ),
                            (
                                Indexes,
                                Icon::Complete,
                                "Indexes",
                                self.info.map(|i| i.indexes.len()),
                            ),
                            (
                                ForeignKeys,
                                Icon::Link,
                                "Foreign keys",
                                self.info.map(|info| {
                                    crate::app::diagram::ForeignMap {
                                        info,
                                        graph: self.graph,
                                        incomplete: self.incomplete,
                                    }
                                    .count()
                                }),
                            ),
                            (
                                Triggers,
                                Icon::Trigger,
                                "Triggers",
                                Some(self.definitions("trigger").len()),
                            ),
                            (Ddl, Icon::Sql, "DDL", None),
                        ] {
                            if definition_only && candidate != Ddl
                                || is_view && matches!(candidate, Indexes | ForeignKeys)
                            {
                                continue;
                            }
                            let selected = *section == candidate;
                            let name = if full || selected { label } else { "" };
                            let text = format!(
                                "{name}{}{}",
                                if !name.is_empty() && count.is_some() {
                                    " · "
                                } else {
                                    ""
                                },
                                count.map_or(String::new(), |n| n.to_string())
                            );
                            let size = ui
                                .painter()
                                .layout_no_wrap(
                                    text.clone(),
                                    crate::appearance::proportional(ui.ctx(), 11.5),
                                    ui.visuals().text_color(),
                                )
                                .size();
                            let (rect, response) = ui.allocate_exact_size(
                                egui::vec2(25.0 + size.x, 22.0),
                                egui::Sense::click(),
                            );
                            if selected || response.hovered() {
                                ui.painter().rect_filled(
                                    rect,
                                    4.0,
                                    if selected {
                                        ui.visuals().selection.bg_fill
                                    } else {
                                        ui.visuals().widgets.hovered.bg_fill
                                    },
                                );
                            }
                            let color = if selected {
                                ui.visuals().text_color()
                            } else {
                                crate::appearance::Tokens::of(ui.ctx()).muted
                            };
                            icons::paint(
                                ui.painter(),
                                egui::Rect::from_center_size(
                                    rect.left_center() + egui::vec2(10.0, 0.0),
                                    egui::vec2(13.0, 13.0),
                                ),
                                icon,
                                color,
                            );
                            text_at(
                                ui,
                                egui::Rect::from_min_max(
                                    rect.min + egui::vec2(21.0, 0.0),
                                    rect.max,
                                ),
                                &text,
                                color,
                                11.5,
                            );
                            response.widget_info(|| {
                                egui::WidgetInfo::selected(
                                    egui::Role::Tab,
                                    ui.is_enabled(),
                                    selected,
                                    count.map_or(label.into(), |n| format!("{label}, {n}")),
                                )
                            });
                            if response.on_hover_text(label).clicked() {
                                *section = candidate;
                            }
                        }
                    });
                });
        }
        let mut action = None;
        if wide && self.info.is_none() && !definition_only && ui.button("Load structure").clicked()
        {
            action = Some(Action::Load);
        }
        if wide && !definition_only {
            ui.columns(2, |panes| {
                egui::ScrollArea::vertical()
                    .id_salt("structure-left")
                    .auto_shrink([false, false])
                    .show(&mut panes[0], |ui| {
                        if let Some(info) = self.info {
                            action = self.columns(ui, info);
                        }
                        ui.strong("CREATE statement");
                        if let Some(next) = self.definition(ui) {
                            action = Some(next);
                        }
                    });
                egui::ScrollArea::vertical()
                    .id_salt("structure-right")
                    .auto_shrink([false, false])
                    .show(&mut panes[1], |ui| {
                        ui.strong("Foreign keys");
                        if let Some(info) = self.info
                            && let Some(target) = (crate::app::diagram::ForeignMap {
                                info,
                                graph: self.graph,
                                incomplete: self.incomplete,
                            })
                            .show(ui)
                        {
                            action = Some(Action::Browse(target));
                        }
                        ui.strong("Indexes");
                        if let Some(next) = self.indexes(ui) {
                            action = Some(next);
                        }
                        ui.strong("Triggers");
                        if let Some(next) = self.triggers(ui) {
                            action = Some(next);
                        }
                    });
            });
        } else {
            egui::ScrollArea::vertical()
                .id_salt(("structure-section", *section))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    if self.info.is_none()
                        && !definition_only
                        && *section != Ddl
                        && ui.button("Load structure").clicked()
                    {
                        action = Some(Action::Load);
                    }
                    match *section {
                        Columns => {
                            if let Some(info) = self.info {
                                action = self.columns(ui, info);
                            }
                        }
                        Indexes => action = self.indexes(ui),
                        ForeignKeys => {
                            if let Some(info) = self.info
                                && let Some(target) = (crate::app::diagram::ForeignMap {
                                    info,
                                    graph: self.graph,
                                    incomplete: self.incomplete,
                                })
                                .show(ui)
                            {
                                action = Some(Action::Browse(target));
                            }
                        }
                        Triggers => action = self.triggers(ui),
                        Ddl => action = self.definition(ui),
                    }
                });
        }
        action
    }
    fn definition(&self, ui: &mut egui::Ui) -> Option<Action> {
        if let Some(ddl) = self.object.and_then(|o| o.ddl.as_deref()) {
            sql_view(ui, ddl);
        } else {
            ui.weak("DDL not retained; load it on demand.");
            if let Some(object) = self.object
                && ui.button("Load complete DDL").clicked()
            {
                return Some(Action::Definition(object.name.clone()));
            }
        }
        None
    }
    fn columns(&self, ui: &mut egui::Ui, info: &Inspection) -> Option<Action> {
        let mut action = None;
        let tokens = crate::appearance::Tokens::of(ui.ctx());
        if info.table.engine_managed() {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                "Engine-managed storage · generated edits disabled",
            );
        }
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let composite = info
                .table
                .columns
                .iter()
                .filter(|c| c.primary_key > 0)
                .count()
                > 1;
            for c in &info.table.columns {
                let keys: Vec<_> = info
                    .foreign_keys
                    .iter()
                    .filter(|k| {
                        k.columns
                            .iter()
                            .any(|(from, _)| from.eq_ignore_ascii_case(&c.name))
                    })
                    .collect();
                let indexes: Vec<_> = info
                    .indexes
                    .iter()
                    .filter(|i| {
                        i.columns
                            .iter()
                            .flatten()
                            .any(|name| name.eq_ignore_ascii_case(&c.name))
                    })
                    .collect();
                let selected = self
                    .selected
                    .is_some_and(|name| name.eq_ignore_ascii_case(&c.name));
                let (rect, response) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), 22.0),
                    egui::Sense::click(),
                );
                if selected || response.hovered() {
                    ui.painter().rect_filled(
                        rect,
                        0.0,
                        if selected {
                            ui.visuals().selection.bg_fill
                        } else {
                            ui.visuals().widgets.hovered.bg_fill
                        },
                    );
                }
                let icon_rect = egui::Rect::from_center_size(
                    rect.left_center() + egui::vec2(10.0, 0.0),
                    egui::vec2(13.0, 13.0),
                );
                let related = c.primary_key > 0 || !keys.is_empty();
                icons::paint(
                    ui.painter(),
                    icon_rect,
                    if c.primary_key > 0 {
                        Icon::Key
                    } else if !keys.is_empty() {
                        Icon::Link
                    } else {
                        Icon::Columns
                    },
                    if related {
                        tokens.relationship
                    } else {
                        tokens.muted
                    },
                );
                if c.not_null {
                    ui.painter()
                        .circle_filled(icon_rect.right_top(), 3.0, ui.visuals().panel_fill);
                    ui.painter().circle_filled(
                        icon_rect.right_top(),
                        2.0,
                        ui.visuals().text_color(),
                    );
                }
                if composite && c.primary_key > 0 {
                    ui.painter().text(
                        icon_rect.right_bottom(),
                        egui::Align2::CENTER_CENTER,
                        c.primary_key,
                        crate::appearance::proportional(ui.ctx(), 8.5),
                        tokens.relationship,
                    );
                }
                let mut right = rect.right() - 4.0;
                let type_width = (rect.width() * 0.22).clamp(45.0, 110.0);
                let type_rect = egui::Rect::from_min_max(
                    egui::pos2(right - type_width, rect.top()),
                    egui::pos2(right, rect.bottom()),
                );
                text_at(ui, type_rect, &c.declared_type, tokens.faint, 11.0);
                right = type_rect.left() - 5.0;
                if !indexes.is_empty() {
                    icons::paint(
                        ui.painter(),
                        egui::Rect::from_center_size(
                            egui::pos2(right - 6.0, rect.center().y),
                            egui::vec2(12.0, 12.0),
                        ),
                        Icon::Complete,
                        tokens.muted,
                    );
                    right -= 17.0;
                    if indexes.iter().any(|i| i.unique) {
                        text_at(
                            ui,
                            egui::Rect::from_min_max(
                                egui::pos2(right - 19.0, rect.top()),
                                egui::pos2(right, rect.bottom()),
                            ),
                            "UQ",
                            tokens.muted,
                            10.0,
                        );
                        right -= 23.0;
                    }
                }
                let name_right = if let Some(key) = keys.first() {
                    let width = (rect.width() * 0.28).clamp(45.0, 160.0);
                    let link = egui::Rect::from_min_max(
                        egui::pos2(right - width, rect.top()),
                        egui::pos2(right, rect.bottom()),
                    );
                    let target = key
                        .columns
                        .iter()
                        .find(|(from, _)| from.eq_ignore_ascii_case(&c.name))
                        .and_then(|(_, to)| to.as_deref())
                        .unwrap_or("?");
                    text_at(
                        ui,
                        link,
                        &format!("→ {}.{target}", key.target),
                        tokens.relationship,
                        11.0,
                    );
                    let go = ui.interact(
                        link,
                        ui.scope_id().with(("column-target", &c.name)),
                        egui::Sense::click(),
                    );
                    go.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::Role::Link,
                            ui.is_enabled(),
                            format!("Open {} for {}", key.target, c.name),
                        )
                    });
                    if go
                        .on_hover_text(format!("Open {} · {target}", key.target))
                        .clicked()
                    {
                        action = Some(Action::Browse(key.target.clone()));
                    }
                    link.left() - 5.0
                } else {
                    right
                };
                text_at(
                    ui,
                    egui::Rect::from_min_max(
                        rect.min + egui::vec2(23.0, 0.0),
                        egui::pos2(name_right.max(rect.left() + 24.0), rect.bottom()),
                    ),
                    &c.name,
                    if related {
                        tokens.relationship
                    } else {
                        ui.visuals().text_color()
                    },
                    12.5,
                );
                let description = format!(
                    "{} · {}{}{}{}{} · {}",
                    c.name,
                    c.declared_type,
                    if c.not_null { " · NOT NULL" } else { "" },
                    if c.primary_key > 0 {
                        if composite {
                            format!(" · Primary key, part {}", c.primary_key)
                        } else {
                            " · Primary key".into()
                        }
                    } else {
                        String::new()
                    },
                    if c.hidden > 0 {
                        format!(" · hidden {}", c.hidden)
                    } else {
                        String::new()
                    },
                    c.default_sql
                        .as_ref()
                        .map_or(String::new(), |s| format!(" · DEFAULT {s}")),
                    indexes
                        .iter()
                        .map(|i| i.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                response.widget_info(|| {
                    egui::WidgetInfo::selected(
                        egui::Role::Button,
                        ui.is_enabled(),
                        selected,
                        &description,
                    )
                });
                if response.on_hover_text(description).clicked() {
                    action = Some(Action::Column(c.name.clone()));
                }
            }
        });
        action
    }
    fn indexes(&self, ui: &mut egui::Ui) -> Option<Action> {
        let info = self.info?;
        let mut action = None;
        if info.indexes.is_empty() {
            ui.weak("No declared indexes returned");
        }
        for index in &info.indexes {
            let object = self
                .schema
                .iter()
                .find(|o| o.kind == "index" && o.name == index.name);
            if let Some(next) = definition_row(ui, &index.name, Some(index), object) {
                action = Some(next);
            }
        }
        action
    }
    fn triggers(&self, ui: &mut egui::Ui) -> Option<Action> {
        let definitions = self.definitions("trigger");
        if definitions.is_empty() {
            ui.weak("No declared triggers");
        }
        let mut action = None;
        for object in definitions {
            if let Some(next) = definition_row(ui, &object.name, None, Some(object)) {
                action = Some(next);
            }
        }
        action
    }
}
fn definition_row(
    ui: &mut egui::Ui,
    name: &str,
    index: Option<&IndexInfo>,
    object: Option<&SchemaObject>,
) -> Option<Action> {
    let automatic = index.is_some()
        && object.is_none_or(|o| o.ddl.is_none())
        && name.starts_with("sqlite_autoindex_");
    let summary = index.map_or(String::new(), |i| {
        i.columns
            .iter()
            .map(|c| c.as_deref().unwrap_or("expression"))
            .collect::<Vec<_>>()
            .join(" › ")
    });
    let id = ui.scope_id().with(("definition", name));
    let mut open = ui
        .ctx()
        .data_mut(|d| d.get_temp::<bool>(id).unwrap_or(false));
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 22.0), egui::Sense::click());
    let tokens = crate::appearance::Tokens::of(ui.ctx());
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, 0.0, ui.visuals().widgets.hovered.bg_fill);
    }
    let description = format!(
        "{name} · {summary}{}{}{}",
        if index.is_some_and(|i| i.unique) {
            " · UNIQUE"
        } else {
            ""
        },
        if index.is_some_and(|i| i.partial) {
            " · partial index"
        } else {
            ""
        },
        if automatic {
            " · Created automatically for a key"
        } else {
            ""
        }
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::Button, ui.is_enabled(), open, &description)
    });
    if response.on_hover_text(&description).clicked() {
        open = !open;
        ui.ctx().data_mut(|d| d.insert_temp(id, open));
    }
    if !automatic {
        icons::paint(
            ui.painter(),
            egui::Rect::from_center_size(
                rect.left_center() + egui::vec2(9.0, 0.0),
                egui::vec2(12.0, 12.0),
            ),
            if open { Icon::Down } else { Icon::Next },
            tokens.muted,
        );
    }
    let mut right = rect.right() - 5.0;
    for (show, icon) in [
        (automatic, Icon::System),
        (index.is_some_and(|i| i.partial), Icon::Filter),
    ] {
        if show {
            icons::paint(
                ui.painter(),
                egui::Rect::from_center_size(
                    egui::pos2(right - 6.0, rect.center().y),
                    egui::vec2(12.0, 12.0),
                ),
                icon,
                tokens.muted,
            );
            right -= 17.0;
        }
    }
    if index.is_some_and(|i| i.unique) {
        text_at(
            ui,
            egui::Rect::from_min_max(
                egui::pos2(right - 20.0, rect.top()),
                egui::pos2(right, rect.bottom()),
            ),
            "UQ",
            tokens.muted,
            11.0,
        );
        right -= 25.0;
    }
    let split = if summary.is_empty() {
        right
    } else {
        (rect.left() + rect.width() * 0.53).min(right)
    };
    text_at(
        ui,
        egui::Rect::from_min_max(
            rect.min + egui::vec2(21.0, 0.0),
            egui::pos2(split, rect.bottom()),
        ),
        name,
        if automatic {
            tokens.muted
        } else {
            ui.visuals().text_color()
        },
        12.0,
    );
    if !summary.is_empty() {
        text_at(
            ui,
            egui::Rect::from_min_max(
                egui::pos2(split + 4.0, rect.top()),
                egui::pos2(right.max(split + 4.0), rect.bottom()),
            ),
            &summary,
            tokens.faint,
            11.0,
        );
    }
    if open {
        if let Some(ddl) = object.and_then(|o| o.ddl.as_deref()) {
            sql_view(ui, ddl);
        } else if automatic {
            ui.weak("Created automatically for a key. No CREATE statement.");
        } else {
            ui.weak("DDL not retained; load it on demand.");
            if let Some(object) = object
                && ui.button("Load complete DDL").clicked()
            {
                return Some(Action::Definition(object.name.clone()));
            }
        }
    }
    None
}

pub(in crate::app) fn text_at(
    ui: &egui::Ui,
    rect: egui::Rect,
    text: &str,
    color: egui::Color32,
    size: f32,
) {
    let mut job = egui::text::LayoutJob::simple_singleline(
        text.into(),
        crate::appearance::proportional(ui.ctx(), size),
        color,
    );
    job.wrap.max_width = rect.width().max(0.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    let galley = ui.painter().layout_job(job);
    ui.painter()
        .with_clip_rect(rect.intersect(ui.clip_rect()))
        .galley(
            rect.left_center() - egui::vec2(0.0, galley.size().y / 2.0),
            galley,
            color,
        );
}
fn sql_view(ui: &mut egui::Ui, mut ddl: &str) {
    let mut layouter = |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, width| {
        crate::app::editor::highlighted(ui, buffer.as_str(), width)
    };
    ui.add(
        egui::TextEdit::multiline(&mut ddl)
            .code_editor()
            .desired_width(ui.available_width())
            .desired_rows(3)
            .layouter(&mut layouter),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use tursobench_core::{ForeignKey, Table, TableColumn};

    fn metadata() -> Inspection {
        Inspection {
            table: Table {
                name: "child".into(),
                is_table: true,
                has_rowid: true,
                binary_collations: true,
                columns: (0..8)
                    .map(|i| TableColumn {
                        name: format!("field_{i}"),
                        declared_type: "INTEGER".into(),
                        not_null: i == 0,
                        primary_key: if i < 2 { i + 1 } else { 0 },
                        hidden: if i == 3 { 2 } else { 0 },
                        default_sql: (i == 3).then(|| "7".into()),
                    })
                    .collect(),
            },
            indexes: vec![IndexInfo {
                name: "child_index".into(),
                unique: true,
                partial: true,
                columns: vec![Some("field_2".into()), None],
            }],
            foreign_keys: vec![ForeignKey {
                id: 0,
                target: "parent".into(),
                columns: vec![("field_2".into(), None)],
                on_update: "CASCADE".into(),
                on_delete: "RESTRICT".into(),
            }],
        }
    }
    fn texts(output: &egui::FullOutput) -> Vec<String> {
        output
            .shapes
            .iter()
            .filter_map(|s| {
                if let egui::Shape::Text(t) = &s.shape {
                    Some(t.galley.job.text.clone())
                } else {
                    None
                }
            })
            .collect()
    }
    #[test]
    fn metadata_states_and_actions() {
        let info = metadata();
        let schema = [SchemaObject {
            kind: "trigger".into(),
            name: "after_insert".into(),
            table: "child".into(),
            ddl: Some(
                "CREATE TRIGGER after_insert AFTER INSERT ON child BEGIN SELECT 1; END".into(),
            ),
        }];
        for width in [280.0, 396.0, 562.0, 1100.0] {
            let ctx = egui::Context::default();
            ctx.enable_accesskit();
            let structure = Structure {
                info: Some(&info),
                object: None,
                schema: &schema,
                loading: false,
                graph: None,
                incomplete: false,
                selected: Some("field_2"),
            };
            let mut section = StructureSection::Columns;
            let output =
                crate::app::test_ui::render(&ctx, egui::vec2(width, 232.0), vec![], |ui| {
                    structure.show(ui, &mut section);
                });
            let rows: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|s| {
                    if let egui::Shape::Text(t) = &s.shape {
                        if width >= 720.0 && t.pos.x >= width / 2.0 {
                            return None;
                        }
                        t.galley
                            .job
                            .text
                            .strip_prefix("field_")
                            .is_some_and(|n| n.parse::<usize>().is_ok())
                            .then_some(t.pos.y)
                    } else {
                        None
                    }
                })
                .collect();
            assert_eq!(rows.len(), 8, "width {width}");
            for pair in rows.windows(2) {
                assert!((pair[1] - pair[0] - 22.0).abs() < 0.1);
            }
            assert!(
                !texts(&output)
                    .iter()
                    .any(|t| t.contains("PK 1") || t == "Copy original DDL")
            );
            let nodes = &output
                .platform_output
                .accesskit_update
                .as_ref()
                .unwrap()
                .nodes;
            assert!(
                nodes
                    .iter()
                    .any(|(_, n)| n.label().is_some_and(|s| s.contains("field_0")
                        && s.contains("NOT NULL")
                        && s.contains("part 1")))
            );
            assert!(
                nodes
                    .iter()
                    .any(|(_, n)| n.label().is_some_and(|s| s.contains("field_3")
                        && s.contains("DEFAULT 7")
                        && s.contains("hidden 2")))
            );
        }
        for (section, expected) in [
            (StructureSection::Indexes, "child_index"),
            (StructureSection::ForeignKeys, "parent"),
            (StructureSection::Triggers, "after_insert"),
            (StructureSection::Ddl, "DDL not retained"),
        ] {
            let ctx = egui::Context::default();
            let mut section = section;
            let structure = Structure {
                info: Some(&info),
                object: None,
                schema: &schema,
                loading: false,
                graph: None,
                incomplete: false,
                selected: None,
            };
            let output =
                crate::app::test_ui::render(&ctx, egui::vec2(396.0, 232.0), vec![], |ui| {
                    structure.show(ui, &mut section);
                });
            assert!(
                texts(&output).iter().any(|s| s.contains(expected)),
                "missing {expected}"
            );
        }
        let ctx = egui::Context::default();
        let structure = Structure {
            info: None,
            object: None,
            schema: &[],
            loading: false,
            graph: None,
            incomplete: false,
            selected: None,
        };
        let mut section = StructureSection::Columns;
        let output = crate::app::test_ui::render(&ctx, egui::vec2(396.0, 232.0), vec![], |ui| {
            structure.show(ui, &mut section);
        });
        assert!(texts(&output).iter().any(|s| s == "Load structure"));
        let structure = Structure {
            loading: true,
            ..structure
        };
        let output = crate::app::test_ui::render(&ctx, egui::vec2(396.0, 232.0), vec![], |ui| {
            structure.show(ui, &mut section);
        });
        assert!(texts(&output).is_empty());
    }
}

#[cfg(test)]
mod action_tests {
    use super::*;
    #[test]
    fn sections_and_definition_expansion_do_not_need_database_reads() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let object = SchemaObject {
            kind: "table".into(),
            name: "sample".into(),
            table: "sample".into(),
            ddl: Some("CREATE TABLE sample(id INTEGER)".into()),
        };
        let schema = [SchemaObject {
            kind: "trigger".into(),
            name: "sample_trigger".into(),
            table: "sample".into(),
            ddl: Some(
                "CREATE TRIGGER sample_trigger AFTER INSERT ON sample BEGIN SELECT 1; END".into(),
            ),
        }];
        let structure = Structure {
            info: None,
            object: Some(&object),
            schema: &schema,
            loading: false,
            graph: None,
            incomplete: false,
            selected: None,
        };
        let mut section = StructureSection::Columns;
        let mut action = None;
        let mut render = |events| {
            crate::app::test_ui::render(&ctx, egui::vec2(396.0, 300.0), events, |ui| {
                action = structure.show(ui, &mut section);
            })
        };
        let output = render(vec![]);
        let pos = crate::app::test_ui::control(&output, "Load structure").center();
        render(crate::app::test_ui::pointer(pos, true));
        render(crate::app::test_ui::pointer(pos, false));
        drop(render);
        assert!(matches!(action, Some(Action::Load)));
        let mut render = |events| {
            crate::app::test_ui::render(&ctx, egui::vec2(396.0, 300.0), events, |ui| {
                structure.show(ui, &mut section);
            })
        };
        let output = render(vec![]);
        let pos = crate::app::test_ui::control(&output, "Triggers, 1").center();
        render(crate::app::test_ui::pointer(pos, true));
        render(crate::app::test_ui::pointer(pos, false));
        let output = render(vec![]);
        let pos = crate::app::test_ui::control(&output, "sample_trigger · ").center();
        render(crate::app::test_ui::pointer(pos, true));
        render(crate::app::test_ui::pointer(pos, false));
        let output = render(vec![]);
        assert!(output.shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.job.text.contains("CREATE TRIGGER"))));
        assert_eq!(section, StructureSection::Triggers);
    }
}
