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

/// How the copy buttons write text, as Settings names each format.
pub(super) const FORMATS: [(CopyFormat, &str); 2] = [
    (CopyFormat::Markdown, "Markdown"),
    (CopyFormat::Html, "HTML"),
];

pub(super) fn format_name(format: CopyFormat) -> &'static str {
    FORMATS
        .into_iter()
        .find_map(|(choice, name)| (choice == format).then_some(name))
        .unwrap_or_default()
}

/// One block of the chat's Markdown subset, as the panel and the copies read it.
pub(super) enum Block<'a> {
    /// A fenced block: its language, lowercased, and its text.
    Code {
        language: String,
        code: String,
    },
    Heading(u8, &'a str),
    Item(&'a str),
    Blank,
    /// A line of a paragraph, untrimmed.
    Text(&'a str),
}

pub(super) fn blocks(markdown: &str) -> Vec<Block<'_>> {
    let mut lines = markdown.lines();
    let mut blocks = vec![];
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        blocks.push(if let Some(language) = trimmed.strip_prefix("```") {
            let code: Vec<&str> = lines
                .by_ref()
                .take_while(|l| !l.trim_start().starts_with("```"))
                .collect();
            Block::Code {
                language: language.trim().to_lowercase(),
                code: code.join("\n"),
            }
        } else if let Some((level, heading)) = [("### ", 3), ("## ", 2), ("# ", 1)]
            .into_iter()
            .find_map(|(mark, level)| trimmed.strip_prefix(mark).map(|h| (level, h)))
        {
            Block::Heading(level, heading)
        } else if let Some(item) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            Block::Item(item)
        } else if trimmed.is_empty() {
            Block::Blank
        } else {
            Block::Text(line)
        });
    }
    blocks
}

/// The Markdown subset above as HTML; everything else stays escaped text.
fn html(markdown: &str) -> String {
    let mut out = Vec::new();
    let mut paragraph: Vec<&str> = vec![];
    let mut list: Vec<String> = vec![];
    // A closing blank ends the last paragraph or list.
    for block in blocks(markdown).into_iter().chain([Block::Blank]) {
        // A list item ends a paragraph, a text line ends a list, anything else
        // ends both; at most one of them is open.
        if !matches!(block, Block::Text(_)) && !paragraph.is_empty() {
            out.push(format!("<p>{}</p>", inline(&paragraph.join("\n"))));
            paragraph.clear();
        }
        if !matches!(block, Block::Item(_)) && !list.is_empty() {
            out.push(format!("<ul>{}</ul>", list.concat()));
            list.clear();
        }
        match block {
            Block::Code { code, .. } => {
                out.push(format!("<pre><code>{}</code></pre>", escape(&code)));
            }
            Block::Heading(level, heading) => {
                out.push(format!("<h{level}>{}</h{level}>", inline(heading)));
            }
            Block::Item(item) => list.push(format!("<li>{}</li>", inline(item))),
            Block::Text(line) => paragraph.push(line),
            Block::Blank => {}
        }
    }
    out.join("\n")
}

/// How a run of a line's text is set.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Span {
    Plain,
    Bold,
    Code,
}

/// A line's `code` and **bold** runs, in order; `bold` starts it in bold. Bold
/// carries on past code until its closing marks.
pub(super) fn spans(text: &str, mut bold: bool) -> Vec<(Span, &str)> {
    let mut spans = vec![];
    for (index, part) in text.split('`').enumerate() {
        if index % 2 == 1 {
            spans.push((Span::Code, part));
            continue;
        }
        for (piece, segment) in part.split("**").enumerate() {
            bold ^= piece > 0;
            spans.push((if bold { Span::Bold } else { Span::Plain }, segment));
        }
    }
    spans
}

/// `code` and **bold**, as the chat panel draws them.
fn inline(text: &str) -> String {
    let mut out = String::new();
    let mut strong = false;
    for (span, text) in spans(text, false) {
        if span != Span::Code && strong != (span == Span::Bold) {
            strong = !strong;
            out.push_str(if strong { "<strong>" } else { "</strong>" });
        }
        match span {
            Span::Code => out.push_str(&format!("<code>{}</code>", escape(text))),
            _ => out.push_str(&escape(text).replace('\n', "<br>")),
        }
    }
    if strong {
        out.push_str("</strong>");
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
    use super::{CopyFormat, Entry, conversation, inline, last_reply, tools};
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
        // Bold carries past inline code, as the panel draws it.
        assert_eq!(
            inline("**a `b` c** d"),
            "<strong>a <code>b</code> c</strong> d"
        );
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
