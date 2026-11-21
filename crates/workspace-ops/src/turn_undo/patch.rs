//! Apply reported line edits at their original coordinates, without fuzz or filesystem access.
use serde_json::Value;

#[derive(Default)]
struct Hunk {
    start: usize,
    removed: usize,
    added: usize,
    lines: Vec<(u8, String)>,
}

fn apply(content: &str, hunks: Vec<Hunk>) -> Option<String> {
    if hunks.is_empty() {
        return None;
    }
    let original: Vec<_> = content.split_inclusive('\n').collect();
    let mut cursor = 0;
    let mut out = String::new();
    for h in hunks {
        let start = if h.removed == 0 {
            h.start
        } else {
            h.start.checked_sub(1)?
        };
        for line in original.get(cursor..start)? {
            out.push_str(line);
        }
        cursor = start;
        let mut added = 0;
        for (kind, text) in h.lines {
            if kind != b'+' {
                if original.get(cursor)? != &text {
                    return None;
                }
                cursor += 1;
            }
            if kind != b'-' {
                out.push_str(&text);
                added += 1;
            }
        }
        if cursor - start != h.removed || added != h.added {
            return None;
        }
    }
    for line in &original[cursor..] {
        out.push_str(line);
    }
    Some(out)
}

pub(super) fn structured(content: &str, patch: &Value) -> Option<String> {
    let mut hunks = Vec::new();
    for h in patch.as_array()? {
        let mut lines = Vec::new();
        for line in h["lines"].as_array()? {
            let line = line.as_str()?;
            let kind = *line.as_bytes().first()?;
            if !matches!(kind, b' ' | b'+' | b'-') {
                return None;
            }
            lines.push((kind, format!("{}\n", &line[1..])));
        }
        hunks.push(Hunk {
            start: usize::try_from(h["oldStart"].as_u64()?).ok()?,
            removed: usize::try_from(h["oldLines"].as_u64()?).ok()?,
            added: usize::try_from(h["newLines"].as_u64()?).ok()?,
            lines,
        });
    }
    apply(content, hunks)
}

pub(super) fn unified(content: &str, patch: &str) -> Option<String> {
    fn range(s: &str) -> Option<(usize, usize)> {
        let (start, len) = s.split_once(',').unwrap_or((s, "1"));
        Some((start.parse().ok()?, len.parse().ok()?))
    }
    let mut hunks = Vec::<Hunk>::new();
    for line in patch.split_inclusive('\n') {
        if let Some(header) = line.strip_prefix("@@ -") {
            let (old, rest) = header.split_once(" +")?;
            let (new, _) = rest.split_once(" @@")?;
            let (start, removed) = range(old)?;
            hunks.push(Hunk {
                start,
                removed,
                added: range(new)?.1,
                lines: Vec::new(),
            });
        } else if let Some(h) = hunks.last_mut() {
            if line.trim_end() == "\\ No newline at end of file" {
                let (_, text) = h.lines.last_mut()?;
                if text.pop() != Some('\n') {
                    return None;
                }
            } else {
                let kind = *line.as_bytes().first()?;
                if !matches!(kind, b' ' | b'+' | b'-') {
                    return None;
                }
                h.lines.push((kind, line[1..].to_owned()));
            }
        }
    }
    apply(content, hunks)
}
