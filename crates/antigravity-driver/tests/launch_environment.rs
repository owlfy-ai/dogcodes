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
        let mut driver = antigravity_driver::AntigravityDriver::new(
            "antigravity",
            &agent_domain::InstallationId::new().to_string(),
            TargetExecutable::new(target.clone(), "fixture-agy"),
            None,
            SessionResumeCapability::Unknown,
        )
        .unwrap();
        // This is about what reaches the target, not this machine's network.
        driver.set_sign_in_endpoint(None);
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
        // Only the print command may not start other processes: a conversation's
        // tools must, and readiness and models are not print mode.
        let quota = requests
            .iter()
            .find(|request| request.args.iter().any(|arg| arg == "--print=/usage"))
            .expect("quota request");
        assert!(quota.without_child_processes);
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.without_child_processes)
                .count(),
            1
        );
    }
}

fn driver_on(target: Arc<ProbeTarget>) -> antigravity_driver::AntigravityDriver {
    antigravity_driver::AntigravityDriver::new(
        "antigravity",
        &agent_domain::InstallationId::new().to_string(),
        TargetExecutable::new(target, "fixture-agy"),
        None,
        SessionResumeCapability::Unknown,
    )
    .unwrap()
}

/// Right after waking, the network isn't back. agy started then would open a Google
/// sign-in page, so quota waits instead and keeps whatever it last read.
#[tokio::test]
async fn quota_waits_for_the_network_instead_of_starting_agy() {
    let target = Arc::new(ProbeTarget {
        local: true,
        requests: Mutex::default(),
    });
    let mut driver = driver_on(target.clone());
    let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/", closed.local_addr().unwrap());
    drop(closed);
    driver.set_sign_in_endpoint(Some(endpoint));
    let error = driver
        .read_quota_with_refresh(None, true)
        .await
        .unwrap_err();
    assert!(
        matches!(&error, driver_api::DriverError::CapabilityUnavailable(kind) if kind == "quota_network_error"),
        "{error:?}"
    );
    assert!(
        target.requests.lock().unwrap().is_empty(),
        "agy was started"
    );

    // Once sign-in answers, even with an error status, agy runs.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/", listener.local_addr().unwrap());
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut stream, _) = listener.accept().await.unwrap();
        let _ = stream.read(&mut [0; 1024]).await;
        let _ = stream
            .write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n")
            .await;
    });
    driver.set_sign_in_endpoint(Some(endpoint));
    assert!(driver.read_quota_with_refresh(None, true).await.is_err());
    assert_eq!(target.requests.lock().unwrap().len(), 1);
}

/// Another machine's network can't be judged from here; its own agy decides.
#[tokio::test]
async fn a_remote_installation_is_not_checked_from_this_machine() {
    let target = Arc::new(ProbeTarget {
        local: false,
        requests: Mutex::default(),
    });
    let driver = driver_on(target.clone());
    assert!(driver.read_quota_with_refresh(None, true).await.is_err());
    assert_eq!(target.requests.lock().unwrap().len(), 1);
}
