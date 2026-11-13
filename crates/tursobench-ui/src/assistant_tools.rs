//! The tools TursoBench offers an agent: read the schema, run one bounded
//! read-only query when the user shared rows, and propose SQL for the user to
//! review. Nothing here can write to a database.
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, Ordering},
        mpsc::Sender,
    },
    time::Duration,
};
use tursobench_assistant::mcp::{ToolHost, ToolOutput, ToolSpec};
use tursobench_db::assistant::{Handle, MAX_ROWS, Tool};

pub const LIST_TABLES: &str = "list_tables";
pub const DESCRIBE_TABLE: &str = "describe_table";
pub const RUN_QUERY: &str = "run_query";
pub const PROPOSE_SQL: &str = "propose_sql";
pub const NAMES: [&str; 4] = [LIST_TABLES, DESCRIBE_TABLE, RUN_QUERY, PROPOSE_SQL];
const TIMEOUT: Duration = Duration::from_secs(60);
const MAX_PROPOSAL: usize = 256 * 1024;

/// What the user lets the agent read, chosen per database.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Sharing {
    /// Names, columns, keys, indexes and DDL; no row values.
    Schema,
    /// The schema and the results of read-only queries.
    Rows,
}

pub struct Proposal {
    pub sql: String,
}

pub struct Host {
    pub session: Arc<Mutex<Option<Handle>>>,
    sharing: AtomicU8,
    proposals: Sender<Proposal>,
    wake: Arc<dyn Fn() + Send + Sync>,
}
impl Host {
    pub fn new(
        session: Arc<Mutex<Option<Handle>>>,
        sharing: Sharing,
        proposals: Sender<Proposal>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        let host = Self {
            session,
            sharing: AtomicU8::new(0),
            proposals,
            wake,
        };
        host.set_sharing(sharing);
        host
    }
    pub fn set_sharing(&self, sharing: Sharing) {
        self.sharing.store(
            match sharing {
                Sharing::Schema => 0,
                Sharing::Rows => 1,
            },
            Ordering::Release,
        );
    }
    fn rows_shared(&self) -> bool {
        self.sharing.load(Ordering::Acquire) == 1
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
                              allowed) against committed data and get at most 200 rows as \
                              JSON. Long text is shortened and blobs show only their size. \
                              Writes are refused. Only available when the user shares \
                              query results.",
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
        let result =
            match name {
                LIST_TABLES => Ok(self.database(Tool::ListTables)),
                DESCRIBE_TABLE => text_argument(arguments, "name")
                    .map(|n| self.database(Tool::Describe(n.into()))),
                RUN_QUERY if !self.rows_shared() => Ok(ToolOutput::error(
                    "The user shares only the schema with you, not row data. Describe the \
                 query instead, or use propose_sql so they can run it.",
                )),
                RUN_QUERY => text_argument(arguments, "sql")
                    .map(|sql| self.database(Tool::Query(sql.into()))),
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
        format!(
            "Tools for the Turso database open in TursoBench. Reads see committed data \
             only and return at most {MAX_ROWS} rows. You cannot change the database; \
             use propose_sql for changes."
        )
    }
}

/// The agent's standing instructions for one database conversation.
pub fn system_prompt(database: &str, sharing: Sharing) -> String {
    let shared = match sharing {
        Sharing::Schema => {
            "The user shares only the schema. run_query is refused; reason from the \
             schema and use propose_sql when they should run something."
        }
        Sharing::Rows => {
            "The user shares query results, so run_query is available. Keep queries \
             small: select only needed columns, aggregate, and use LIMIT."
        }
    };
    format!(
        "You are the assistant inside TursoBench, a desktop workbench for local Turso \
         databases. Turso is an SQLite-compatible engine; native full-text search uses \
         `CREATE INDEX … USING fts(...)` and `fts_match(...)`.\n\
         The user has the database `{database}` open and is asking about it.\n\
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
    use super::*;
    use std::sync::mpsc;
    #[test]
    fn schema_only_sharing_refuses_queries_and_proposals_reach_the_ui() {
        let (tx, rx) = mpsc::channel();
        let host = Host::new(
            Arc::new(Mutex::new(None)),
            Sharing::Schema,
            tx,
            Arc::new(|| {}),
        );
        let refused = host.call(RUN_QUERY, &json!({"sql": "SELECT 1"}));
        assert!(refused.is_error && refused.text.contains("only the schema"));
        host.set_sharing(Sharing::Rows);
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
        assert!(system_prompt("shop.db", Sharing::Schema).contains("`shop.db`"));
    }
}
