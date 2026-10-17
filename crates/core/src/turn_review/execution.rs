use super::{Core, ReviewRun, ReviewStatus};
use super::{activity, activity::ActivityOutput, workspace};
use chrono::Utc;
use driver_api::{AgentDriver, DriverEvent, PromptRequest};
use futures::StreamExt;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};
use tokio::sync::watch;

// Drivers may publish either legacy deltas or normalized presentation messages.
// Keep channels separate so an adapter emitting both cannot duplicate JSON.
#[derive(Default)]
struct ReviewOutput {
    legacy: String,
    messages: BTreeMap<String, String>,
    order: Vec<String>,
    seen: BTreeSet<String>,
}
impl ReviewOutput {
    fn report(&self) -> Option<crate::review_analysis::ReviewReport> {
        self.order
            .last()
            .and_then(|id| self.messages.get(id))
            .and_then(|text| workspace::report(text))
            .or_else(|| workspace::report(&self.text()))
    }
    fn completed_result(&self, completed: bool) -> Result<String, String> {
        let text = self.text();
        if !completed || text.trim().is_empty() {
            return Err("Agent 未返回完整审查结论，请重试或更换 Agent".into());
        }
        if self.report().is_none() {
            if let Some(error) =
                driver_api::failure::reported_transport_error(&text).or_else(|| {
                    self.messages
                        .values()
                        .find_map(|message| driver_api::failure::reported_transport_error(message))
                })
            {
                return Err(format!("审查连接中断：{error}"));
            }
            return Err("Agent 未返回完整审查结论，请重试或更换 Agent".into());
        }
        Ok(text)
    }
    fn push(&mut self, event: &DriverEvent, session: &str) -> Result<(), String> {
        match event {
            DriverEvent::TextDelta { text, .. } => self.legacy.push_str(text),
            DriverEvent::Presentation { item_id, data } => {
                if data["type"] != "message"
                    || data["parentId"].as_str().is_some_and(|id| !id.is_empty())
                    || data["sessionId"]
                        .as_str()
                        .is_some_and(|id| !id.is_empty() && id != session)
                {
                    return Ok(());
                }
                if let Some(id) = data["sourceEventId"].as_str()
                    && !self.seen.insert(id.to_owned())
                {
                    return Ok(());
                }
                if !self.messages.contains_key(item_id) {
                    self.order.push(item_id.clone());
                }
                let text = self.messages.entry(item_id.clone()).or_default();
                if let Some(snapshot) = data["text"].as_str() {
                    snapshot.clone_into(text);
                } else if let Some(delta) = data["textDelta"].as_str() {
                    text.push_str(delta);
                }
            }
            _ => {}
        }
        if self.legacy.len() + self.messages.values().map(String::len).sum::<usize>() > 64_000 {
            return Err("审查输出超过限制".into());
        }
        Ok(())
    }
    fn text(&self) -> String {
        if self.legacy.is_empty() {
            self.order
                .iter()
                .filter_map(|id| self.messages.get(id))
                .map(String::as_str)
                .collect()
        } else {
            self.legacy.clone()
        }
    }
}

// Older adapters could signal end_turn after printing a network failure. Correct
// these saved records on read, preserving their original transcript as evidence.
pub(super) fn recover_incomplete_review(run: &mut ReviewRun) {
    if !run.workspace_review
        || run.status != ReviewStatus::Completed
        || run.report.is_some()
        || workspace::report(&run.result).is_some()
    {
        return;
    }
    let error = driver_api::failure::reported_transport_error(&run.result)
        .or_else(|| {
            run.activities
                .iter()
                .filter(|item| {
                    item.data["type"] == "message"
                        && item.data["parentId"].as_str().is_none_or(str::is_empty)
                })
                .filter_map(|item| item.data["text"].as_str())
                .find_map(driver_api::failure::reported_transport_error)
        })
        .map(|error| format!("审查连接中断：{error}"));
    if let Some(error) = error {
        run.status = ReviewStatus::Failed;
        run.error = Some(error);
    }
}

impl Core {
    async fn flush_review_activity(
        &self,
        run: &mut ReviewRun,
        output: &mut ActivityOutput,
    ) -> Result<(), String> {
        let mut updates = output.changes();
        if updates.is_empty() {
            return Ok(());
        }
        for item in &mut updates {
            crate::redact_strings(&mut item.data, self.redactor.as_ref());
        }
        run.activity_revision += 1;
        activity::apply(&mut run.activities, updates.clone());
        self.append_runtime_event(
            &run.request.task_id,
            "review.activity",
            serde_json::json!({
                "taskId":run.request.task_id,"runId":run.request.run_id,
                "revision":run.activity_revision,"activities":updates,
            }),
            agent_protocol::EventSource::Hub,
        )
        .await
        .map_err(|_| "无法保存审查输出".to_owned())?;
        output.committed();
        Ok(())
    }
    pub(super) async fn execute_review(
        self: Arc<Self>,
        mut run: ReviewRun,
        scope: String,
        driver: Arc<dyn AgentDriver>,
        mut cancel: watch::Receiver<bool>,
    ) {
        let value: Value = serde_json::from_str(&scope).unwrap_or_default();
        let directories: Vec<String> =
            serde_json::from_value(value["directories"].clone()).unwrap_or_default();
        let cwd = directories.first().cloned().unwrap_or_default();
        let additional: Vec<_> = directories.into_iter().skip(1).collect();
        run.status = ReviewStatus::Running;
        // Keep partial output outside the cancellable future.
        let mut output = ReviewOutput::default();
        let mut activity = ActivityOutput::default();
        let outcome = async {
            self.save_review(&run, None).await.map_err(|_| "无法保存审查状态".to_owned())?;
            if *cancel.borrow() { return Err("审查已取消".into()); }
            let handle = tokio::select! {
                biased;
                _ = cancel.changed() => return Err("审查已取消".into()),
                result = tokio::time::timeout(Duration::from_mins(1), driver.create_review_session(&cwd, &additional)) => {
                    result.map_err(|_| "审查 Agent 启动超时".to_owned())?
                        .map_err(|error| format!("无法创建审查会话：{error}"))?
                }
            };
            run.native_session_id = Some(handle.native_session_id.clone());
            let analyze = async {
                self.save_review(&run, None).await.map_err(|_| "无法保存审查状态".to_owned())?;
                let mut stream = driver.prompt(&handle, PromptRequest {
                    prompt: workspace::prompt(run.request.kind, &scope), working_directory: cwd,
                    additional_directories: additional, attachments: vec![], context: vec![],
                    permission_mode: driver.review_prompt_permission_mode().map(str::to_owned),
                }).await.map_err(|error| format!("审查请求失败：{error}"))?;
                let mut tick = tokio::time::interval(Duration::from_millis(250));
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                let mut completed = false;
                loop {
                    let event = tokio::select! {
                        biased;
                        _ = tick.tick() => {
                            self.flush_review_activity(&mut run, &mut activity).await?;
                            continue;
                        }
                        event = stream.next() => event,
                    };
                    let Some(event) = event else { break; };
                    let event = event.map_err(|error| format!("审查连接中断：{error}"))?;
                    activity.push(&event, &handle.native_session_id);
                    output.push(&event, &handle.native_session_id)?;
                    match event {
                        DriverEvent::TurnCompleted { .. } => { completed = true; break; }
                        // User-authorized full-access review, scoped to this native session.
                        DriverEvent::ApprovalRequested { native_request_id, options, .. } => {
                            let option = options.iter().find(|option| option.semantic_role == driver_api::ApprovalSemanticRole::ApproveOnce)
                                .ok_or_else(|| "Agent 未提供本次操作的允许选项，无法继续审查".to_owned())?;
                            driver.respond_to_interaction(&handle, &native_request_id, &option.option_id).await
                                .map_err(|error| format!("审查操作授权失败：{error}"))?;
                        }
                        DriverEvent::QuestionRequested { .. } => return Err("审查 Agent 请求交互，未完成检查；请重试或更换 Agent".into()),
                        DriverEvent::TurnFailed { reason } => return Err(format!("审查失败：{reason}")),
                        DriverEvent::TurnInterrupted { .. } => return Err("审查执行中断".into()),
                        DriverEvent::TurnQuotaExhausted { .. } => return Err("审查 Agent 额度已耗尽".into()),
                        _ => {}
                    }
                }
                output.completed_result(completed)
            };
            let result = tokio::select! {
                biased;
                _ = cancel.changed() => Err("审查已取消".into()),
                result = tokio::time::timeout(Duration::from_mins(15), analyze) => result.unwrap_or_else(|_| Err("审查超过 15 分钟，可重试".into())),
            };
            if result.is_err() { let _ = tokio::time::timeout(Duration::from_secs(5), driver.interrupt(&handle)).await; }
            let _ = tokio::time::timeout(Duration::from_secs(5), driver.shutdown(handle)).await;
            result
        }.await;
        // Flush the tail on success, failure, timeout and cancellation alike.
        let flush = self.flush_review_activity(&mut run, &mut activity).await;
        let outcome = outcome.and_then(|value| flush.map(|()| value));
        run.result = self.redactor.redact(&output.text()).text;
        // Serialize final status with cancellation, including the completion race.
        let mut active = self.active_reviews.lock().await;
        match outcome {
            Ok(final_text) if !*cancel.borrow() => {
                run.status = ReviewStatus::Completed;
                run.result = self.redactor.redact(&final_text).text;
                // Progress messages belong to the execution trace, not the final JSON report.
                run.report = output.report();
                if let Some(report) = run.report.as_mut() {
                    for warning in &mut report.warnings {
                        *warning = self.redactor.redact(warning).text;
                    }
                    for item in &mut report.findings {
                        for text in [
                            &mut item.finding.title,
                            &mut item.finding.description,
                            &mut item.finding.suggestion,
                            &mut item.finding.evidence,
                        ] {
                            *text = self.redactor.redact(text).text;
                        }
                    }
                }
            }
            result => {
                run.status = if *cancel.borrow() {
                    ReviewStatus::Cancelled
                } else {
                    ReviewStatus::Failed
                };
                run.error = result.err().map(|error| self.redactor.redact(&error).text);
            }
        }
        run.finished_at = Some(Utc::now());
        let _ = self.save_review(&run, None).await;
        active.remove(&(run.request.task_id.clone(), run.request.run_id.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::{ReviewOutput, recover_incomplete_review};
    use driver_api::DriverEvent;
    use serde_json::json;
    fn display(data: serde_json::Value) -> DriverEvent {
        DriverEvent::Presentation {
            item_id: "message".into(),
            data,
        }
    }
    const NETWORK_ERROR: &str =
        "Error: RetriableError: [internal] HTTP/2 keepalive ping timed out after 5000ms";

    #[test]
    fn end_turn_with_progress_or_network_failure_is_not_a_completed_review() {
        let mut output = ReviewOutput::default();
        output
            .push(
                &DriverEvent::TextDelta {
                    item_id: "reply".into(),
                    text: format!("先核对调用方。\n\n{NETWORK_ERROR}"),
                },
                "root",
            )
            .unwrap();
        assert!(
            output
                .completed_result(true)
                .unwrap_err()
                .contains("审查连接中断")
        );
        assert!(output.text().contains(NETWORK_ERROR));
        let mut output = ReviewOutput::default();
        output
            .push(
                &display(json!({"type":"message", "text":"正在检查调用方。"})),
                "root",
            )
            .unwrap();
        assert!(output.completed_result(true).is_err());
        assert!(output.completed_result(false).is_err());
    }

    #[test]
    fn only_a_complete_report_can_confirm_zero_findings_and_quoted_errors_are_evidence() {
        let mut output = ReviewOutput::default();
        let report = json!({"summary":"已检查，日志含 Error: HTTP/2 keepalive ping timed out after 5000ms", "findings":[]}).to_string();
        output
            .push(&display(json!({"type":"message", "text":report})), "root")
            .unwrap();
        assert_eq!(output.completed_result(true).unwrap(), report);
        assert!(output.completed_result(false).is_err());
        assert!(output.report().unwrap().findings.is_empty());
    }

    #[test]
    fn saved_false_completions_recover_as_failed_without_losing_output() {
        let original = format!("先核对调用方。\n\n{NETWORK_ERROR}");
        let mut run: super::ReviewRun = serde_json::from_value(json!({
            "workspaceReview":true, "request":{"taskId":agent_domain::TaskId::new(),
                "runId":agent_domain::RunId::new(), "turnId":null, "sourceSnapshot":"snapshot",
                "executor":{"providerId":"cursor", "installationId":null}, "kind":"code", "retryOf":null},
            "status":"completed", "createdAt":"2026-10-14T08:21:00Z", "finishedAt":"2026-10-14T08:24:01Z",
            "nativeSessionId":null, "sourceSequence":1, "sourceArtifact":null,
            "inputSha256":"hash", "coverage":"", "result":original, "error":null
        })).unwrap();
        let old = run.clone();
        recover_incomplete_review(&mut run);
        assert_eq!(run.status, super::ReviewStatus::Failed);
        assert!(run.error.as_ref().unwrap().contains("审查连接中断"));
        assert_eq!(run.result, original);
        let mut legacy = old.clone();
        legacy.workspace_review = false;
        recover_incomplete_review(&mut legacy);
        assert_eq!(legacy.status, super::ReviewStatus::Completed);
        let mut valid = old;
        valid.report = super::workspace::report(r#"{"findings":[]}"#);
        recover_incomplete_review(&mut valid);
        assert_eq!(valid.status, super::ReviewStatus::Completed);
    }

    #[test]
    fn collects_native_messages_deduplicates_and_excludes_thoughts_children() {
        let mut output = ReviewOutput::default();
        let first = display(
            json!({"type":"message","sessionId":"root","sourceEventId":"a","textDelta":"{\"ok\":"}),
        );
        output.push(&first, "root").unwrap();
        output.push(&first, "root").unwrap();
        output
            .push(
                &display(json!({"type":"thought","delta":"private"})),
                "root",
            )
            .unwrap();
        output
            .push(
                &display(json!({"type":"message","parentId":"child","text":"child"})),
                "root",
            )
            .unwrap();
        output
            .push(
                &display(json!({"type":"message","sessionId":"other","textDelta":"other"})),
                "root",
            )
            .unwrap();
        output
            .push(
                &display(json!({"type":"message","sessionId":"root","textDelta":"true}"})),
                "root",
            )
            .unwrap();
        assert_eq!(output.text(), r#"{"ok":true}"#);
    }
    #[test]
    fn mirrors_do_not_duplicate_json_and_native_tools_are_allowed() {
        let mut output = ReviewOutput::default();
        output
            .push(
                &display(json!({"type":"message","textDelta":"part"})),
                "root",
            )
            .unwrap();
        output
            .push(
                &display(json!({"type":"message","text":"complete"})),
                "root",
            )
            .unwrap();
        output
            .push(
                &DriverEvent::TextDelta {
                    item_id: "legacy".into(),
                    text: "complete".into(),
                },
                "root",
            )
            .unwrap();
        assert_eq!(output.text(), "complete");
        let mut output = ReviewOutput::default();
        assert!(
            output
                .push(&display(json!({"type":"tool","name":"Read"})), "root")
                .is_ok()
        );
        assert!(
            output
                .push(&display(json!({"type":"subagent"})), "root")
                .is_ok()
        );
        assert!(
            output
                .push(
                    &display(json!({"type":"message","text":"x".repeat(64_001)})),
                    "root"
                )
                .is_err()
        );
    }
}
