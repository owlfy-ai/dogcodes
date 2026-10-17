//! Compare frozen candidates in an isolated directory. Never stage user files
//! just to ask Git to recognize an untracked rename destination.
use super::{BTreeMap, FileChange, FileKind, GitReview, MAX_BYTES, ReviewResult, ReviewStore, fs};

impl GitReview {
    pub(super) async fn pair_working_renames(
        &self,
        changes: &mut Vec<FileChange>,
        store: &ReviewStore,
    ) -> ReviewResult<()> {
        use crate::review::ChangeKind;
        if !changes.iter().any(|c| c.kind == ChangeKind::Added)
            || !changes.iter().any(|c| c.kind == ChangeKind::Deleted)
        {
            return Ok(());
        }
        let temporary = tempfile::tempdir().map_err(|_| "cannot allocate rename comparison")?;
        let mut total = 0_u64;
        for side in ["before", "after"] {
            fs::create_dir(temporary.path().join(side))
                .map_err(|_| "cannot prepare rename comparison")?;
        }
        for (index, change) in changes.iter().enumerate() {
            let (side, version) = match change.kind {
                ChangeKind::Added => ("after", change.after.as_ref()),
                ChangeKind::Deleted => ("before", change.before.as_ref()),
                _ => continue,
            };
            let Some(version) = version.filter(|v| v.kind == FileKind::File) else {
                continue;
            };
            total += version.bytes;
            if total > 128 * 1024 * 1024 {
                return Err("rename comparison content budget exceeded".into());
            }
            fs::write(
                temporary.path().join(side).join(index.to_string()),
                store.read_blob(&version.hash)?,
            )
            .map_err(|_| "cannot materialize frozen rename candidate")?;
        }
        let env = BTreeMap::from([
            ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
            (
                "GIT_CONFIG_GLOBAL".into(),
                if cfg!(windows) { "NUL" } else { "/dev/null" }.into(),
            ),
        ]);
        let output = self
            .provider
            .git_process(
                temporary.path(),
                &[
                    "diff",
                    "--no-index",
                    "--name-status",
                    "-z",
                    "--find-renames=50%",
                    "-l2000",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--",
                    "before",
                    "after",
                ],
                env,
                MAX_BYTES,
            )
            .await
            .map_err(|_| "frozen rename comparison failed")?;
        if !matches!(output.exit_code, Some(0 | 1)) {
            return Err("frozen rename comparison failed".into());
        }
        let mut fields = output.stdout.split(|b| *b == 0).filter(|f| !f.is_empty());
        let mut removed = std::collections::BTreeSet::new();
        while let Some(status) = fields.next() {
            let source = fields.next().ok_or("missing rename candidate")?;
            if !status.starts_with(b"R") {
                continue;
            }
            let destination = fields.next().ok_or("missing rename destination")?;
            // Ambiguous exact copies deliberately remain independent entries;
            // unique exact pairs were already resolved before this method.
            if status == b"R100" {
                continue;
            }
            let parse = |value: &[u8], prefix: &str| -> ReviewResult<usize> {
                std::str::from_utf8(value)
                    .ok()
                    .and_then(|s| s.strip_prefix(prefix))
                    .and_then(|s| s.parse::<usize>().ok())
                    .filter(|i| *i < changes.len())
                    .ok_or_else(|| "invalid rename candidate identity".into())
            };
            let old = parse(source, "before/")?;
            let new = parse(destination, "after/")?;
            if changes[old].kind != ChangeKind::Deleted || changes[new].kind != ChangeKind::Added {
                return Err("unexpected rename candidate pairing".into());
            }
            changes[new].previous_path = Some(changes[old].path.clone());
            let original = changes[old].before.clone();
            changes[new].before = original;
            changes[new].kind = ChangeKind::Renamed;
            removed.insert(old);
        }
        let mut index = 0;
        changes.retain(|_| {
            let keep = !removed.contains(&index);
            index += 1;
            keep
        });
        Ok(())
    }
}
