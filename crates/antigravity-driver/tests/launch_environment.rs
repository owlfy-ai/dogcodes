use agent_runtime::{ExecutionTarget, OwnedChild, ProcessRequest, TargetExecutable};
use driver_api::{AgentDriver, QuotaProvider, SessionResumeCapability};
use std::{
    ffi::OsString,
    io,
    path::PathBuf,
    sync::{Arc, Mutex},
};

struct ProbeTarget {
    local: bool,
    requests: Mutex<Vec<ProcessRequest>>,
}

impl ExecutionTarget for ProbeTarget {
    fn is_local(&self) -> bool {
        self.local
    }
    fn inherited(&self, _: &str) -> Option<OsString> {
        None
    }
    fn temp_directory(&self) -> PathBuf {
        std::env::temp_dir()
    }
    fn spawn(&self, request: ProcessRequest) -> io::Result<OwnedChild> {
        self.requests.lock().unwrap().push(request);
        Err(io::ErrorKind::NotFound.into())
    }
}

#[tokio::test]
async fn background_queries_cannot_start_the_native_background_updater() {
    for local in [true, false] {
        let target = Arc::new(ProbeTarget {
            local,
            requests: Mutex::default(),
        });
        let result = antigravity_driver::AntigravityDriver::probe_ready(TargetExecutable::new(
            target.clone(),
            "fixture-agy",
        ))
        .await;
        assert!(matches!(
            result,
            Err(driver_api::DriverError::NotConfigured(_))
        ));
        let driver = antigravity_driver::AntigravityDriver::new(
            "antigravity",
            &agent_domain::InstallationId::new().to_string(),
            TargetExecutable::new(target.clone(), "fixture-agy"),
            None,
            SessionResumeCapability::Unknown,
        )
        .unwrap();
        assert!(driver.available_models().await.is_err());
        assert!(driver.read_quota(None).await.is_err());
        let requests = target.requests.lock().unwrap();
        assert_eq!(
            requests.len(),
            3,
            "readiness, models and quota must reach the target"
        );
        for request in requests.iter() {
            assert_eq!(
                request
                    .env
                    .get(std::ffi::OsStr::new("AGY_CLI_DISABLE_AUTO_UPDATE")),
                Some(&OsString::from("true")),
                "{:?}",
                request.args,
            );
            assert_eq!(
                request
                    .env
                    .get(std::ffi::OsStr::new("AGY_CLI_NONINTERACTIVE_HEADLESS")),
                Some(&OsString::from("1")),
            );
        }
    }
}
