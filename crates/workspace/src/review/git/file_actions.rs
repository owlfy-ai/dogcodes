use super::{GitReview, ReviewResult, ReviewRoot, canonical_root, validate_relative};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
};

impl GitReview {
    pub async fn head_file(&self, root: &ReviewRoot, path: &str) -> ReviewResult<Vec<u8>> {
        validate_relative(path)?;
        self.run(
            Path::new(&root.path),
            &["cat-file", "blob", &format!("HEAD:{path}")],
            20 * 1024 * 1024,
        )
        .await
    }

    pub async fn ignore_file(&self, root: &ReviewRoot, path: &str) -> ReviewResult<()> {
        let result = self.ignore_files(root, &[path.to_owned()]).await?;
        result.error.map_or(Ok(()), Err)
    }

    pub async fn ignore_files(
        &self,
        root: &ReviewRoot,
        paths: &[String],
    ) -> ReviewResult<crate::review::recovery::batch::FileBatchResult> {
        if paths.is_empty() || paths.len() > 2000 {
            return Err("请选择 1 至 2000 个文件".into());
        }
        let mut groups =
            std::collections::BTreeMap::<std::path::PathBuf, Vec<(String, String)>>::new();
        for path in paths.iter().collect::<std::collections::BTreeSet<_>>() {
            validate_relative(path)?;
            if path.contains(['\n', '\r']) {
                return Err("文件名包含换行，无法写入 .gitignore".into());
            }
            let scope = root
                .scopes
                .iter()
                .filter(|scope| scope.is_empty() || path.starts_with(&format!("{scope}/")))
                .max_by_key(|scope| scope.len())
                .ok_or("文件不在当前项目目录内")?;
            let tracked = self
                .run(
                    Path::new(&root.path),
                    &[
                        "ls-files",
                        "--cached",
                        "-z",
                        "--",
                        &format!(":(literal){path}"),
                    ],
                    1024 * 1024,
                )
                .await?;
            if !tracked.is_empty() {
                return Err("文件已被 Git 跟踪，.gitignore 无法忽略已跟踪文件".into());
            }
            let base = canonical_root(Path::new(&root.path))?;
            let directory = base.join(scope).canonicalize().map_err(|e| e.to_string())?;
            if !directory.starts_with(&base) {
                return Err("忽略目录超出项目范围".into());
            }
            let relative = if scope.is_empty() {
                path.as_str()
            } else {
                &path[scope.len() + 1..]
            };
            let mut rule = String::from("/");
            for character in relative.chars() {
                if matches!(character, '*' | '?' | '[' | ']' | '!' | '#' | ' ') {
                    rule.push('\\');
                }
                rule.push(character);
            }
            groups
                .entry(directory.join(".gitignore"))
                .or_default()
                .push((path.clone(), rule));
        }
        tokio::task::spawn_blocking(move || {
            // Validate all target documents before appending the first group.
            for path in groups.keys() {
                validate_ignore_document(path)?;
            }
            let mut result = crate::review::recovery::batch::FileBatchResult::default();
            for (path, entries) in groups {
                let rules: Vec<_> = entries.iter().map(|(_, rule)| rule.as_str()).collect();
                if let Err(error) = append_ignore_rules(&path, &rules) {
                    result.error = Some(format!("{}：{error}，请刷新检查忽略规则", path.display()));
                    break;
                }
                result
                    .completed
                    .extend(entries.into_iter().map(|(path, _)| path));
            }
            Ok(result)
        })
        .await
        .map_err(|e| e.to_string())?
    }
}

#[cfg(all(test, unix))]
fn append_ignore(path: &Path, rule: &str) -> ReviewResult<()> {
    append_ignore_rules(path, &[rule])
}

fn validate_ignore_document(path: &Path) -> ReviewResult<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.is_file() {
                return Err(".gitignore 不是普通文件".into());
            }
            let bytes = crate::review::bounded_read::read(path, 1024 * 1024)?;
            std::str::from_utf8(&bytes).map_err(|_| ".gitignore 不是 UTF-8 文本")?;
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn append_ignore_rules(path: &Path, rules: &[&str]) -> ReviewResult<()> {
    let mut options = fs::OpenOptions::new();
    options.read(true).append(true).create(true);
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(
            (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK)
                .bits()
                .cast_signed(),
        );
    }
    if let Ok(metadata) = fs::symlink_metadata(path)
        && !metadata.is_file()
    {
        return Err(".gitignore 不是普通文件".into());
    }
    let mut file = options.open(path).map_err(|e| e.to_string())?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > 1024 * 1024 {
        return Err(".gitignore 不是普通文件或超过大小限制".into());
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 1024 * 1024 {
        return Err(".gitignore 超过大小限制".into());
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| ".gitignore 不是 UTF-8 文本")?;
    let rules: Vec<_> = rules
        .iter()
        .filter(|rule| !text.lines().any(|line| line == **rule))
        .copied()
        .collect();
    if rules.is_empty() {
        return Ok(());
    }
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let separator = if text.is_empty() || text.ends_with('\n') {
        ""
    } else {
        newline
    };
    let additions = format!("{separator}{}{newline}", rules.join(newline));
    if bytes.len() + additions.len() > 1024 * 1024 {
        return Err(".gitignore 超过大小限制".into());
    }
    file.write_all(additions.as_bytes())
        .map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn head_and_ignore_preserve_working_files_and_index() {
        let dir = tempfile::tempdir().unwrap();
        let git = GitReview::default();
        let path = dir.path();
        git.run(path, &["init"], 1024).await.unwrap();
        fs::write(path.join("tracked.txt"), "original").unwrap();
        git.run(path, &["add", "--", "tracked.txt"], 1024)
            .await
            .unwrap();
        git.run(
            path,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.test",
                "commit",
                "-m",
                "initial",
            ],
            4096,
        )
        .await
        .unwrap();
        fs::write(path.join("tracked.txt"), "changed").unwrap();
        let root = ReviewRoot {
            id: "r".into(),
            path: path.to_str().unwrap().into(),
            git: true,
            scopes: vec![String::new()],
        };
        assert_eq!(
            git.head_file(&root, "tracked.txt").await.unwrap(),
            b"original"
        );
        assert!(git.ignore_file(&root, "tracked.txt").await.is_err());
        fs::write(path.join("a[1]*.txt"), "new").unwrap();
        fs::write(path.join("a1x.txt"), "keep").unwrap();
        fs::write(path.join(".gitignore"), "existing").unwrap();
        git.ignore_file(&root, "a[1]*.txt").await.unwrap();
        git.ignore_file(&root, "a[1]*.txt").await.unwrap();
        assert_eq!(
            fs::read_to_string(path.join(".gitignore")).unwrap(),
            "existing\n/a\\[1\\]\\*.txt\n"
        );
        assert_eq!(
            git.run(path, &["check-ignore", "--", "a[1]*.txt"], 1024)
                .await
                .unwrap(),
            b"a[1]*.txt\n"
        );
        assert!(
            git.run(path, &["check-ignore", "--", "a1x.txt"], 1024)
                .await
                .is_err()
        );
        assert_eq!(
            fs::read_to_string(path.join("tracked.txt")).unwrap(),
            "changed"
        );
        assert!(git.head_file(&root, "../outside").await.is_err());
    }
    #[tokio::test]
    async fn batch_ignore_preflights_tracked_files_and_appends_literal_rules_once() {
        let dir = tempfile::tempdir().unwrap();
        let git = GitReview::default();
        git.run(dir.path(), &["init"], 1024).await.unwrap();
        for path in ["tracked", "a[1]*.txt", "b?.txt"] {
            fs::write(dir.path().join(path), path).unwrap();
        }
        git.run(dir.path(), &["add", "tracked"], 1024)
            .await
            .unwrap();
        let root = ReviewRoot {
            id: "r".into(),
            path: dir.path().to_str().unwrap().into(),
            git: true,
            scopes: vec![String::new()],
        };
        assert!(
            git.ignore_files(&root, &["a[1]*.txt".into(), "tracked".into()])
                .await
                .is_err()
        );
        assert!(!dir.path().join(".gitignore").exists());
        fs::write(dir.path().join(".gitignore"), "existing\r\n").unwrap();
        let index = fs::read(dir.path().join(".git/index")).unwrap();
        let paths = ["a[1]*.txt".into(), "b?.txt".into(), "a[1]*.txt".into()];
        let result = git.ignore_files(&root, &paths).await.unwrap();
        assert_eq!(result.completed.len(), 2);
        assert!(result.error.is_none());
        git.ignore_files(&root, &paths).await.unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join(".gitignore")).unwrap(),
            "existing\r\n/a\\[1\\]\\*.txt\r\n/b\\?.txt\r\n"
        );
        assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), index);
        assert_eq!(
            fs::read_to_string(dir.path().join("a[1]*.txt")).unwrap(),
            "a[1]*.txt"
        );
        assert!(
            git.run(dir.path(), &["check-ignore", "a[1]*.txt", "b?.txt"], 1024)
                .await
                .is_ok()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn batch_ignore_checks_every_scoped_target_before_writing() {
        let dir = tempfile::tempdir().unwrap();
        let git = GitReview::default();
        git.run(dir.path(), &["init"], 1024).await.unwrap();
        for path in ["one", "two"] {
            fs::create_dir(dir.path().join(path)).unwrap();
            fs::write(dir.path().join(path).join("new"), "keep").unwrap();
        }
        let root = ReviewRoot {
            id: "r".into(),
            path: dir.path().to_str().unwrap().into(),
            git: true,
            scopes: vec!["one".into(), "two".into()],
        };
        let outside = tempfile::NamedTempFile::new().unwrap();
        fs::write(outside.path(), "original").unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("two/.gitignore")).unwrap();
        assert!(
            git.ignore_files(&root, &["one/new".into(), "two/new".into()])
                .await
                .is_err()
        );
        assert!(!dir.path().join("one/.gitignore").exists());
        assert_eq!(fs::read_to_string(outside.path()).unwrap(), "original");
        fs::remove_file(dir.path().join("two/.gitignore")).unwrap();
        let result = git
            .ignore_files(&root, &["one/new".into(), "two/new".into()])
            .await
            .unwrap();
        assert_eq!(result.completed.len(), 2);
        for scope in ["one", "two"] {
            assert_eq!(
                fs::read_to_string(dir.path().join(scope).join(".gitignore")).unwrap(),
                "/new\n"
            );
        }
        assert!(!dir.path().join(".gitignore").exists());
    }

    #[cfg(unix)]
    #[test]
    fn never_appends_through_an_ignore_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("outside");
        fs::write(&target, "safe").unwrap();
        let link = dir.path().join(".gitignore");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(append_ignore(&link, "/file").is_err());
        assert_eq!(fs::read_to_string(target).unwrap(), "safe");
    }
}
