use crate::{Failure, Table, Value, quote_identifier};

#[derive(Debug, Clone, PartialEq)]
pub enum Edit {
    Insert {
        values: Vec<Option<Value>>,
    },
    Update {
        original: Vec<Value>,
        changes: Vec<(usize, Value)>,
    },
    Delete {
        original: Vec<Value>,
    },
}
#[derive(Debug, Clone)]
pub struct DraftBatch {
    pub table: Table,
    pub edits: Vec<Edit>,
}
#[derive(Debug, Clone)]
pub struct BoundEdit {
    pub sql: String,
    pub parameters: Vec<Value>,
}
#[derive(Debug, Clone)]
pub enum CommitOutcome {
    Committed { operations: usize },
    Rejected(Failure),
    Unknown(Failure),
}
impl DraftBatch {
    pub fn stage_cell(&mut self, original: Vec<Value>, column: usize, value: Value) {
        self.edits
            .retain(|e| !matches!(e, Edit::Delete { original: o } if *o == original));
        if let Some(Edit::Update { changes, .. }) = self
            .edits
            .iter_mut()
            .find(|e| matches!(e, Edit::Update { original: o, .. } if *o == original))
        {
            if let Some((_, v)) = changes.iter_mut().find(|(i, _)| *i == column) {
                *v = value;
            } else {
                changes.push((column, value));
            }
        } else {
            self.edits.push(Edit::Update {
                original,
                changes: vec![(column, value)],
            });
        }
    }
    pub fn revert_cell(&mut self, original: &[Value], column: usize) {
        for edit in &mut self.edits {
            if let Edit::Update {
                original: o,
                changes,
            } = edit
                && o == original
            {
                changes.retain(|(i, _)| *i != column);
            }
        }
        self.edits
            .retain(|e| !matches!(e, Edit::Update { changes, .. } if changes.is_empty()));
    }
    pub fn revert_row(&mut self, original: &[Value]) {
        self.edits.retain(|e| !matches!(e, Edit::Update { original: o, .. } | Edit::Delete { original: o } if o == original));
    }
    pub fn displayed(&self, original: &[Value], column: usize) -> Option<&Value> {
        self.edits.iter().find_map(|e| match e {
            Edit::Update {
                original: o,
                changes,
            } if o == original => changes.iter().find(|(i, _)| *i == column).map(|(_, v)| v),
            _ => None,
        })
    }
    pub fn deleted(&self, original: &[Value]) -> bool {
        self.edits
            .iter()
            .any(|e| matches!(e, Edit::Delete { original: o } if o == original))
    }
    pub fn stage_delete(&mut self, original: Vec<Value>) {
        self.edits.retain(|e| !matches!(e, Edit::Update { original: o, .. } | Edit::Delete { original: o } if *o == original));
        self.edits.push(Edit::Delete { original });
    }
    pub fn preview(&self) -> Result<Vec<BoundEdit>, Failure> {
        if !self.table.editable() {
            return Err(Failure::input(
                "Only base tables with a proven primary key and no hidden/generated columns are editable",
            ));
        }
        if self.edits.is_empty() || self.edits.len() > 1000 {
            return Err(Failure::input("Stage between 1 and 1,000 operations"));
        }
        let statements = self
            .edits
            .iter()
            .map(|e| generate(&self.table, e))
            .collect::<Result<Vec<_>, _>>()?;
        if statements
            .iter()
            .map(|s| s.sql.len() + s.parameters.iter().map(Value::bytes).sum::<usize>())
            .sum::<usize>()
            > 8 * 1024 * 1024
        {
            return Err(Failure::input("Edit batch exceeds 8 MiB"));
        }
        Ok(statements)
    }
}
fn generate(table: &Table, edit: &Edit) -> Result<BoundEdit, Failure> {
    let name = quote_identifier(&table.name);
    let column = |i: usize| {
        table
            .columns
            .get(i)
            .map(|c| quote_identifier(&c.name))
            .ok_or_else(|| Failure::input("Invalid edit column"))
    };
    let mut params = Vec::new();
    let mut bind = |value: Value| -> Result<String, Failure> {
        if matches!(value, Value::Deferred(_))
            || value.bytes() > 64 * 1024
            || matches!(&value, Value::Real(n) if !n.is_finite())
        {
            return Err(Failure::input("Edit value is oversized or non-finite"));
        }
        params.push(value);
        Ok(format!("?{}", params.len()))
    };
    let predicate = |original: &[Value],
                     bind: &mut dyn FnMut(Value) -> Result<String, Failure>|
     -> Result<String, Failure> {
        if original.len() != table.columns.len()
            || table
                .keys()
                .iter()
                .any(|i| original.get(*i) == Some(&Value::Null))
        {
            return Err(Failure::input(
                "Original row lacks a complete non-NULL primary key",
            ));
        }
        original
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let name = column(i)?;
                let kind = match v {
                    Value::Null => "null",
                    Value::Integer(_) => "integer",
                    Value::Real(_) => "real",
                    Value::Text(_) => "text",
                    Value::Blob(_) => "blob",
                    Value::Deferred(_) => return Err(Failure::input("Original row contains deferred cells; full originals are required for editing")),
                };
                Ok(format!(
                    "(typeof({name}) = {} AND {name} COLLATE BINARY IS {})",
                    bind(Value::Text(kind.into()))?,
                    bind(v.clone())?
                ))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(|p| p.join(" AND "))
    };
    let sql = match edit {
        Edit::Insert { values } => {
            if values.len() != table.columns.len() {
                return Err(Failure::input(
                    "Insert must specify one optional value per column",
                ));
            }
            let mut columns = Vec::new();
            let mut slots = Vec::new();
            for (i, v) in values.iter().enumerate() {
                if let Some(v) = v {
                    columns.push(column(i)?);
                    slots.push(bind(v.clone())?);
                }
            }
            if columns.is_empty() {
                format!("INSERT INTO {name} DEFAULT VALUES")
            } else {
                format!(
                    "INSERT INTO {name} ({}) VALUES ({})",
                    columns.join(", "),
                    slots.join(", ")
                )
            }
        }
        Edit::Update { original, changes } => {
            if changes.is_empty() {
                return Err(Failure::input("Empty update"));
            }
            let mut seen = std::collections::HashSet::new();
            let mut assignments = Vec::new();
            for (i, v) in changes {
                if !seen.insert(i) {
                    return Err(Failure::input("Duplicate update column"));
                }
                assignments.push(format!("{} = {}", column(*i)?, bind(v.clone())?));
            }
            format!(
                "UPDATE {name} SET {} WHERE {}",
                assignments.join(", "),
                predicate(original, &mut bind)?
            )
        }
        Edit::Delete { original } => format!(
            "DELETE FROM {name} WHERE {}",
            predicate(original, &mut bind)?
        ),
    };
    Ok(BoundEdit {
        sql,
        parameters: params,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TableColumn;
    fn table() -> Table {
        Table {
            name: "a\"b".into(),
            is_table: true,
            has_rowid: true,
            binary_collations: true,
            columns: vec![TableColumn {
                name: "id".into(),
                declared_type: "INTEGER".into(),
                not_null: false,
                primary_key: 1,
                hidden: 0,
                default_sql: None,
            }],
        }
    }
    #[test]
    fn default_is_not_null_and_changed_keys_keep_original() {
        let batch = |edit| DraftBatch {
            table: table(),
            edits: vec![edit],
        };
        assert!(
            batch(Edit::Insert { values: vec![None] })
                .preview()
                .unwrap()[0]
                .sql
                .ends_with("DEFAULT VALUES")
        );
        let s = batch(Edit::Insert {
            values: vec![Some(Value::Null)],
        })
        .preview()
        .unwrap();
        assert_eq!(s[0].parameters, vec![Value::Null]);
        let s = batch(Edit::Update {
            original: vec![Value::Integer(1)],
            changes: vec![(0, Value::Integer(2))],
        })
        .preview()
        .unwrap();
        assert_eq!(
            s[0].parameters,
            vec![
                Value::Integer(2),
                Value::Text("integer".into()),
                Value::Integer(1)
            ]
        );
        assert!(s[0].sql.contains("\"a\"\"b\""));
    }
    #[test]
    fn null_key_and_keyless_rejected() {
        let mut b = DraftBatch {
            table: table(),
            edits: vec![Edit::Delete {
                original: vec![Value::Null],
            }],
        };
        assert!(b.preview().is_err());
        let insert = |value| Edit::Insert {
            values: vec![Some(value)],
        };
        let update = |changes| Edit::Update {
            original: vec![Value::Integer(1)],
            changes,
        };
        for edit in [
            Edit::Insert { values: vec![] },
            insert(Value::Real(f64::NAN)),
            insert(Value::Text("x".repeat(64 * 1024))),
            update(vec![]),
            update(vec![(1, Value::Null)]),
            update(vec![(0, Value::Null), (0, Value::Null)]),
            Edit::Delete { original: vec![] },
        ] {
            b.edits = vec![edit];
            assert!(b.preview().is_err(), "{:?}", b.edits);
        }
        for edits in [
            vec![],
            vec![Edit::Insert { values: vec![None] }; 1001],
            vec![
                Edit::Insert {
                    values: vec![Some(Value::Blob(vec![0; 60 * 1024]))]
                };
                140
            ],
        ] {
            b.edits = edits;
            assert!(b.preview().is_err());
        }
        b.table.columns[0].primary_key = 0;
        assert!(b.preview().is_err());
    }
}
