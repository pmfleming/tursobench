//! Borrowed schema presentation; database actions are applied after rendering.
use crate::app::{
    icons::{self, Icon},
    results::code_view,
};
use eframe::egui;
use tursobench_core::{ForeignKey, IndexInfo, Inspection, SchemaObject, Table};

pub(super) enum Action {
    Load,
    Browse(String),
}
pub(super) struct Structure<'a> {
    pub info: Option<&'a Inspection>,
    pub object: Option<&'a SchemaObject>,
    pub schema: &'a [SchemaObject],
    pub loading: bool,
}
impl Structure<'_> {
    pub fn show(&self, ui: &mut egui::Ui) -> Option<Action> {
        self.definition(ui);
        ui.separator();
        let mut action = None;
        let count = if ui.available_width() >= 720.0 { 2 } else { 1 };
        ui.columns(count, |panes| {
            if let Some(info) = self.info {
                columns(&mut panes[0], &info.table);
            }
            let ui = &mut panes[count - 1];
            if let Some(info) = self.info {
                indexes(ui, &info.indexes);
                action = foreign_keys(ui, &info.foreign_keys).map(Action::Browse);
            } else if !self.loading
                && self
                    .object
                    .is_none_or(|o| o.kind == "table" || o.kind == "view")
                && ui.button("Load structure").clicked()
            {
                action = Some(Action::Load);
            }
        });
        self.definitions(ui);
        action
    }
    fn definition(&self, ui: &mut egui::Ui) {
        let ddl = self.object.and_then(|o| o.ddl.as_deref());
        ui.horizontal(|ui| {
            ui.strong("CREATE statement");
            if let Some(ddl) = ddl {
                copy_definition(ui, ddl, "Copy original DDL");
            }
        });
        if let Some(ddl) = ddl {
            code_view(ui, ddl, 3);
        } else {
            ui.weak("DDL not retained; refresh the catalog.");
        }
    }
    fn definitions(&self, ui: &mut egui::Ui) {
        let owner = self
            .info
            .map(|i| i.table.name.as_str())
            .or_else(|| self.object.map(|o| o.table.as_str()));
        let mut definitions = self
            .schema
            .iter()
            .filter(|o| {
                Some(o.table.as_str()) == owner && matches!(o.kind.as_str(), "index" | "trigger")
            })
            .peekable();
        if definitions.peek().is_some() {
            ui.strong("Index and trigger definitions");
        }
        for object in definitions {
            ui.group(|ui| {
                ui.strong(format!("{} {}", object.kind, object.name));
                if let Some(ddl) = &object.ddl {
                    copy_definition(ui, ddl, "Copy definition");
                    code_view(ui, ddl, 2);
                } else {
                    ui.weak("Engine-generated definition");
                }
            });
        }
    }
}
fn copy_definition(ui: &mut egui::Ui, ddl: &str, label: &str) {
    if icons::button(ui, Icon::Copy, label, "").clicked() {
        ui.ctx().copy_text(ddl.into());
    }
}
fn columns(ui: &mut egui::Ui, table: &Table) {
    if table.engine_managed() {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            "Engine-managed storage · generated edits disabled",
        );
    }
    ui.strong(format!("Columns · {}", table.columns.len()))
        .on_hover_text("Committed browse metadata. Uncommitted editor changes are not visible.");
    ui.group(|ui| {
        for c in &table.columns {
            ui.horizontal_wrapped(|ui| {
                if c.primary_key > 0 {
                    let (r, _) =
                        ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                    icons::paint(
                        ui.painter(),
                        r,
                        Icon::Key,
                        crate::appearance::Tokens::of(ui.ctx()).relationship,
                    );
                }
                ui.monospace(&c.name);
                ui.add(egui::Label::new(
                    egui::RichText::new(&c.declared_type)
                        .monospace()
                        .small()
                        .color(crate::appearance::Tokens::of(ui.ctx()).faint),
                ));
                if c.primary_key > 0 {
                    ui.small(format!("PK {}", c.primary_key));
                }
                if c.not_null {
                    ui.small("NOT NULL");
                }
                if c.hidden > 0 {
                    ui.small(format!("hidden {}", c.hidden));
                }
                if let Some(default) = &c.default_sql {
                    ui.small(format!("DEFAULT {default}"));
                }
            });
            ui.separator();
        }
    });
}
fn indexes(ui: &mut egui::Ui, indexes: &[IndexInfo]) {
    ui.strong(format!("Indexes · {}", indexes.len()));
    if indexes.is_empty() {
        ui.weak("No declared indexes returned");
    }
    for index in indexes {
        ui.group(|ui| {
            ui.monospace(&index.name);
            ui.horizontal_wrapped(|ui| {
                if index.unique {
                    ui.small("UNIQUE");
                }
                if index.partial {
                    ui.small("partial");
                }
                ui.label(
                    index
                        .columns
                        .iter()
                        .map(|c| c.as_deref().unwrap_or("expression / method: see DDL"))
                        .collect::<Vec<_>>()
                        .join(", "),
                );
            });
        });
    }
}
fn foreign_keys(ui: &mut egui::Ui, keys: &[ForeignKey]) -> Option<String> {
    let mut browse = None;
    ui.strong(format!("Foreign keys · {}", keys.len()));
    if keys.is_empty() {
        ui.weak("No declared foreign keys");
    }
    for key in keys {
        ui.group(|ui| {
            if ui
                .link(
                    egui::RichText::new(&key.target)
                        .color(crate::appearance::Tokens::of(ui.ctx()).relationship),
                )
                .clicked()
            {
                browse = Some(key.target.clone());
            }
            ui.monospace(
                key.columns
                    .iter()
                    .map(|(from, to)| {
                        format!(
                            "{from} references {}",
                            to.as_deref().unwrap_or("unresolved PK")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            ui.small(format!(
                "ON UPDATE {} · ON DELETE {}",
                key.on_update, key.on_delete
            ));
        });
    }
    browse
}

#[cfg(test)]
mod tests {
    use super::{Action, Structure};
    use eframe::egui;
    use tursobench_core::{ForeignKey, IndexInfo, Inspection, SchemaObject, Table, TableColumn};

    #[test]
    fn metadata_states_and_actions() {
        let info = Inspection {
            table: Table {
                name: "__turso_internal_test".into(),
                is_table: true,
                columns: vec![TableColumn {
                    name: "child_id".into(),
                    declared_type: "INTEGER".into(),
                    not_null: true,
                    primary_key: 1,
                    hidden: 2,
                    default_sql: Some("7".into()),
                }],
            },
            indexes: vec![IndexInfo {
                name: "expression_index".into(),
                unique: true,
                partial: true,
                columns: vec![None],
            }],
            foreign_keys: vec![ForeignKey {
                id: 0,
                target: "parent_table".into(),
                columns: vec![
                    ("child_id".into(), None),
                    ("name".into(), Some("parent_name".into())),
                ],
                on_update: "CASCADE".into(),
                on_delete: "RESTRICT".into(),
            }],
        };
        let empty = Inspection {
            table: info.table.clone(),
            indexes: vec![],
            foreign_keys: vec![],
        };
        let schema = [SchemaObject {
            kind: "trigger".into(),
            name: "after_insert".into(),
            table: info.table.name.clone(),
            ddl: None,
        }];
        for width in [640.0, 1100.0] {
            for (metadata, object, loading, expected, click) in [
                (
                    None,
                    None,
                    false,
                    vec!["Load structure", "DDL not retained"],
                    Some("Load structure"),
                ),
                (None, None, true, vec!["DDL not retained"], None),
                (
                    None,
                    Some(&schema[0]),
                    false,
                    vec!["trigger after_insert"],
                    None,
                ),
                (
                    Some(&empty),
                    None,
                    false,
                    vec!["No declared indexes", "No declared foreign keys"],
                    None,
                ),
                (
                    Some(&info),
                    None,
                    false,
                    vec![
                        "child_id",
                        "PK 1",
                        "NOT NULL",
                        "hidden 2",
                        "DEFAULT 7",
                        "UNIQUE",
                        "partial",
                        "expression / method",
                        "unresolved PK",
                        "parent_name",
                        "ON UPDATE CASCADE",
                        "Engine-managed storage",
                        "trigger after_insert",
                    ],
                    Some("parent_table"),
                ),
            ] {
                let ctx = egui::Context::default();
                let structure = Structure {
                    info: metadata,
                    object,
                    schema: &schema,
                    loading,
                };
                let mut action = None;
                let mut render = |events| {
                    crate::app::test_ui::render(&ctx, egui::vec2(width, 1200.0), events, |ui| {
                        action = structure.show(ui)
                    })
                };
                let output = render(vec![]);
                let texts: Vec<_> = output
                    .shapes
                    .iter()
                    .filter_map(|s| match &s.shape {
                        egui::Shape::Text(t) => Some(t),
                        _ => None,
                    })
                    .collect();
                for expected in expected {
                    assert!(
                        texts.iter().any(|t| t.galley.job.text.contains(expected)),
                        "missing {expected}"
                    );
                }
                assert_eq!(
                    texts.iter().any(|t| t.galley.job.text == "Load structure"),
                    metadata.is_none() && object.is_none() && !loading
                );
                if let Some(label) = click {
                    let text = texts.iter().find(|t| t.galley.job.text == label).unwrap();
                    let pos = text.pos + text.galley.size() / 2.0;
                    for pressed in [true, false] {
                        render(crate::app::test_ui::pointer(pos, pressed));
                    }
                    match action {
                        Some(Action::Load) => assert_eq!(label, "Load structure"),
                        Some(Action::Browse(name)) => assert_eq!(name, label),
                        None => panic!("missing action for {label}"),
                    }
                }
            }
        }
    }
}
