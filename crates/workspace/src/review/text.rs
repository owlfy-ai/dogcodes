//! One frozen text model for inline/split views, counts and context expansion.
use super::ReviewResult;
use serde::{Deserialize, Serialize};
use similar::{Algorithm, DiffTag, capture_diff_slices_deadline};
use std::time::{Duration, Instant};

const MAX_TEXT_BYTES: usize = 8 * 1024 * 1024;
const MAX_LINES: usize = 100_000;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BlockKind {
    Equal,
    Delete,
    Insert,
    Replace,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextBlock {
    pub kind: BlockKind,
    /// Zero-based, half-open ranges into original lines, including terminators.
    pub old_start: usize,
    pub old_end: usize,
    pub new_start: usize,
    pub new_end: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextComparison {
    pub before: Vec<String>,
    pub after: Vec<String>,
    pub blocks: Vec<TextBlock>,
    pub additions: usize,
    pub deletions: usize,
}

pub fn compare(before: &str, after: &str) -> ReviewResult<TextComparison> {
    compare_with_budget(before, after, false, Duration::from_secs(2))
}

/// Git presentation treats CRLF and LF as the same line terminator. Original
/// strings and CAS objects remain untouched; this is not a checkout transform.
pub fn compare_git(before: &str, after: &str) -> ReviewResult<TextComparison> {
    compare_with_budget(before, after, true, Duration::from_secs(2))
}

fn compare_with_budget(
    before: &str,
    after: &str,
    ignore_line_endings: bool,
    budget: Duration,
) -> ReviewResult<TextComparison> {
    if before.len().saturating_add(after.len()) > MAX_TEXT_BYTES {
        return Err("文本超过 8 MiB 比较预算".into());
    }
    if before.contains('\0') || after.contains('\0') {
        return Err("二进制文件不支持文本比较".into());
    }
    let deadline = Instant::now() + budget;
    let old: Vec<_> = before.split_inclusive('\n').collect();
    let new: Vec<_> = after.split_inclusive('\n').collect();
    if old.len().saturating_add(new.len()) > MAX_LINES {
        return Err("文本超过 100000 行比较预算".into());
    }
    // Only the comparison keys are normalized. Never replace displayed content.
    // A missing final newline remains significant in Git comparisons.
    let keys = |lines: &[&str]| -> Vec<String> {
        lines
            .iter()
            .map(|line| {
                if ignore_line_endings && let Some(content) = line.strip_suffix("\r\n") {
                    return format!("{content}\n");
                }
                (*line).to_owned()
            })
            .collect()
    };
    let old_keys = keys(&old);
    let new_keys = keys(&new);
    let ops = capture_diff_slices_deadline(Algorithm::Myers, &old_keys, &new_keys, Some(deadline));
    // The library can approximate on timeout; do not present that as exact stats.
    if Instant::now() >= deadline {
        return Err("文本差异计算超时，请缩小比较范围".into());
    }
    let mut additions = 0;
    let mut deletions = 0;
    let blocks = ops
        .into_iter()
        .map(|op| {
            let old = op.old_range();
            let new = op.new_range();
            let kind = match op.tag() {
                DiffTag::Equal => BlockKind::Equal,
                DiffTag::Delete => BlockKind::Delete,
                DiffTag::Insert => BlockKind::Insert,
                DiffTag::Replace => BlockKind::Replace,
            };
            if kind != BlockKind::Equal {
                additions += new.len();
                deletions += old.len();
            }
            TextBlock {
                kind,
                old_start: old.start,
                old_end: old.end,
                new_start: new.start,
                new_end: new.end,
            }
        })
        .collect();
    Ok(TextComparison {
        before: old.into_iter().map(str::to_owned).collect(),
        after: new.into_iter().map(str::to_owned).collect(),
        blocks,
        additions,
        deletions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_lines_and_ranges_reconstruct_both_sides() {
        let before = "same\nold\n尾部";
        let after = "same\nnew\nextra\n尾部\n";
        let diff = compare(before, after).unwrap();
        assert_eq!((diff.additions, diff.deletions), (3, 2));
        let old: String = diff
            .blocks
            .iter()
            .flat_map(|b| diff.before[b.old_start..b.old_end].iter().cloned())
            .collect();
        let new: String = diff
            .blocks
            .iter()
            .flat_map(|b| diff.after[b.new_start..b.new_end].iter().cloned())
            .collect();
        assert_eq!(old, before);
        assert_eq!(new, after);
    }

    #[test]
    fn whitespace_and_final_newline_remain_changes() {
        let diff = compare(" x \r\n", "\tx\n").unwrap();
        assert_eq!((diff.additions, diff.deletions), (1, 1));
        assert_eq!(diff.before[0], " x \r\n");
        assert_eq!(diff.after[0], "\tx\n");
        let diff = compare("x", "x\n").unwrap();
        assert_eq!((diff.additions, diff.deletions), (1, 1));
        assert_eq!(compare(" x\n", "x\n").unwrap().additions, 1);
    }

    #[test]
    fn empty_add_delete_and_budgets_are_explicit() {
        assert!(compare("", "").unwrap().blocks.is_empty());
        assert_eq!(compare("", "a\nb").unwrap().additions, 2);
        assert_eq!(compare("a\nb", "").unwrap().deletions, 2);
        assert!(compare("\0", "").is_err());
        assert!(compare_with_budget("a", "b", false, Duration::ZERO).is_err());
        assert!(compare(&"\n".repeat(MAX_LINES + 1), "").is_err());
    }

    #[test]
    fn git_line_endings_preserve_bytes_and_only_count_content_changes() {
        let before = "same\nold\nlast\n";
        let after = "same\r\nnew\r\nlast\r\n";
        let diff = compare_git(before, after).unwrap();
        assert_eq!((diff.additions, diff.deletions), (1, 1));
        assert_eq!(diff.before.concat(), before);
        assert_eq!(diff.after.concat(), after);
        assert_eq!(diff.blocks[0].kind, BlockKind::Equal);
        assert_eq!(compare(before, after).unwrap().additions, 3);
        assert_eq!(compare_git("x\r\n", "x\n").unwrap().additions, 0);
        assert_eq!(compare_git("x", "x\r\n").unwrap().additions, 1);
        assert_eq!(compare_git("x\r", "x").unwrap().additions, 1);
        assert_eq!(compare_git(" x\n", "x\r\n").unwrap().additions, 1);
    }
}
