use agent_core::Core;
use agent_domain::TaskId;
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    path::{Path, PathBuf},
};

pub async fn project_paths(core: &Core, task_id: &str) -> Result<Vec<PathBuf>, String> {
    let id = TaskId::parse(task_id).map_err(|e| e.to_string())?;
    let snapshot = core.bootstrap().await.map_err(|e| e.to_string())?;
    let task = snapshot
        .tasks
        .iter()
        .find(|t| t.task_id == id)
        .ok_or("任务不存在")?;
    let project = snapshot
        .projects
        .iter()
        .find(|p| p.project_id == task.project_id)
        .ok_or("项目不存在")?;
    Ok(std::iter::once(project.path.clone())
        .chain(project.additional_directories.iter().cloned())
        .map(PathBuf::from)
        .collect())
}

pub(super) fn resolve(base: &Path, path: &str) -> Result<PathBuf, String> {
    if Path::new(path)
        .components()
        .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err("无效路径".into());
    }
    let base = base.canonicalize().map_err(|e| e.to_string())?;
    let target = base.join(path).canonicalize().map_err(|e| e.to_string())?;
    if !target.starts_with(base) {
        return Err("文件位于项目目录之外".into());
    }
    Ok(target)
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Entry {
    name: String,
    path: String,
    directory: bool,
}
fn list_directory(base: &Path, path: &str) -> Result<Vec<Entry>, String> {
    let target = resolve(base, path)?;
    if !target.is_dir() {
        return Err("不是目录".into());
    }
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(&target).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        // Do not follow links outside the workspace or recurse through cycles.
        if !kind.is_dir() && !kind.is_file() && !kind.is_symlink() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let relative = if path.is_empty() {
            name.clone()
        } else {
            format!("{path}/{name}")
        };
        entries.push(Entry {
            name,
            path: relative,
            directory: kind.is_dir(),
        });
        if entries.len() > 20000 {
            return Err("此目录文件过多，暂无法展开".into());
        }
    }
    entries.sort_by(|a, b| {
        b.directory
            .cmp(&a.directory)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(entries)
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Preview {
    pub text: Option<String>,
    pub image: Option<String>,
}
fn read_preview(base: &Path, path: &str) -> Result<Preview, String> {
    let target = resolve(base, path)?;
    if !target.is_file() {
        return Err("不是普通文件".into());
    }
    let mut bytes = Vec::new();
    std::fs::File::open(target)
        .map_err(|e| e.to_string())?
        .take(20 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    preview_bytes(bytes, path)
}

pub(crate) fn preview_bytes(bytes: Vec<u8>, path: &str) -> Result<Preview, String> {
    if bytes.len() > 20 * 1024 * 1024 {
        return Err("文件超过 20 MB，暂不支持预览".into());
    }
    let image = crate::review::image_data_url(&bytes, path);
    let text = if image.is_none() && !bytes.contains(&0) {
        if bytes.len() > 1024 * 1024 {
            return Err("文本超过 1 MB，暂不支持预览".into());
        }
        String::from_utf8(bytes).ok()
    } else {
        None
    };
    Ok(Preview { text, image })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lists_ignored_and_hidden_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        std::fs::create_dir(dir.path().join("empty")).unwrap();
        std::fs::write(dir.path().join(".gitignore"), "ignored.txt\n").unwrap();
        std::fs::write(dir.path().join("ignored.txt"), "hi").unwrap();
        let entries = list_directory(dir.path(), "").unwrap();
        for name in [".git", ".gitignore", "ignored.txt", "empty"] {
            assert!(entries.iter().any(|entry| entry.name == name));
        }
        assert!(entries[0].directory);
        assert!(list_directory(dir.path(), "empty").unwrap().is_empty());
    }
    #[test]
    fn confines_preview_to_project() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hi").unwrap();
        assert!(resolve(dir.path(), "a.txt").is_ok());
        assert!(resolve(dir.path(), "").is_ok());
        assert!(resolve(dir.path(), "../a.txt").is_err());
        assert!(resolve(dir.path(), "/etc/passwd").is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc/passwd", dir.path().join("escape")).unwrap();
            assert!(resolve(dir.path(), "escape").is_err());
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct FileMatch {
    root: usize,
    path: String,
}
#[derive(Debug, Deserialize, Serialize)]
pub struct FileMatches {
    files: Vec<FileMatch>,
    partial: bool,
}

#[derive(serde::Deserialize)]
struct NoiseRules {
    directories: Vec<String>,
    files: Vec<String>,
    suffixes: Vec<String>,
}
fn is_noise_name(name: &str) -> bool {
    static RULES: std::sync::LazyLock<NoiseRules> = std::sync::LazyLock::new(|| {
        serde_json::from_str(include_str!("../../src/workspaceNoise.json"))
            .expect("valid bundled workspace filter rules")
    });
    let name = name.to_lowercase();
    name.starts_with('.')
        || RULES.directories.contains(&name)
        || RULES.files.contains(&name)
        || RULES.suffixes.iter().any(|suffix| name.ends_with(suffix))
}

fn find_files(roots: &[PathBuf], query: &str, hide_noise: bool) -> Result<FileMatches, String> {
    let query = query.trim().replace('\\', "/").to_lowercase();
    let mut result = FileMatches {
        files: Vec::new(),
        partial: false,
    };
    if query.is_empty() {
        return Ok(result);
    }
    let pattern = if query.contains(['*', '?', '[', '{']) {
        let pattern = if query.contains('/') {
            query.clone()
        } else {
            format!("**/{query}")
        };
        Some(
            globset::GlobBuilder::new(&pattern)
                .literal_separator(true)
                .backslash_escape(false)
                .build()
                .map_err(|_| "通配符格式不正确，请检查括号是否配对".to_owned())?
                .compile_matcher(),
        )
    } else {
        None
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let mut visited = 0;
    for (root, base) in roots.iter().enumerate() {
        let mut pending = vec![base.clone()];
        while let Some(directory) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(&directory) else {
                result.partial = true;
                continue;
            };
            for entry in entries {
                visited += 1;
                if visited > 200_000
                    || std::time::Instant::now() >= deadline
                    || result.files.len() >= 2000
                {
                    result.partial = true;
                    return Ok(result);
                }
                let Ok(entry) = entry else {
                    result.partial = true;
                    continue;
                };
                let Ok(kind) = entry.file_type() else {
                    result.partial = true;
                    continue;
                };
                if hide_noise && is_noise_name(&entry.file_name().to_string_lossy()) {
                    continue;
                }
                if kind.is_dir() {
                    pending.push(entry.path());
                } else if kind.is_file() || kind.is_symlink() {
                    let path = entry
                        .path()
                        .strip_prefix(base)
                        .unwrap_or(entry.path().as_path())
                        .to_string_lossy()
                        .replace('\\', "/");
                    if pattern.as_ref().map_or_else(
                        || path.to_lowercase().contains(&query),
                        |pattern| pattern.is_match(path.to_lowercase()),
                    ) {
                        result.files.push(FileMatch { root, path });
                    }
                }
            }
        }
    }
    result
        .files
        .sort_by(|a, b| a.root.cmp(&b.root).then_with(|| a.path.cmp(&b.path)));
    Ok(result)
}

#[cfg(test)]
mod filter_tests {
    #[test]
    fn noise_filter_prunes_dependencies_and_hidden_paths_but_preserves_source() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for folder in [
            "node_modules",
            ".git",
            ".pnpm-store",
            ".claude",
            ".wrangler",
            "dist",
            "src/node_modules",
            "src",
        ] {
            std::fs::create_dir_all(root.join(folder)).unwrap();
            std::fs::write(root.join(folder).join("index.ts"), "code").unwrap();
        }
        for file in [
            ".env",
            "package.json",
            "pnpm-lock.yaml",
            "README.md",
            "cache.pyc",
        ] {
            std::fs::write(root.join(file), "content").unwrap();
        }
        let roots = vec![root.to_owned()];
        let all = super::find_files(&roots, "*", false).unwrap();
        let clean = super::find_files(&roots, "*", true).unwrap();
        let paths: Vec<_> = clean.files.iter().map(|file| file.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "README.md",
                "package.json",
                "pnpm-lock.yaml",
                "src/index.ts"
            ]
        );
        assert!(!clean.partial);
        assert!(all.files.len() > clean.files.len());
        assert!(
            super::find_files(&roots, ".git/", true)
                .unwrap()
                .files
                .is_empty()
        );
        assert_eq!(
            super::find_files(&roots, ".git/", false)
                .unwrap()
                .files
                .len(),
            1
        );
    }

    #[test]
    fn finds_collapsed_hidden_and_multi_root_files_by_path() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        std::fs::create_dir(first.path().join(".hidden")).unwrap();
        std::fs::write(first.path().join(".hidden/Readme.md"), "").unwrap();
        std::fs::write(second.path().join("README.md"), "").unwrap();
        let roots = vec![first.path().to_owned(), second.path().to_owned()];
        assert_eq!(
            super::find_files(&roots, "readme", false)
                .unwrap()
                .files
                .len(),
            2
        );
        assert_eq!(
            super::find_files(&roots, ".hidden/", false)
                .unwrap()
                .files
                .len(),
            1
        );
        assert!(
            super::find_files(&roots, "missing", false)
                .unwrap()
                .files
                .is_empty()
        );
    }
}

#[cfg(test)]
mod wildcard_tests {
    #[test]
    fn supports_filename_and_path_patterns() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/nested")).unwrap();
        for path in [
            "README.MD",
            "src/a.md",
            "src/nested/b.md",
            "src/a.ts",
            "src/b.js",
            "src/ab.ts",
        ] {
            std::fs::write(dir.path().join(path), "").unwrap();
        }
        let roots = vec![dir.path().to_owned()];
        for (query, count) in [
            ("*.md", 3),
            ("src/*.md", 1),
            ("src/**/*.md", 2),
            ("?.ts", 1),
            ("[ab].*", 4),
            ("*.{ts,js}", 3),
            ("readme", 1),
        ] {
            assert_eq!(
                super::find_files(&roots, query, false).unwrap().files.len(),
                count,
                "{query}"
            );
        }
        assert!(super::find_files(&roots, "[abc", false).is_err());
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DirectoryRequest {
    pub task_id: String,
    pub root: usize,
    pub path: String,
}

pub async fn directory_base(core: &Core, request: &DirectoryRequest) -> Result<PathBuf, String> {
    project_paths(core, &request.task_id)
        .await?
        .get(request.root)
        .cloned()
        .ok_or("目录不存在".into())
}
pub async fn dispatch(
    core: &Core,
    method: &str,
    args: serde_json::Value,
) -> Result<serde_json::Value, String> {
    use serde_json::{from_value, json};
    match method {
        "workspace_roots" => {
            let task: String = from_value(args).map_err(|e| e.to_string())?;
            Ok(json!(project_paths(core, &task).await?))
        }
        "workspace_directory" | "workspace_preview" => {
            let request: DirectoryRequest = from_value(args).map_err(|e| e.to_string())?;
            let base = directory_base(core, &request).await?;
            let preview = method == "workspace_preview";
            tokio::task::spawn_blocking(move || {
                if preview {
                    Ok(json!(read_preview(&base, &request.path)?))
                } else {
                    Ok(json!(list_directory(&base, &request.path)?))
                }
            })
            .await
            .map_err(|e| e.to_string())?
        }
        "workspace_find_files" => {
            let (task, query, hide_noise): (String, String, bool) =
                from_value(args).map_err(|e| e.to_string())?;
            let roots = project_paths(core, &task).await?;
            tokio::task::spawn_blocking(move || Ok(json!(find_files(&roots, &query, hide_noise)?)))
                .await
                .map_err(|e| e.to_string())?
        }
        _ => Err("unknown workspace operation".into()),
    }
}
pub mod watch;
