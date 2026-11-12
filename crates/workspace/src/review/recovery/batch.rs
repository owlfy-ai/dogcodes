//! Batch working-file recovery. Each member retains its normal guarded backup;
//! a filesystem batch can partially complete, so return exact completed paths.
use super::{ReviewResult, ReviewStore, transaction};
use crate::review::{FileChange, FileKind, validate_relative};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::Path};

#[derive(Default, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileBatchResult {
    pub completed: Vec<String>,
    pub error: Option<String>,
}

impl ReviewStore {
    pub fn discard_changes(
        &self,
        task: &str,
        root: &Path,
        changes: &[FileChange],
    ) -> ReviewResult<FileBatchResult> {
        if changes.is_empty() || changes.len() > 2000 {
            return Err("backendText.m7b62ad72".into());
        }
        let root = root
            .canonicalize()
            .map_err(|_| "backendText.workspaceUnavailable")?;
        let root_text = root.to_str().ok_or("backendText.workspaceEncoding")?;
        let mut paths = BTreeSet::new();
        for change in changes {
            for path in std::iter::once(&change.path).chain(change.previous_path.iter()) {
                validate_relative(path)?;
                if !paths.insert(path.clone()) {
                    return Err("backendText.mc1b26d4c".into());
                }
            }
            if change
                .before
                .iter()
                .chain(change.after.iter())
                .any(|version| version.kind == FileKind::Gitlink)
            {
                return Err("backendText.m30d0802c".into());
            }
        }
        for path in &paths {
            if paths
                .iter()
                .any(|other| path != other && path.starts_with(&format!("{other}/")))
            {
                return Err("backendText.mf553a852".into());
            }
        }
        let check = || -> ReviewResult<()> {
            for change in changes {
                if transaction::current_version(self, root_text, &change.path)? != change.after {
                    return Err(format!(
                        "backendText.m72c22b6c\u{1e}{}",
                        serde_json::to_string(std::slice::from_ref(&change.path))
                            .unwrap_or_else(|_| "[]".into())
                    ));
                }
                if let Some(previous) = &change.previous_path
                    && transaction::current_version(self, root_text, previous)?.is_some()
                {
                    return Err(format!(
                        "backendText.md48e9a88\u{1e}{}",
                        serde_json::to_string(std::slice::from_ref(previous))
                            .unwrap_or_else(|_| "[]".into())
                    ));
                }
            }
            Ok(())
        };
        check()?;
        // Prepare every backup and validate every replacement before touching files.
        let records = changes
            .iter()
            .map(|change| {
                let grouped = change.previous_path.is_some();
                let id = if grouped {
                    self.prepare_rename_recovery(task, &root, change)?.id
                } else {
                    self.prepare_recovery(
                        task,
                        &root,
                        &change.path,
                        change.after.clone(),
                        change.before.clone(),
                    )?
                    .id
                };
                Ok((change.path.clone(), id, grouped))
            })
            .collect::<ReviewResult<Vec<_>>>()?;
        check()?;
        let mut result = FileBatchResult::default();
        for (path, id, grouped) in records {
            let outcome = if grouped {
                self.apply_recovery_group(task, &id)
            } else {
                self.apply_recovery(task, &id)
            };
            if let Err(error) = outcome {
                result.error = Some(format!(
                    "backendText.m01b66617\u{1e}{}",
                    serde_json::to_string(&[path.clone(), error.clone(), id.clone()])
                        .unwrap_or_else(|_| "[]".into())
                ));
                break;
            }
            result.completed.push(path);
        }
        Ok(result)
    }
}
