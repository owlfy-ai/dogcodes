//! Offers the hosts in the user's SSH config. Only the suggestion list is parsed
//! here: connecting passes the alias to `ssh`, so `Match` and every other directive
//! keep their real meaning. Displayed values come from `ssh -G`, not from this file.
use crate::program::SshProgram;
use agent_runtime::{ProcessSpec, run_bounded};
use serde::Serialize;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

const MAX_HOSTS: usize = 256;
const MAX_FILES: usize = 64;
const MAX_FILE_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedHost {
    pub host: String,
    pub user: Option<String>,
    pub port: Option<u16>,
}

/// Literal `Host` names from the config and its `Include`s. Wildcards and negations
/// describe groups of hosts, not something one can connect to, so they are skipped.
#[must_use]
pub fn list_hosts(config: &Path, home: &Path) -> Vec<String> {
    let mut hosts = BTreeSet::new();
    let mut seen = BTreeSet::new();
    let mut queue = vec![config.to_path_buf()];
    while let Some(file) = queue.pop() {
        if seen.len() >= MAX_FILES || !seen.insert(file.clone()) {
            continue;
        }
        let Some(text) = read_limited(&file) else {
            continue;
        };
        for line in text.lines() {
            let Some((keyword, value)) = directive(line) else {
                continue;
            };
            if keyword.eq_ignore_ascii_case("host") {
                hosts.extend(
                    words(value)
                        .into_iter()
                        .filter(|word| !word.contains(['*', '?', '!']))
                        .filter(|word| !word.starts_with('-')),
                );
            } else if keyword.eq_ignore_ascii_case("include") {
                for pattern in words(value) {
                    queue.extend(expand_include(&pattern, home));
                }
            }
        }
    }
    hosts.into_iter().take(MAX_HOSTS).collect()
}

fn read_limited(file: &Path) -> Option<String> {
    if std::fs::metadata(file).ok()?.len() > MAX_FILE_BYTES {
        return None;
    }
    let bytes = std::fs::read(file).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// `Keyword value`, `Keyword=value` and `Keyword = value` are all valid.
fn directive(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let end = line.find(|c: char| c.is_whitespace() || c == '=')?;
    let (keyword, rest) = line.split_at(end);
    let rest = rest.trim_start();
    let rest = rest.strip_prefix('=').unwrap_or(rest).trim_start();
    Some((keyword, rest))
}

fn words(value: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in value.chars() {
        match c {
            '"' => quoted = !quoted,
            '#' if !quoted && current.is_empty() => break,
            c if c.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    found.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        found.push(current);
    }
    found
}

/// Relative includes resolve against `~/.ssh`, as `ssh_config(5)` specifies for
/// user configs. Only the file name may contain wildcards here.
fn expand_include(pattern: &str, home: &Path) -> Vec<PathBuf> {
    let path = if let Some(tail) = pattern.strip_prefix("~/") {
        home.join(tail)
    } else if Path::new(pattern).is_absolute() {
        PathBuf::from(pattern)
    } else {
        home.join(".ssh").join(pattern)
    };
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return vec![];
    };
    if !name.contains(['*', '?']) {
        return vec![path];
    }
    let Some(directory) = path.parent() else {
        return vec![];
    };
    let Ok(entries) = std::fs::read_dir(directory) else {
        return vec![];
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|candidate| wildcard(name.as_bytes(), candidate.as_bytes()))
        })
        .map(|entry| entry.path())
        .collect();
    files.sort();
    files
}

fn wildcard(pattern: &[u8], text: &[u8]) -> bool {
    match (pattern.first(), text.first()) {
        (None, None) => true,
        (Some(b'*'), _) => {
            wildcard(&pattern[1..], text) || (!text.is_empty() && wildcard(pattern, &text[1..]))
        }
        (Some(b'?'), Some(_)) => wildcard(&pattern[1..], &text[1..]),
        (Some(a), Some(b)) if a == b => wildcard(&pattern[1..], &text[1..]),
        _ => false,
    }
}

/// What `ssh` itself would use for this name, shown so the person can check it.
pub async fn resolve(program: &SshProgram, alias: &str) -> Option<ResolvedHost> {
    if alias.is_empty() || alias.starts_with('-') {
        return None;
    }
    let output = run_bounded(&ProcessSpec::probe(
        program.path.clone(),
        vec!["-G".into(), "--".into(), alias.into()],
    ))
    .await
    .ok()?;
    output.success.then(|| parse_effective(&output.stdout))?
}

fn parse_effective(text: &str) -> Option<ResolvedHost> {
    let mut host = None;
    let mut user = None;
    let mut port = None;
    for line in text.lines() {
        match line.split_once(' ') {
            Some(("hostname", value)) => host = Some(value.to_owned()),
            Some(("user", value)) => user = Some(value.to_owned()),
            Some(("port", value)) => port = value.parse().ok(),
            _ => {}
        }
    }
    Some(ResolvedHost {
        host: host?,
        user,
        port,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_come_from_includes_and_skip_patterns() {
        let home = tempfile::tempdir().unwrap();
        let ssh = home.path().join(".ssh");
        std::fs::create_dir_all(ssh.join("config.d")).unwrap();
        std::fs::write(
            ssh.join("config"),
            "# comment\nInclude config.d/*.conf\nHost build prod-*\n  HostName 10.0.0.1\nhost=gpu \"with space\"\nHost *\n  ServerAliveInterval 5\nHost !bastion -oBad\nMatch host x\n",
        )
        .unwrap();
        std::fs::write(ssh.join("config.d/a.conf"), "Host included\n").unwrap();
        std::fs::write(ssh.join("config.d/b.txt"), "Host ignored\n").unwrap();
        let hosts = list_hosts(&ssh.join("config"), home.path());
        assert_eq!(hosts, ["build", "gpu", "included", "with space"]);
    }

    #[test]
    fn a_missing_config_and_an_include_cycle_are_harmless() {
        let home = tempfile::tempdir().unwrap();
        let ssh = home.path().join(".ssh");
        assert!(list_hosts(&ssh.join("config"), home.path()).is_empty());
        std::fs::create_dir_all(&ssh).unwrap();
        std::fs::write(ssh.join("config"), "Include config\nHost one\n").unwrap();
        assert_eq!(list_hosts(&ssh.join("config"), home.path()), ["one"]);
    }

    #[test]
    fn effective_values_come_from_ssh_g_output() {
        let parsed =
            parse_effective("user dev\nhostname 10.0.0.1\nport 2222\nidentityfile ~/.ssh/a\n");
        assert_eq!(
            parsed,
            Some(ResolvedHost {
                host: "10.0.0.1".into(),
                user: Some("dev".into()),
                port: Some(2222),
            })
        );
        assert_eq!(parse_effective("user dev\n"), None);
    }

    #[test]
    fn wildcards_match_whole_file_names() {
        assert!(wildcard(b"*.conf", b"a.conf"));
        assert!(wildcard(b"a?c", b"abc"));
        assert!(!wildcard(b"*.conf", b"a.txt"));
        assert!(!wildcard(b"a?c", b"ac"));
    }
}
