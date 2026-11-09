//! Transient visual context for the person watching a computer-use turn.
//!
//! Previews are display-only: never persisted, never returned to the model,
//! and captured with the driver's `display_only` mode so the agent's own
//! snapshot, element tokens and coordinate frame stay untouched.
use agent_domain::{TaskId, TurnId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};

/// Longest edge of a preview capture; small enough to stream every action.
pub(super) const PREVIEW_DIMENSION: u64 = 960;
/// Reuse an image the agent already received only when it is this small.
const MAX_REUSED_IMAGE: usize = 3 * 1024 * 1024;
const MAX_SNAPSHOTS: usize = 8;
const MAX_CAPTURES: usize = 32;
const MAX_WINDOWS: usize = 8;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct PreviewMarker {
    /// Fractions of the image width/height.
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComputerPreview {
    /// The `computer_use` timeline item this preview belongs to.
    pub item_id: String,
    pub task_id: TaskId,
    /// The turn that made the call; viewers drop the preview once it ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    /// `data:` URL of the window after the action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marker: Option<PreviewMarker>,
    /// "App · window title" of the captured window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<String>,
    /// Accessibility description of the element acted on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element: Option<String>,
    /// Windows a `list_windows` call returned.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub windows: Vec<String>,
}

type WindowKey = (i64, i64);

struct Snapshot {
    id: String,
    window: WindowKey,
    rows: HashMap<u64, String>,
}

/// What the agent has seen so far, so later actions can be described.
#[derive(Default)]
pub(super) struct Grounding {
    snapshots: VecDeque<Snapshot>,
    /// Pixel size of the agent's most recent screenshot of each window.
    captures: VecDeque<(WindowKey, (f64, f64))>,
}

/// A preview still waiting for its capture.
#[derive(Debug, PartialEq)]
pub(super) struct PreviewJob {
    pub preview: ComputerPreview,
    /// Window to capture when no reusable image came back.
    pub capture: Option<WindowKey>,
    /// Marker in the agent's screenshot pixels, with that screenshot's size.
    marker_px: Option<((f64, f64), (f64, f64))>,
}

impl PreviewJob {
    pub fn capture_args(window: WindowKey) -> Value {
        json!({"pid":window.0,"window_id":window.1,"include_accessibility_tree":false,
            "display_only":true,"max_dimension":PREVIEW_DIMENSION})
    }

    /// Fill in the capture result; returns the finished preview.
    pub fn finish(mut self, capture: Option<&Value>) -> ComputerPreview {
        if let Some(capture) = capture {
            if self.preview.image.is_none() {
                self.preview.image = image_url(capture, usize::MAX);
            }
            if self.preview.window.is_none() {
                self.preview.window = window_label(&capture["structuredContent"]);
            }
        }
        if self.preview.image.is_some()
            && let Some(((x, y), (width, height))) = self.marker_px
            && width > 0.0
            && height > 0.0
            && (0.0..=width).contains(&x)
            && (0.0..=height).contains(&y)
        {
            self.preview.marker = Some(PreviewMarker {
                x: x / width,
                y: y / height,
            });
        }
        self.preview
    }
}

fn int(value: &Value) -> Option<i64> {
    value.as_i64()
}

fn image_url(result: &Value, limit: usize) -> Option<String> {
    let image = result["content"]
        .as_array()?
        .iter()
        .find(|block| block["type"] == "image")?;
    let mime = image["mimeType"].as_str()?;
    let data = image["data"].as_str()?;
    if !mime.starts_with("image/") || data.is_empty() || data.len() > limit {
        return None;
    }
    Some(format!("data:{mime};base64,{data}"))
}

fn window_label(info: &Value) -> Option<String> {
    let app = info["app_name"].as_str().filter(|s| !s.is_empty());
    let title = info["window_title"]
        .as_str()
        .or_else(|| info["title"].as_str())
        .filter(|s| !s.is_empty());
    match (app, title) {
        (Some(app), Some(title)) if app != title => Some(format!("{app} · {title}")),
        (Some(name), _) | (None, Some(name)) => Some(name.to_owned()),
        (None, None) => None,
    }
}

/// `- [12] AXButton "Save" [id=x actions=[press]]` → (12, `Button "Save"`).
fn parse_row(line: &str) -> Option<(u64, String)> {
    let line = line.trim_start().trim_start_matches(['-', '+', '~', ' ']);
    let rest = line.strip_prefix('[')?;
    let (index, rest) = rest.split_once("] ")?;
    let index = index.parse().ok()?;
    let mut text = rest;
    // Drop trailing `[key=value …]` attribute groups.
    while let Some(position) = text.rfind(" [") {
        let tail = &text[position + 2..];
        let is_attributes = tail.ends_with(']')
            && tail.split_once('=').is_some_and(|(key, _)| {
                !key.is_empty() && key.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
            });
        if !is_attributes {
            break;
        }
        text = &text[..position];
    }
    let text = text.trim().strip_prefix("AX").unwrap_or(text.trim());
    (!text.is_empty()).then(|| (index, text.chars().take(120).collect()))
}

fn element_label(element: &Value) -> Option<String> {
    let role = element["role"].as_str().unwrap_or("");
    let role = role.strip_prefix("AX").unwrap_or(role);
    let label = element["label"]
        .as_str()
        .or_else(|| element["value"].as_str())
        .filter(|s| !s.is_empty());
    let text = match label {
        Some(label) => format!("{role} \"{label}\""),
        None => role.to_owned(),
    };
    let text = text.trim().to_owned();
    (!text.is_empty()).then(|| text.chars().take(120).collect())
}

fn window_target(args: &Value) -> Option<WindowKey> {
    let target = &args["target"];
    if target["kind"] == "window"
        && let (Some(pid), Some(window)) = (int(&target["pid"]), int(&target["window_id"]))
    {
        return Some((pid, window));
    }
    Some((int(&args["pid"])?, int(&args["window_id"])?))
}

impl Grounding {
    fn snapshot(&self, id: &str) -> Option<&Snapshot> {
        self.snapshots.iter().rev().find(|s| s.id == id)
    }

    fn latest_window_of(&self, pid: i64) -> Option<WindowKey> {
        self.snapshots
            .iter()
            .rev()
            .map(|s| s.window)
            .chain(self.captures.iter().rev().map(|(window, _)| *window))
            .find(|window| window.0 == pid)
    }

    fn capture_size(&self, window: WindowKey) -> Option<(f64, f64)> {
        self.captures
            .iter()
            .rev()
            .find(|(key, _)| *key == window)
            .map(|(_, size)| *size)
    }

    /// Remember what an agent observation showed.
    fn observe(&mut self, args: &Value, result: &Value) {
        let info = &result["structuredContent"];
        if info["display_only"] == true {
            return;
        }
        let Some(window) = (match (int(&info["pid"]), int(&info["window_id"])) {
            (Some(pid), Some(window)) => Some((pid, window)),
            _ => window_target(args),
        }) else {
            return;
        };
        if let (Some(width), Some(height)) = (
            info["screenshot_width"].as_f64(),
            info["screenshot_height"].as_f64(),
        ) && image_url(result, usize::MAX).is_some()
        {
            self.captures.retain(|(key, _)| *key != window);
            self.captures.push_back((window, (width, height)));
            while self.captures.len() > MAX_CAPTURES {
                self.captures.pop_front();
            }
        }
        let Some(id) = info["snapshot_id"].as_str() else {
            return;
        };
        // Diff reads list only changed rows; start from the window's last rows.
        let mut rows = self
            .snapshots
            .iter()
            .rev()
            .find(|s| s.window == window)
            .map(|s| s.rows.clone())
            .unwrap_or_default();
        if let Some(markdown) = info["tree_markdown"].as_str() {
            rows.extend(markdown.lines().filter_map(parse_row));
        }
        for element in info["elements"].as_array().into_iter().flatten() {
            if let (Some(index), Some(label)) =
                (element["element_index"].as_u64(), element_label(element))
            {
                rows.insert(index, label);
            }
        }
        self.snapshots.retain(|s| s.id != id);
        self.snapshots.push_back(Snapshot {
            id: id.to_owned(),
            window,
            rows,
        });
        while self.snapshots.len() > MAX_SNAPSHOTS {
            self.snapshots.pop_front();
        }
    }

    /// Window, element and pixel point an action addressed, if known.
    fn resolve(&self, args: &Value) -> (Option<WindowKey>, Option<String>, Option<(f64, f64)>) {
        let mut window = window_target(args);
        let mut element = None;
        if let Some((snapshot, row)) = args["element_token"]
            .as_str()
            .and_then(|token| token.split_once(':'))
            && let Some(snapshot) = self.snapshot(snapshot)
        {
            window = window.or(Some(snapshot.window));
            element = row
                .parse::<u64>()
                .ok()
                .and_then(|row| snapshot.rows.get(&row).cloned());
        }
        if window.is_none()
            && let Some(pid) = int(&args["pid"])
        {
            window = self.latest_window_of(pid);
        }
        let point = (args["element_token"].is_null()
            && args["from_zoom"] != true
            && args["scope"] != "desktop")
            .then(|| Some((args["x"].as_f64()?, args["y"].as_f64()?)))
            .flatten();
        (window, element, point)
    }

    /// Record the call and decide what, if anything, to show for it.
    pub fn plan(
        &mut self,
        item_id: String,
        task_id: TaskId,
        tool: &str,
        args: &Value,
        result: &Value,
    ) -> Option<PreviewJob> {
        if matches!(tool, "get_window_state" | "get_desktop_state") {
            self.observe(args, result);
        }
        let mut job = PreviewJob {
            preview: ComputerPreview {
                item_id,
                task_id,
                turn_id: None,
                image: None,
                marker: None,
                window: None,
                element: None,
                windows: vec![],
            },
            capture: None,
            marker_px: None,
        };
        let info = &result["structuredContent"];
        match tool {
            "list_apps" => return None,
            "list_windows" => {
                job.preview.windows = info["windows"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(window_label)
                    .take(MAX_WINDOWS)
                    .collect();
                return (!job.preview.windows.is_empty()).then_some(job);
            }
            // These return their own picture of exactly what the agent saw.
            "zoom" | "get_desktop_state" => {
                job.preview.image = image_url(result, MAX_REUSED_IMAGE);
                job.preview.window = window_label(info);
                return job.preview.image.is_some().then_some(job);
            }
            _ => {}
        }
        // A batch is described by its last step that names a target.
        let addressed = if tool == "run_steps" {
            args["steps"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|step| {
                    step.get("args")
                        .or_else(|| step.as_object()?.values().find(|v| v.is_object()))
                        .cloned()
                        .unwrap_or(Value::Null)
                })
                .map(|step| self.resolve(&step))
                .rfind(|(window, element, point)| {
                    window.is_some() || element.is_some() || point.is_some()
                })
                .unwrap_or((None, None, None))
        } else {
            self.resolve(args)
        };
        let (mut window, element, point) = addressed;
        if window.is_none()
            && let (Some(pid), Some(id)) = (int(&info["pid"]), int(&info["window_id"]))
        {
            window = Some((pid, id));
        }
        let window = window?;
        job.preview.element = element;
        job.marker_px = point.zip(self.capture_size(window));
        if tool == "get_window_state" {
            job.preview.image = image_url(result, MAX_REUSED_IMAGE);
            job.preview.window = window_label(info);
        }
        if job.preview.image.is_none() {
            job.capture = Some(window);
        }
        Some(job)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task() -> TaskId {
        TaskId::new()
    }
    fn png() -> Value {
        json!({"type":"image","mimeType":"image/png","data":"iVBORw0KGgo="})
    }

    #[test]
    fn rows_drop_attribute_groups_but_keep_labels() {
        assert_eq!(
            parse_row(r#"    - [12] AXButton "Save [draft]" [id=x actions=[press]]"#),
            Some((12, r#"Button "Save [draft]""#.into()))
        );
        assert_eq!(
            parse_row("+ [3] AXCell [actions=[open]]"),
            Some((3, "Cell".into()))
        );
        assert_eq!(parse_row("  - AXStaticText = \"Recents\""), None);
    }

    #[test]
    fn element_clicks_name_the_element_and_capture_its_window() {
        let mut grounding = Grounding::default();
        let read = json!({"content":[{"type":"text","text":"tree"}],"structuredContent":{"pid":683,"window_id":7418,"snapshot_id":"s00000001","app_name":"Finder","window_title":"test","tree_markdown":"- [0] AXWindow \"test\" [id=FinderWindow actions=[raise]]\n  - [3] AXCell [actions=[open]]\n    - [4] AXButton \"Recents\" [actions=[press]]"}});
        let args = json!({"pid":683,"window_id":7418,"include_screenshot":false});
        let job = grounding
            .plan("a".into(), task(), "get_window_state", &args, &read)
            .unwrap();
        assert_eq!(job.capture, Some((683, 7418)));
        assert_eq!(job.preview.window.as_deref(), Some("Finder · test"));

        let click = json!({"element_token":"s00000001:4"});
        let job = grounding
            .plan("b".into(), task(), "click", &click, &json!({"content":[]}))
            .unwrap();
        assert_eq!(job.capture, Some((683, 7418)));
        assert_eq!(job.preview.element.as_deref(), Some("Button \"Recents\""));
        assert_eq!(job.marker_px, None);
        assert_eq!(PreviewJob::capture_args((683, 7418))["display_only"], true);
    }

    #[test]
    fn pixel_clicks_mark_the_point_in_the_agents_screenshot_frame() {
        let mut grounding = Grounding::default();
        let read = json!({"content":[png()],"structuredContent":{"pid":7,"window_id":9,"snapshot_id":"s00000002","screenshot_width":1000,"screenshot_height":500}});
        let job = grounding
            .plan(
                "a".into(),
                task(),
                "get_window_state",
                &json!({"pid":7,"window_id":9}),
                &read,
            )
            .unwrap();
        // The agent already got a picture: reuse it, no extra capture.
        assert_eq!(job.capture, None);
        assert!(
            job.preview
                .image
                .unwrap()
                .starts_with("data:image/png;base64,")
        );

        let click = json!({"pid":7,"window_id":9,"x":250,"y":400});
        let job = grounding
            .plan("b".into(), task(), "click", &click, &json!({}))
            .unwrap();
        let capture = json!({"content":[png()],"structuredContent":{"app_name":"Ledger","window_title":"Invoices"}});
        let preview = job.finish(Some(&capture));
        assert_eq!(preview.marker, Some(PreviewMarker { x: 0.25, y: 0.8 }));
        assert_eq!(preview.window.as_deref(), Some("Ledger · Invoices"));

        // Zoom-relative and desktop coordinates are not window pixels.
        let zoomed = json!({"pid":7,"window_id":9,"x":250,"y":400,"from_zoom":true});
        let job = grounding
            .plan("c".into(), task(), "click", &zoomed, &json!({}))
            .unwrap();
        assert_eq!(job.finish(Some(&capture)).marker, None);
    }

    #[test]
    fn display_only_captures_never_replace_the_agents_frame() {
        let mut grounding = Grounding::default();
        let agent = json!({"content":[png()],"structuredContent":{"pid":7,"window_id":9,"screenshot_width":1000,"screenshot_height":500}});
        grounding.observe(&json!({}), &agent);
        let preview = json!({"content":[png()],"structuredContent":{"pid":7,"window_id":9,"display_only":true,"screenshot_width":960,"screenshot_height":480}});
        grounding.observe(&json!({}), &preview);
        assert_eq!(grounding.capture_size((7, 9)), Some((1000.0, 500.0)));
    }

    #[test]
    fn listings_and_unknown_targets_are_described_without_capture() {
        let mut grounding = Grounding::default();
        let listing = json!({"structuredContent":{"windows":[{"app_name":"Finder","title":"test","pid":1,"window_id":2},{"app_name":"Ledger","title":"","pid":3,"window_id":4}]}});
        let job = grounding
            .plan("a".into(), task(), "list_windows", &json!({}), &listing)
            .unwrap();
        assert_eq!(job.preview.windows, vec!["Finder · test", "Ledger"]);
        assert_eq!(job.capture, None);
        assert!(
            grounding
                .plan("b".into(), task(), "list_apps", &json!({}), &json!({}))
                .is_none()
        );
        // No window can be determined: nothing to show.
        assert!(
            grounding
                .plan(
                    "c".into(),
                    task(),
                    "press_key",
                    &json!({"key":"return"}),
                    &json!({})
                )
                .is_none()
        );
    }

    #[test]
    fn batches_use_their_last_addressed_step() {
        let mut grounding = Grounding::default();
        let steps = json!({"steps":[{"tool":"press_key","args":{"key":"tab"}},{"click":{"pid":7,"window_id":9,"x":5,"y":5}}]});
        let job = grounding
            .plan("a".into(), task(), "run_steps", &steps, &json!({}))
            .unwrap();
        assert_eq!(job.capture, Some((7, 9)));
    }
}
