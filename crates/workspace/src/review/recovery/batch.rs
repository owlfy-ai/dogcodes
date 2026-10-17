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
            return Err("请选择 1 至 2000 个文件".into());
        }
        let root = root.canonicalize().map_err(|_| "工作目录不可用")?;
        let root_text = root.to_str().ok_or("工作目录编码无效")?;
        let mut paths = BTreeSet::new();
        for change in changes {
            for path in std::iter::once(&change.path).chain(change.previous_path.iter()) {
                validate_relative(path)?;
                if !paths.insert(path.clone()) {
                    return Err("选择包含重叠路径，请分开处理".into());
                }
            }
            if change
                .before
                .iter()
                .chain(change.after.iter())
                .any(|version| version.kind == FileKind::Gitlink)
            {
                return Err("子模块不支持丢弃工作文件".into());
            }
        }
        for path in &paths {
            if paths
                .iter()
                .any(|other| path != other && path.starts_with(&format!("{other}/")))
            {
                return Err("选择包含文件与目录互换，请分开处理".into());
            }
        }
        let check = || -> ReviewResult<()> {
            for change in changes {
                if transaction::current_version(self, root_text, &change.path)? != change.after {
                    return Err(format!("{} 在预览后有修改，请刷新后重试", change.path));
                }
                if let Some(previous) = &change.previous_path
                    && transaction::current_version(self, root_text, previous)?.is_some()
                {
                    return Err(format!("{previous} 在预览后有修改，请刷新后重试"));
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
                    "{path}：{error}。该文件可能已部分处理，请刷新检查；备份记录 {id}"
                ));
                break;
            }
            result.completed.push(path);
        }
        Ok(result)
    }
}
