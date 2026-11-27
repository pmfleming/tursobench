//! The tools TursoBench offers an agent: read the schema, run one bounded
//! read-only query when the user shared rows, and propose SQL for the user to
//! review. Nothing here can write to a database.
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use std::{
    sync::{Arc, Mutex, mpsc::Sender},
    time::Duration,
};
use tursobench_assistant::mcp::{ToolHost, ToolOutput, ToolSpec};
use tursobench_db::assistant::{Handle, MAX_BYTES, MAX_ROWS, MOST_BYTES, MOST_ROWS, Tool};

pub const LIST_TABLES: &str = "list_tables";
pub const DESCRIBE_TABLE: &str = "describe_table";
pub const RUN_QUERY: &str = "run_query";
pub const PROPOSE_SQL: &str = "propose_sql";
pub const NAMES: [&str; 4] = [LIST_TABLES, DESCRIBE_TABLE, RUN_QUERY, PROPOSE_SQL];
const TIMEOUT: Duration = Duration::from_secs(60);
const MAX_PROPOSAL: usize = 256 * 1024;

/// What the user lets every agent read in one database; a default, changeable
/// per database.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Sharing {
    /// Names, columns, keys, indexes and DDL; no row values.
    Schema,
    /// The schema and up to 200 rows of a read-only query.
    #[default]
    Limited,
    /// The schema and read-only queries up to the database's row limit.
    Complete,
}
impl Sharing {
    pub const ALL: [Self; 3] = [Self::Schema, Self::Limited, Self::Complete];
}

/// A database's sharing and its row limit, which bounds a complete read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentAccess {
    pub sharing: Sharing,
    pub row_limit: usize,
}
impl AgentAccess {
    /// The most rows one query returns; none when only the schema is shared.
    /// A complete read never returns fewer than a limited one.
    pub fn rows(self) -> usize {
        match self.sharing {
            Sharing::Schema => 0,
            Sharing::Limited => MAX_ROWS,
            Sharing::Complete => self.row_limit.clamp(MAX_ROWS, MOST_ROWS),
        }
    }
    /// The most JSON one query returns.
    pub fn bytes(self) -> usize {
        match self.sharing {
            Sharing::Complete => MOST_BYTES,
            _ => MAX_BYTES,
        }
    }
}

pub struct Proposal {
    pub sql: String,
}

pub struct Host {
    pub session: Arc<Mutex<Option<Handle>>>,
    /// What the user shares now; it changes while the agent runs.
    access: Mutex<AgentAccess>,
    proposals: Sender<Proposal>,
    wake: Arc<dyn Fn() + Send + Sync>,
}
impl Host {
    pub fn new(
        session: Arc<Mutex<Option<Handle>>>,
        access: AgentAccess,
        proposals: Sender<Proposal>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        Self {
            session,
            access: Mutex::new(access),
            proposals,
            wake,
        }
    }
    pub fn set_access(&self, access: AgentAccess) {
        *self.access.lock().unwrap() = access;
    }
    pub fn access(&self) -> AgentAccess {
        *self.access.lock().unwrap()
    }
    fn database(&self, tool: Tool) -> ToolOutput {
        let Some(handle) = self.session.lock().unwrap().clone() else {
            return ToolOutput::error("No database is open in TursoBench right now.");
        };
        match handle.call(tool, TIMEOUT) {
            Ok(value) => ToolOutput::ok(value.to_string()),
            Err(error) => ToolOutput::error(error.message),
        }
    }
}

fn text_argument<'a>(arguments: &'a Json, name: &str) -> Result<&'a str, ToolOutput> {
    arguments
        .get(name)
        .and_then(Json::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| ToolOutput::error(format!("`{name}` is required")))
}

impl ToolHost for Host {
    fn tools(&self) -> Vec<ToolSpec> {
        let text = |description: &str| json!({"type": "string", "description": description});
        vec![
            ToolSpec {
                name: LIST_TABLES,
                description: "List the tables and views in the open database.",
                input_schema: json!({"type": "object", "properties": {}}),
            },
            ToolSpec {
                name: DESCRIBE_TABLE,
                description: "Describe one table or view: columns with declared types, \
                              primary key, defaults, indexes, foreign keys and its CREATE \
                              statement.",
                input_schema: json!({
                    "type": "object",
                    "properties": {"name": text("Exact table or view name")},
                    "required": ["name"],
                }),
            },
            ToolSpec {
                name: RUN_QUERY,
                description: "Run one read-only SELECT (WITH and EXPLAIN QUERY PLAN are \
                              allowed) against committed data and get its rows as JSON, \
                              as many as the user shares (see the instructions). Long \
                              text is shortened and blobs show only their size. Writes \
                              are refused. Only available when the user shares query \
                              results.",
                input_schema: json!({
                    "type": "object",
                    "properties": {"sql": text("A single SELECT statement; prefer LIMIT")},
                    "required": ["sql"],
                }),
            },
            ToolSpec {
                name: PROPOSE_SQL,
                description: "Open SQL in a new TursoBench editor tile for the user to \
                              review and run themselves. Use this for any change \
                              (INSERT, UPDATE, DELETE, DDL) or a query you want the user \
                              to keep. It does not execute anything.",
                input_schema: json!({
                    "type": "object",
                    "properties": {"sql": text("The SQL to show the user")},
                    "required": ["sql"],
                }),
            },
        ]
    }
    fn call(&self, name: &str, arguments: &Json) -> ToolOutput {
        let access = self.access();
        let result = match name {
            LIST_TABLES => Ok(self.database(Tool::ListTables)),
            DESCRIBE_TABLE => {
                text_argument(arguments, "name").map(|n| self.database(Tool::Describe(n.into())))
            }
            RUN_QUERY if access.rows() == 0 => Ok(ToolOutput::error(
                "The user shares only the schema with you, not row data. Describe the \
                 query instead, or use propose_sql so they can run it.",
            )),
            RUN_QUERY => text_argument(arguments, "sql").map(|sql| {
                self.database(Tool::Query {
                    sql: sql.into(),
                    rows: access.rows(),
                    bytes: access.bytes(),
                })
            }),
            PROPOSE_SQL => text_argument(arguments, "sql").map(|sql| {
                if sql.len() > MAX_PROPOSAL {
                    return ToolOutput::error("The SQL is larger than 256 KiB");
                }
                if self.proposals.send(Proposal { sql: sql.into() }).is_err() {
                    return ToolOutput::error("TursoBench could not open an editor tile");
                }
                (self.wake)();
                ToolOutput::ok(
                    "Opened in a new SQL editor tile. It has not been run; the user will \
                     review it and decide.",
                )
            }),
            other => Ok(ToolOutput::error(format!("Unknown tool {other}"))),
        };
        result.unwrap_or_else(|error| error)
    }
    fn instructions(&self) -> String {
        "Tools for the Turso database open in TursoBench. Reads see committed data \
         only and return as many rows as the user shares; each result says when it \
         was cut. You cannot change the database; use propose_sql for changes."
            .into()
    }
}

/// What the agent is told it may read under `access`.
pub fn shared_text(access: AgentAccess) -> String {
    match access.sharing {
        Sharing::Schema => "The user shares only the schema. run_query is refused; reason \
                            from the schema and use propose_sql when they should run \
                            something."
            .into(),
        sharing => {
            let rows = access.rows();
            let scope = if sharing == Sharing::Complete {
                "complete query results"
            } else {
                "limited query results"
            };
            format!(
                "The user shares {scope}: run_query returns at most {rows} rows. Keep \
                 queries small: select only needed columns, aggregate, and use LIMIT."
            )
        }
    }
}

/// The agent's standing instructions for one database conversation.
/// `database` is the file name, or `None` when the user keeps even that private.
pub fn system_prompt(database: Option<&str>, access: AgentAccess) -> String {
    let open = match database {
        Some(name) => format!("The user has the database `{name}` open and is asking about it."),
        None => "The user has a database open and is asking about it.".to_owned(),
    };
    let shared = shared_text(access);
    format!(
        "You are the assistant inside TursoBench, a desktop workbench for local Turso \
         databases. Turso is an SQLite-compatible engine; native full-text search uses \
         `CREATE INDEX … USING fts(...)` and `fts_match(...)`.\n\
         {open}\n\
         Use the tursobench tools to look before answering: list_tables, describe_table, \
         run_query and propose_sql. You cannot write to the database. For any change, \
         or a query the user should keep, call propose_sql; it opens the SQL in their \
         editor without running it, and they decide.\n\
         {shared}\n\
         Answer concisely in Markdown. Put SQL in ```sql fenced blocks. Quote identifiers \
         that need it with double quotes. Do not claim to have run anything you only \
         proposed."
    )
}

#[cfg(test)]
mod tests {
    use super::{
        AgentAccess, Arc, DESCRIBE_TABLE, Host, Mutex, NAMES, PROPOSE_SQL, RUN_QUERY, Sharing,
        ToolHost, json, system_prompt,
    };
    use std::sync::mpsc;
    fn access(sharing: Sharing, row_limit: usize) -> AgentAccess {
        AgentAccess { sharing, row_limit }
    }
    #[test]
    fn schema_only_sharing_refuses_queries_and_proposals_reach_the_ui() {
        let (tx, rx) = mpsc::channel();
        let host = Host::new(
            Arc::new(Mutex::new(None)),
            access(Sharing::Schema, 1000),
            tx,
            Arc::new(|| {}),
        );
        let refused = host.call(RUN_QUERY, &json!({"sql": "SELECT 1"}));
        assert!(refused.is_error && refused.text.contains("only the schema"));
        host.set_access(access(Sharing::Limited, 1000));
        let closed = host.call(RUN_QUERY, &json!({"sql": "SELECT 1"}));
        assert!(closed.is_error && closed.text.contains("No database"));
        assert!(host.call(DESCRIBE_TABLE, &json!({})).is_error);
        let proposed = host.call(PROPOSE_SQL, &json!({"sql": "DELETE FROM t"}));
        assert!(!proposed.is_error);
        assert_eq!(rx.try_recv().unwrap().sql, "DELETE FROM t");
        assert_eq!(
            host.tools().iter().map(|t| t.name).collect::<Vec<_>>(),
            NAMES
        );
        let schema = access(Sharing::Schema, 1000);
        assert!(system_prompt(Some("shop.db"), schema).contains("`shop.db`"));
        let unnamed = system_prompt(None, access(Sharing::Limited, 1000));
        assert!(!unnamed.contains(".db") && unnamed.contains("a database open"));
        assert!(unnamed.contains("at most 200 rows"));
    }
    #[test]
    fn each_level_reads_as_many_rows_as_it_shares() {
        let rows = |sharing, limit| access(sharing, limit).rows();
        assert_eq!(rows(Sharing::Schema, 5000), 0);
        assert_eq!(rows(Sharing::Limited, 5000), 200);
        // A complete read follows the database's row limit, within the bounds.
        assert_eq!(rows(Sharing::Complete, 5000), 5000);
        assert_eq!(rows(Sharing::Complete, 50), 200);
        assert_eq!(rows(Sharing::Complete, 50_000), 10_000);
        let prompt = system_prompt(None, access(Sharing::Complete, 5000));
        assert!(prompt.contains("complete query results") && prompt.contains("5000 rows"));
    }
}
