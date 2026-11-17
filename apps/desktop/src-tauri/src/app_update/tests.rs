use super::*;
use std::{
    collections::HashMap,
    sync::{
        Mutex as StdMutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use tauri::Listener;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

// Test-only key and signatures; see fixtures/README.md.
const PUBKEY: &str = include_str!("fixtures/test-only.pub");
const PACKAGE: &[u8] = include_bytes!("fixtures/package.bin");
const SIGNATURE: &str = include_str!("fixtures/package.bin.sig");
const UNVERSIONED_SIGNATURE: &str = include_str!("fixtures/package-unversioned.bin.sig");

/// The `origin` remote of this repository; releases are published there.
const REPOSITORY: &str = "owlfy-ai/dogcodes";

#[test]
fn the_app_reads_signed_manifests_from_the_latest_release() {
    let config: serde_json::Value =
        serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
    let updater = &config["plugins"]["updater"];
    assert_eq!(
        updater["endpoints"],
        serde_json::json!([format!(
            "https://github.com/{REPOSITORY}/releases/latest/download/latest.json"
        )])
    );
    assert_eq!(updater["requireSignedVersion"], true);
}

/// The manifest key the updater plugin looks up (`{os}-{arch}`), as `scripts/updater-manifest.mjs`
/// writes it.
fn manifest_platform(os: &str, arch: &str) -> Option<String> {
    let os = match os {
        "macos" => "darwin",
        "windows" => "windows",
        "linux" => "linux",
        _ => return None,
    };
    let arch = match arch {
        "aarch64" => "aarch64",
        "x86_64" => "x86_64",
        _ => return None,
    };
    Some(format!("{os}-{arch}"))
}

fn version(text: &str) -> Version {
    Version::parse(text).unwrap()
}

#[test]
fn only_a_higher_stable_version_is_an_update() {
    let current = version("0.1.3");
    assert!(is_update(&current, &version("0.1.4")));
    assert!(is_update(&current, &version("0.2.0")));
    assert!(is_update(&current, &version("1.0.0")));
    assert!(!is_update(&current, &version("0.1.3")));
    assert!(!is_update(&current, &version("0.1.2")));
    assert!(!is_update(&current, &version("0.1.10-beta.1")));
    assert!(!is_update(&current, &version("0.1.3+build.7")));
    // Numeric, not textual, ordering.
    assert!(is_update(&version("0.1.9"), &version("0.1.10")));
    // A pre-release build moves on to its stable release.
    assert!(is_update(&version("0.2.0-rc.1"), &version("0.2.0")));
}

#[test]
fn each_desktop_platform_has_one_manifest_key() {
    assert_eq!(
        manifest_platform("macos", "aarch64").as_deref(),
        Some("darwin-aarch64")
    );
    assert_eq!(
        manifest_platform("macos", "x86_64").as_deref(),
        Some("darwin-x86_64")
    );
    assert_eq!(
        manifest_platform("windows", "x86_64").as_deref(),
        Some("windows-x86_64")
    );
    assert_eq!(
        manifest_platform("windows", "aarch64").as_deref(),
        Some("windows-aarch64")
    );
    assert_eq!(
        manifest_platform("linux", "x86_64").as_deref(),
        Some("linux-x86_64")
    );
    assert_eq!(manifest_platform("freebsd", "x86_64"), None);
    assert_eq!(manifest_platform("windows", "x86"), None);
}

#[test]
fn a_cached_package_must_match_the_signature_and_version() {
    assert_eq!(verify_package(PACKAGE, SIGNATURE, PUBKEY, "0.2.0"), Ok(()));
    assert_eq!(verify_package(PACKAGE, SIGNATURE, PUBKEY, "v0.2.0"), Ok(()));
    let mut tampered = PACKAGE.to_vec();
    tampered[0] ^= 1;
    assert!(verify_package(&tampered, SIGNATURE, PUBKEY, "0.2.0").is_err());
    assert!(
        verify_package(PACKAGE, SIGNATURE, PUBKEY, "0.3.0")
            .unwrap_err()
            .contains("signed for 0.2.0")
    );
    assert_eq!(
        verify_package(PACKAGE, UNVERSIONED_SIGNATURE, PUBKEY, "0.2.0"),
        Err("package signature has no version".into())
    );
    assert!(verify_package(PACKAGE, SIGNATURE, "not a key", "0.2.0").is_err());
}

/// A minimal stand-in for the GitHub release download host.
type Routes = Arc<StdMutex<HashMap<String, (u16, Vec<u8>)>>>;

struct Server {
    base: String,
    routes: Routes,
    hits: Arc<StdMutex<HashMap<String, usize>>>,
}

impl Server {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let routes: Routes = Arc::default();
        let hits: Arc<StdMutex<HashMap<String, usize>>> = Arc::default();
        let (served, counted) = (Arc::clone(&routes), Arc::clone(&hits));
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let (served, counted) = (Arc::clone(&served), Arc::clone(&counted));
                tokio::spawn(async move {
                    let mut request = Vec::new();
                    let mut buffer = [0; 1024];
                    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                        let Ok(read) = stream.read(&mut buffer).await else {
                            return;
                        };
                        if read == 0 {
                            return;
                        }
                        request.extend_from_slice(&buffer[..read]);
                    }
                    let request = String::from_utf8_lossy(&request);
                    let path = request.split(' ').nth(1).unwrap_or("/").to_owned();
                    *counted.lock().unwrap().entry(path.clone()).or_default() += 1;
                    let (status, body) = served
                        .lock()
                        .unwrap()
                        .get(&path)
                        .cloned()
                        .unwrap_or((404, b"not found".to_vec()));
                    let head = format!(
                        "HTTP/1.1 {status} X\r\ncontent-length: {}\r\ncontent-type: application/octet-stream\r\nconnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(head.as_bytes()).await;
                    let _ = stream.write_all(&body).await;
                });
            }
        });
        Self { base, routes, hits }
    }

    fn serve(&self, path: &str, status: u16, body: impl Into<Vec<u8>>) {
        self.routes
            .lock()
            .unwrap()
            .insert(path.into(), (status, body.into()));
    }

    fn hits(&self, path: &str) -> usize {
        self.hits.lock().unwrap().get(path).copied().unwrap_or(0)
    }

    /// A `latest.json` as the release script writes it.
    fn publish(&self, version: &str, platform: Option<&str>, signature: &str) {
        let platform = platform.map_or_else(
            || manifest_platform(std::env::consts::OS, std::env::consts::ARCH).unwrap(),
            str::to_owned,
        );
        let manifest = serde_json::json!({
            "version": version,
            "notes": "",
            "pub_date": "2026-11-14T00:00:00Z",
            "platforms": { platform: { "url": format!("{}/package", self.base), "signature": signature } },
        });
        self.serve("/latest.json", 200, manifest.to_string());
        self.serve("/package", 200, PACKAGE);
    }
}

/// The mock app runs version 0.1.0 with the test key and the mock server as its endpoint.
fn app(server: &Server) -> tauri::App<tauri::test::MockRuntime> {
    let mut context = tauri::test::mock_context(tauri::test::noop_assets());
    context.config_mut().plugins.0.insert(
        "updater".into(),
        serde_json::json!({
            "pubkey": PUBKEY,
            "endpoints": [format!("{}/latest.json", server.base)],
            "requireSignedVersion": true,
        }),
    );
    tauri::test::mock_builder()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .build(context)
        .unwrap()
}

#[tokio::test]
async fn downloads_a_newer_release_once_and_announces_it() {
    let server = Server::start().await;
    server.publish("0.2.0", None, SIGNATURE);
    let app = app(&server);
    let announced = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&announced);
    app.listen_any(READY_EVENT, move |event| {
        assert!(event.payload().contains("\"version\":\"0.2.0\""));
        counter.fetch_add(1, Ordering::SeqCst);
    });
    let cache = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cache.path().join("0.1.9")).unwrap();
    let updates = AppUpdates::default();

    let notice = check(app.handle(), &updates, cache.path()).await.unwrap();
    assert_eq!(
        notice,
        Some(UpdateNotice {
            version: "0.2.0".into(),
            current_version: "0.1.0".into()
        })
    );
    assert_eq!(updates.notice().await, notice);
    assert_eq!(
        std::fs::read(cache.path().join("0.2.0").join(PACKAGE_FILE)).unwrap(),
        PACKAGE
    );
    assert!(
        !cache.path().join("0.1.9").exists(),
        "older packages are removed"
    );
    assert_eq!(announced.load(Ordering::SeqCst), 1);

    // Later rounds keep the ready package without downloading or announcing it again.
    assert_eq!(
        check(app.handle(), &updates, cache.path()).await.unwrap(),
        None
    );
    assert_eq!(server.hits("/package"), 1);
    assert_eq!(announced.load(Ordering::SeqCst), 1);

    // After a restart, the verified package on disk is reused.
    let restarted = AppUpdates::default();
    assert!(
        check(app.handle(), &restarted, cache.path())
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(server.hits("/package"), 1);
}

#[tokio::test]
async fn current_older_and_pre_release_versions_are_ignored() {
    let server = Server::start().await;
    let app = app(&server);
    let cache = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cache.path().join("0.0.9")).unwrap();
    let updates = AppUpdates::default();
    for version in ["0.1.0", "0.0.9", "0.2.0-beta.1"] {
        server.publish(version, None, SIGNATURE);
        assert_eq!(
            check(app.handle(), &updates, cache.path()).await.unwrap(),
            None,
            "{version}"
        );
    }
    assert_eq!(server.hits("/package"), 0);
    assert_eq!(updates.notice().await, None);
    assert!(!cache.path().join("0.0.9").exists());
}

#[tokio::test]
async fn failures_are_reported_to_the_log_and_announce_nothing() {
    let server = Server::start().await;
    let app = app(&server);
    let cache = tempfile::tempdir().unwrap();
    let updates = AppUpdates::default();

    // No manifest in the latest release (offline, or published without updater files).
    assert!(check(app.handle(), &updates, cache.path()).await.is_err());
    // Rate limited.
    server.serve("/latest.json", 429, "slow down");
    assert!(check(app.handle(), &updates, cache.path()).await.is_err());
    // No package for this platform.
    server.publish("0.2.0", Some("plan9-x86_64"), SIGNATURE);
    assert!(check(app.handle(), &updates, cache.path()).await.is_err());
    // A signature without a signed version, or a package that does not match it.
    server.publish("0.2.0", None, UNVERSIONED_SIGNATURE);
    assert!(check(app.handle(), &updates, cache.path()).await.is_err());
    server.publish("0.2.0", None, SIGNATURE);
    server.serve("/package", 200, b"something else".to_vec());
    assert!(check(app.handle(), &updates, cache.path()).await.is_err());

    assert_eq!(updates.notice().await, None);
    assert!(!cache.path().join("0.2.0").join(PACKAGE_FILE).exists());
}
