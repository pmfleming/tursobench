//! Copying the chat as text: the last reply or the whole conversation, in a
//! small Markdown subset or the matching HTML (headings, paragraphs, lists,
//! code blocks, inline code and bold).
use super::assistant::Entry;
use crate::assistant_tools as tools;
use crate::storage::CopyFormat;
use serde_json::Value as Json;

/// The most recent reply, or `None` before the agent has answered.
pub(super) fn last_reply(entries: &[Entry], format: CopyFormat) -> Option<String> {
    entries.iter().rev().find_map(|entry| match entry {
        Entry::Reply(text) => Some(render(text.trim(), format)),
        _ => None,
    })
}

/// Every message, reply, tool call and notice; the agent's private thinking
/// is left out.
pub(super) fn conversation(entries: &[Entry], agent: &str, format: CopyFormat) -> String {
    let mut out = String::new();
    let mut speaker = "";
    for entry in entries {
        let (who, body) = match entry {
            Entry::User(text) => ("You", text.trim().to_owned()),
            Entry::Reply(text) => (agent, text.trim().to_owned()),
            Entry::Tool { name, input, .. } => (agent, tool(name, input)),
            Entry::Notice(_, text) => ("", text.trim().to_owned()),
            Entry::Thought(_) => continue,
        };
        if who != speaker && !who.is_empty() {
            out.push_str(&format!("## {who}\n\n"));
            speaker = who;
        }
        out.push_str(&body);
        out.push_str("\n\n");
    }
    render(out.trim_end(), format)
}

fn tool(name: &str, input: &Json) -> String {
    let argument = |key: &str| input.get(key).and_then(Json::as_str).unwrap_or("").trim();
    let sql = |label: &str| format!("- {label}\n\n```sql\n{}\n```", argument("sql"));
    match name {
        tools::LIST_TABLES => "- Listed tables".into(),
        tools::DESCRIBE_TABLE => format!("- Described `{}`", argument("name")),
        tools::RUN_QUERY => sql("Ran a query"),
        tools::PROPOSE_SQL => sql("Proposed SQL"),
        other => format!("- Used `{other}`"),
    }
}

fn render(markdown: &str, format: CopyFormat) -> String {
    match format {
        CopyFormat::Markdown => markdown.to_owned(),
        CopyFormat::Html => html(markdown),
    }
}

/// The Markdown subset above as HTML; everything else stays escaped text.
fn html(markdown: &str) -> String {
    let mut out = Vec::new();
    let mut paragraph: Vec<&str> = vec![];
    let mut list: Vec<String> = vec![];
    let flush = |out: &mut Vec<String>, paragraph: &mut Vec<&str>, list: &mut Vec<String>| {
        if !paragraph.is_empty() {
            out.push(format!("<p>{}</p>", inline(&paragraph.join("\n"))));
            paragraph.clear();
        }
        if !list.is_empty() {
            out.push(format!("<ul>{}</ul>", list.concat()));
            list.clear();
        }
    };
    let mut lines = markdown.lines();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            flush(&mut out, &mut paragraph, &mut list);
            let code: Vec<&str> = lines
                .by_ref()
                .take_while(|l| !l.trim_start().starts_with("```"))
                .collect();
            out.push(format!(
                "<pre><code>{}</code></pre>",
                escape(&code.join("\n"))
            ));
        } else if let Some((level, heading)) = [("### ", 3), ("## ", 2), ("# ", 1)]
            .into_iter()
            .find_map(|(mark, level)| trimmed.strip_prefix(mark).map(|h| (level, h)))
        {
            flush(&mut out, &mut paragraph, &mut list);
            out.push(format!("<h{level}>{}</h{level}>", inline(heading)));
        } else if let Some(item) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            if !paragraph.is_empty() {
                flush(&mut out, &mut paragraph, &mut vec![]);
            }
            list.push(format!("<li>{}</li>", inline(item)));
        } else if trimmed.is_empty() {
            flush(&mut out, &mut paragraph, &mut list);
        } else {
            if !list.is_empty() {
                flush(&mut out, &mut vec![], &mut list);
            }
            paragraph.push(line);
        }
    }
    flush(&mut out, &mut paragraph, &mut list);
    out.join("\n")
}

/// `code` and **bold**, as the chat panel draws them.
fn inline(text: &str) -> String {
    let mut out = String::new();
    for (index, part) in text.split('`').enumerate() {
        if index % 2 == 1 {
            out.push_str(&format!("<code>{}</code>", escape(part)));
            continue;
        }
        for (piece, segment) in part.split("**").enumerate() {
            if piece > 0 {
                out.push_str(if piece % 2 == 1 {
                    "<strong>"
                } else {
                    "</strong>"
                });
            }
            out.push_str(&escape(segment).replace('\n', "<br>"));
        }
        if part.split("**").count() % 2 == 0 {
            out.push_str("</strong>");
        }
    }
    out
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::presentation::Intent;
    use serde_json::json;

    fn chat() -> Vec<Entry> {
        vec![
            Entry::User("Which project is **busiest**?".into()),
            Entry::Thought("private".into()),
            Entry::Tool {
                id: "1".into(),
                name: tools::RUN_QUERY.into(),
                input: json!({"sql": "SELECT 1 < 2"}),
                result: Some((true, "{}".into())),
            },
            Entry::Reply("**Launch**, with `84` items:\n\n- one\n- two".into()),
            Entry::Notice(Intent::Neutral, "Now sharing only the schema".into()),
        ]
    }

    #[test]
    fn copies_the_last_reply_in_either_format() {
        let entries = chat();
        assert_eq!(
            last_reply(&entries, CopyFormat::Markdown).unwrap(),
            "**Launch**, with `84` items:\n\n- one\n- two"
        );
        assert_eq!(
            last_reply(&entries, CopyFormat::Html).unwrap(),
            "<p><strong>Launch</strong>, with <code>84</code> items:</p>\n\
             <ul><li>one</li><li>two</li></ul>"
        );
        assert_eq!(last_reply(&entries[..1], CopyFormat::Markdown), None);
    }

    #[test]
    fn copies_the_conversation_without_private_thinking() {
        let markdown = conversation(&chat(), "Codex", CopyFormat::Markdown);
        assert!(
            markdown.starts_with("## You\n\nWhich project"),
            "{markdown}"
        );
        assert!(markdown.contains("## Codex\n\n- Ran a query\n\n```sql\nSELECT 1 < 2\n```"));
        assert!(markdown.ends_with("Now sharing only the schema"));
        assert!(!markdown.contains("private"));
        // The agent's tool call and reply share one heading.
        assert_eq!(markdown.matches("## Codex").count(), 1);

        let html = conversation(&chat(), "Codex", CopyFormat::Html);
        assert!(
            html.starts_with("<h2>You</h2>\n<p>Which project is <strong>busiest</strong>?</p>")
        );
        assert!(
            html.contains("<pre><code>SELECT 1 &lt; 2</code></pre>"),
            "{html}"
        );
        assert!(html.ends_with("<p>Now sharing only the schema</p>"));
    }
}
