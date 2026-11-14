//! Provider identities and local executable discovery. No shell is evaluated.
use crate::agent::Protocol;
use serde::{Deserialize, Serialize};
use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

#[derive(
    Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
pub enum Provider {
    #[default]
    #[serde(alias = "claude")]
    Claude,
    #[serde(alias = "copilot")]
    Copilot,
    #[serde(alias = "codex")]
    Codex,
    #[serde(alias = "antigravity")]
    Antigravity,
    #[serde(alias = "custom")]
    Custom,
}

pub struct Vendor {
    pub id: Provider,
    pub name: &'static str,
    pub company: &'static str,
    pub command: &'static str,
    pub args: &'static [&'static str],
    pub protocol: Protocol,
    pub login: &'static [&'static str],
    pub help: &'static str,
}

pub const VENDORS: &[Vendor] = &[
    Vendor {
        id: Provider::Claude,
        name: "Claude Code",
        company: "Anthropic",
        command: "claude",
        args: &[],
        protocol: Protocol::ClaudeStream,
        login: &["auth", "login"],
        help: "https://code.claude.com/docs/en/setup",
    },
    Vendor {
        id: Provider::Codex,
        name: "Codex",
        company: "OpenAI",
        command: "codex",
        args: &[],
        protocol: Protocol::CodexAppServer,
        login: &["login"],
        help: "https://developers.openai.com/codex/cli",
    },
    Vendor {
        id: Provider::Antigravity,
        name: "Antigravity",
        company: "Google",
        command: "agy",
        args: &["acp"],
        protocol: Protocol::Acp,
        login: &["acp", "--login"],
        help: "https://antigravity.google/docs/cli/acp/",
    },
    Vendor {
        id: Provider::Copilot,
        name: "Copilot CLI",
        company: "GitHub",
        command: "copilot",
        args: &["--acp"],
        protocol: Protocol::Acp,
        login: &[],
        help: "https://docs.github.com/en/copilot/how-tos/copilot-cli",
    },
    Vendor {
        id: Provider::Custom,
        name: "Other ACP agent",
        company: "the agent's provider",
        command: "",
        args: &[],
        protocol: Protocol::Acp,
        login: &[],
        help: "https://agentclientprotocol.com/",
    },
];
impl Provider {
    pub fn vendor(self) -> &'static Vendor {
        VENDORS
            .iter()
            .find(|v| v.id == self)
            .expect("registered provider")
    }
}

pub fn search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<_> = std::env::var_os("PATH")
        .map(|p| {
            std::env::split_paths(&p)
                .filter(|p| p.is_absolute())
                .collect()
        })
        .unwrap_or_default();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        for suffix in [
            ".local/bin",
            ".cargo/bin",
            ".npm-global/bin",
            ".antigravity/bin",
            ".local/share/mise/shims",
            ".volta/bin",
            ".nix-profile/bin",
            "bin",
        ] {
            dirs.push(home.join(suffix));
        }
        // Node version managers commonly install CLIs outside a GUI's PATH.
        if let Ok(entries) = std::fs::read_dir(home.join(".nvm/versions/node")) {
            let mut versions: Vec<_> = entries.flatten().map(|e| e.path().join("bin")).collect();
            versions.sort();
            dirs.extend(versions.into_iter().rev().take(32));
        }
    }
    dirs.extend(
        [
            "/usr/local/bin",
            "/usr/bin",
            "/opt/homebrew/bin",
            "/run/current-system/sw/bin",
        ]
        .map(PathBuf::from),
    );
    dirs
}

pub fn resolve(program: &Path, dirs: &[PathBuf]) -> Option<PathBuf> {
    let executable = |p: &Path| {
        std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    };
    if program.is_absolute() {
        return executable(program).then(|| program.to_owned());
    }
    if program.components().count() != 1 || program.as_os_str().is_empty() {
        return None;
    }
    dirs.iter().map(|d| d.join(program)).find(|p| executable(p))
}

/// Split a user-entered command, supporting quoted paths without executing it.
pub fn command_words(command: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut escape = false;
    let mut started = false;
    for ch in command.chars() {
        if escape {
            word.push(ch);
            escape = false;
            started = true;
        } else if ch == '\\' && quote != Some('\'') {
            escape = true;
            started = true;
        } else if quote == Some(ch) {
            quote = None;
        } else if quote.is_none() && (ch == '\'' || ch == '"') {
            quote = Some(ch);
            started = true;
        } else if quote.is_none() && ch.is_whitespace() {
            if started {
                words.push(std::mem::take(&mut word));
                started = false;
            }
        } else {
            word.push(ch);
            started = true;
        }
    }
    if quote.is_some() || escape {
        return Err("Finish the quoted agent command".into());
    }
    if started {
        words.push(word);
    }
    if words.first().is_none_or(String::is_empty) {
        return Err(
            "Enter the agent's command in Settings → Assistant, for example `opencode acp`.".into(),
        );
    }
    Ok(words)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovery_checks_executability_and_preserves_launcher_paths() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent");
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        assert_eq!(resolve(Path::new("agent"), &[dir.path().into()]), None);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            resolve(Path::new("agent"), &[dir.path().into()]),
            Some(path.clone())
        );
        assert_eq!(resolve(&path, &[]), Some(path));
        assert_eq!(resolve(Path::new("../agent"), &[dir.path().into()]), None);
    }
    #[test]
    fn commands_allow_spaces_but_never_evaluate_shell_syntax() {
        assert_eq!(
            command_words("'/opt/my agent' acp --name \"a b\"").unwrap(),
            ["/opt/my agent", "acp", "--name", "a b"]
        );
        assert_eq!(
            command_words("agent $(touch /tmp/nope)").unwrap()[1],
            "$(touch"
        );
        assert!(command_words("'unfinished").is_err());
    }
}
