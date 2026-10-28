//! Serialized admission protects agy's shared auth chain, without blocking stop/replies.
use super::{
    AntigravityDriver, AntigravityLaunch, Arc, AtomicBool, DriverError, HashMap, Ordering,
    SessionHandle, SessionProcess, SessionRequest, SessionResumeCapability, Startup,
    approval_bridge, enter_spawn_slot, run_session, spawn_workspace, stale_scope, uuid_like_id,
    workspace_instruction,
};

impl AntigravityDriver {
    pub(super) async fn open(
        &self,
        mut workspace: SessionRequest,
        resume: Option<&str>,
        model: Option<&str>,
        force: bool,
    ) -> Result<SessionHandle, DriverError> {
        let _admission = self.admission.lock().await;
        if workspace
            .permission_mode
            .as_deref()
            .is_none_or(|m| m == "configured")
        {
            workspace.permission_mode = Some("desktop-approvals".into());
        }
        let mut native = resume.map(str::to_owned);
        let mut selected = model.map(str::to_owned);
        if let Some(id) = resume {
            if let Some((key, process)) = self.find(id).await {
                let _writing = process.admission.lock().await;
                let retiring;
                {
                    let mut map = self.processes.lock().await;
                    let _gate = process.lifecycle.lock().expect("session lifecycle");
                    retiring =
                        process.finished.load(Ordering::Acquire) || *process.closing.borrow();
                    if !retiring && process.turn_active.load(Ordering::Acquire) {
                        return Err(DriverError::AutonomousTurnBusy);
                    }
                    let reusable = !force
                        && !process.finished.load(Ordering::Acquire)
                        && !*process.closing.borrow()
                        && process.permission_mode == workspace.permission_mode
                        && process.workspace_instruction == workspace_instruction(&workspace)
                        && model.is_none_or(|model| process.model.as_deref() == Some(model));
                    if reusable {
                        let canonical = process
                            .native_id
                            .lock()
                            .expect("native id")
                            .clone()
                            .unwrap_or_else(|| id.into());
                        if map.get(&key).is_none_or(|p| !Arc::ptr_eq(p, &process)) {
                            return Err(stale_scope());
                        }
                        if canonical != key && map.contains_key(&canonical) {
                            return Err(DriverError::SessionIncompatible(
                                "Antigravity conversation is already owned".into(),
                            ));
                        }
                        let handle = SessionHandle::scoped(canonical.clone());
                        *process.scope.lock().expect("session scope") = handle.clone();
                        if canonical != key {
                            map.remove(&key);
                            map.insert(canonical, process.clone());
                        }
                        return Ok(handle);
                    }
                }
                let original = process.handle();
                // Core has no replacement handle yet. Even a setup rejection
                // must retain the original retirement obligation until proven.
                if retiring {
                    process.confirmed().await;
                    self.remove_retired(&key, &process, &original).await;
                }
                if self.resume_capability != SessionResumeCapability::Supported {
                    return Err(DriverError::CapabilityUnavailable(
                        "native conversation resume unavailable".into(),
                    ));
                }
                native.clone_from(&process.native_id.lock().expect("native id"));
                if native.is_none() {
                    return Err(DriverError::SessionMissing(
                        "Antigravity has not reported a native conversation identity".into(),
                    ));
                }
                if !force && selected.is_none() {
                    selected.clone_from(&process.model);
                }
                if !retiring {
                    process.request_close(&original, true, false)?;
                    process.confirmed().await;
                    self.remove_retired(&key, &process, &original).await;
                }
            } else if let Some(record) = self
                .retired
                .lock()
                .expect("retired scopes")
                .get(id)
                .cloned()
            {
                native = Some(record.native.ok_or_else(|| {
                    DriverError::SessionMissing(
                        "Antigravity native conversation identity is unavailable".into(),
                    )
                })?);
            } else if id.starts_with("agy_") {
                return Err(DriverError::SessionMissing(
                    "Antigravity local process identity cannot be cold resumed".into(),
                ));
            }
            if self.resume_capability != SessionResumeCapability::Supported {
                return Err(DriverError::CapabilityUnavailable(
                    "agy --conversation resume has not been runtime verified".into(),
                ));
            }
        }
        self.retire_idle_sessions().await?;
        let slot = enter_spawn_slot(&self.spawn).await?;
        let (bridge, slot) =
            approval_bridge::Bridge::prepare(&self.executable, &workspace, slot).await?;
        let usage_log = tempfile::Builder::new()
            .prefix("dogcodes-agy-usage-")
            .tempdir()
            .ok();
        let mut child = spawn_workspace(
            &AntigravityLaunch {
                executable: self.executable.clone(),
            },
            native.as_deref(),
            selected.as_deref(),
            Some(&workspace),
            Some(&bridge),
            usage_log
                .as_ref()
                .map(|dir| dir.path().join("session.log"))
                .as_deref(),
        )?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| DriverError::Disconnected("agy stdin is unavailable".into()))?;
        let key = native
            .clone()
            .unwrap_or_else(|| format!("agy_{}", uuid_like_id()));
        let handle = SessionHandle::scoped(key.clone());
        let (closing, close_rx) = tokio::sync::watch::channel(false);
        let (completed, cleanup) = tokio::sync::watch::channel(None);
        let process = Arc::new(SessionProcess {
            scope: std::sync::Mutex::new(handle.clone()),
            turn: std::sync::Mutex::new(None),
            started: std::sync::Mutex::new(None),
            model: selected,
            admission: tokio::sync::Mutex::new(()),
            usage: std::sync::Mutex::new(None),
            usage_log,
            workspace_instruction: workspace_instruction(&workspace),
            permission_mode: workspace.permission_mode,
            pending_approvals: std::sync::Mutex::new(HashMap::new()),
            seen_hooks: std::sync::Mutex::new(std::collections::HashSet::new()),
            pending_questions: std::sync::Mutex::new(HashMap::new()),
            approval_children: std::sync::Mutex::new(std::collections::HashSet::new()),
            native_id: std::sync::Mutex::new(native),
            closing,
            cleanup,
            background: std::sync::Mutex::new(vec![]),
            lifecycle: std::sync::Mutex::new(()),
            stop_requested: AtomicBool::new(false),
            input_failed: AtomicBool::new(false),
            stdin: tokio::sync::Mutex::new(Some(stdin)),
            updates: std::sync::Mutex::new(None),
            turn_active: AtomicBool::new(false),
            interrupt_requested: AtomicBool::new(false),
            finished: Arc::new(AtomicBool::new(false)),
        });
        let mut startup = Startup {
            process: process.clone(),
            claimed: false,
        };
        process.observe_owner(self.stopping.subscribe());
        let bridge = bridge.start(&process);
        process
            .background
            .lock()
            .expect("background tasks")
            .push(bridge);
        // Move child AND the auth slot before the first post-spawn await.
        tokio::spawn(run_session(
            child,
            process.clone(),
            slot,
            close_rx,
            completed,
        ));
        let mut map = self.processes.lock().await;
        if map.contains_key(&key) {
            drop(map);
            process.request_close(&handle, false, false)?;
            process.confirmed().await;
            return Err(DriverError::SessionIncompatible(
                "Antigravity conversation is already owned".into(),
            ));
        }
        map.insert(key, process);
        startup.claimed = true;
        Ok(handle)
    }
}
