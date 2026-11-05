//! What a project's directory shows about how to run it, read where the files are.
use serde::{Deserialize, Serialize};
use std::{io::Read, path::Path};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct InputFile {
    pub path: String,
    pub text: String,
    pub truncated: bool,
    pub executable: Option<bool>,
}
fn executable(path: &Path) -> Option<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Some(std::fs::metadata(path).ok()?.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}
fn relevant(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.starts_with("readme")
        || [
            "package.json",
            "pyproject.toml",
            "cargo.toml",
            "go.mod",
            "pom.xml",
            "build.gradle",
            "build.gradle.kts",
            "makefile",
            "justfile",
            "dockerfile",
            "compose.yml",
            "compose.yaml",
            "docker-compose.yml",
            "docker-compose.yaml",
            "procfile",
            "deno.json",
            "deno.jsonc",
            "pubspec.yaml",
            "gemfile",
            "pnpm-lock.yaml",
            "package-lock.json",
            "yarn.lock",
            "bun.lock",
            "bun.lockb",
        ]
        .contains(&name.as_str())
        || [".sh", ".ps1", ".cmd", ".bat"]
            .iter()
            .any(|suffix| name.ends_with(suffix))
}
pub fn scan(root: &Path) -> Result<Vec<InputFile>, String> {
    let root = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
    let mut queue = std::collections::VecDeque::from([(root.clone(), 0)]);
    let mut files = vec![];
    let mut budget = 96_000_usize;
    let mut visited = 0;
    while let Some((directory, depth)) = queue.pop_front() {
        let mut entries = std::fs::read_dir(&directory)
            .map_err(|e| e.to_string())?
            .flatten()
            .take(4001 - visited)
            .collect::<Vec<_>>();
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            visited += 1;
            if visited > 4000 || files.len() >= 120 || budget == 0 {
                return Ok(files);
            }
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            if kind.is_symlink() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.')
                || [
                    "node_modules",
                    "target",
                    "vendor",
                    "dist",
                    "build",
                    "coverage",
                    "venv",
                    "__pycache__",
                ]
                .contains(&name.as_str())
            {
                continue;
            }
            if kind.is_dir() && depth < 3 {
                queue.push_back((entry.path(), depth + 1));
            } else if kind.is_file() && relevant(&name) {
                let relative = entry
                    .path()
                    .strip_prefix(&root)
                    .map_err(|e| e.to_string())?
                    .to_string_lossy()
                    .replace('\\', "/");
                if name.contains("lock") && name != "Dockerfile" {
                    files.push(InputFile {
                        path: relative,
                        text: "[lockfile present]".into(),
                        truncated: true,
                        executable: None,
                    });
                    continue;
                }
                let limit = budget.min(12_000);
                let mut bytes = vec![];
                std::fs::File::open(entry.path())
                    .map_err(|e| e.to_string())?
                    .take((limit + 1) as u64)
                    .read_to_end(&mut bytes)
                    .map_err(|e| e.to_string())?;
                let truncated = bytes.len() > limit;
                bytes.truncate(limit);
                budget -= bytes.len();
                if bytes.contains(&0) {
                    continue;
                }
                let text = String::from_utf8_lossy(&bytes).into_owned();
                files.push(InputFile {
                    path: relative,
                    text,
                    truncated,
                    executable: executable(&entry.path()),
                });
            }
        }
    }
    Ok(files)
}
