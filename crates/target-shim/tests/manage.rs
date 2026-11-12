//! `dogcodes-shim manage`, as the desktop runs it on a server: the login environment in,
//! one request and one answer. A fake `curl` stands in for the network.
#![cfg(unix)]
use serde_json::{Value, json};
use std::{
    io::Write,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Command, Stdio},
};

fn script(path: &Path, body: &str) {
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn ask(request: &Value, home: &Path, bin: &Path, extra: &[(&str, &str)]) -> Value {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dogcodes-shim"))
        .arg("manage")
        .env_clear()
        .env("HOME", home)
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .envs(extra.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    writeln!(child.stdin.take().unwrap(), "{request}").unwrap();
    let output = child.wait_with_output().unwrap();
    serde_json::from_slice(&output.stdout).unwrap()
}

fn recording_curl(bin: &Path, body: &str) {
    let reply = bin.join("curl.reply");
    std::fs::write(&reply, body).unwrap();
    script(
        &bin.join("curl"),
        "echo \"$@\" >> \"$0.args\"\ncat >> \"$0.stdin\"\ncat \"$0.reply\"",
    );
}

#[test]
fn claude_usage_uses_the_login_on_this_machine_through_stdin() {
    let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::write(
        home.path().join(".credentials.json"),
        r#"{"claudeAiOauth":{"accessToken":"server-token","subscriptionType":"max"}}"#,
    )
    .unwrap();
    recording_curl(bin.path(), "{\"five_hour\":{\"utilization\":40}}\n\n200");
    let reply = ask(
        &json!({"type":"claude_usage","version":"2.0.0"}),
        home.path(),
        bin.path(),
        &[("CLAUDE_CONFIG_DIR", home.path().to_str().unwrap())],
    );
    let usage = &reply["Ok"];
    assert_eq!(usage["failure"], Value::Null);
    assert_eq!(usage["body"]["five_hour"]["utilization"], 40);
    assert_eq!(usage["subscription"], "max");
    let args = std::fs::read_to_string(bin.path().join("curl.args")).unwrap();
    assert!(!args.contains("server-token"));
    let config = std::fs::read_to_string(bin.path().join("curl.stdin")).unwrap();
    assert!(config.contains("Bearer server-token") && config.contains("claude-code/2.0.0"));
}

#[test]
fn claude_without_a_login_says_so_and_asks_nobody() {
    let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    recording_curl(bin.path(), "unused");
    let reply = ask(
        &json!({"type":"claude_usage","version":null}),
        home.path(),
        bin.path(),
        &[("CLAUDE_CONFIG_DIR", home.path().to_str().unwrap())],
    );
    assert_eq!(reply["Ok"]["failure"], "claude_quota_login_required");
    assert!(!bin.path().join("curl.args").exists());
}

#[test]
fn cursor_usage_reads_the_stored_login_and_both_endpoints() {
    let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    for folder in [".cursor", ".config/cursor"] {
        let folder = home.path().join(folder);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join("auth.json"),
            r#"{"accessToken":"server-token"}"#,
        )
        .unwrap();
    }
    recording_curl(bin.path(), "{\"seen\":true}\n200");
    let reply = ask(
        &json!({"type":"cursor_usage"}),
        home.path(),
        bin.path(),
        &[("AGENT_CLI_CREDENTIAL_STORE", "file")],
    );
    assert_eq!(reply["Ok"]["usage"]["seen"], true);
    assert_eq!(reply["Ok"]["plan"]["seen"], true);
    let args = std::fs::read_to_string(bin.path().join("curl.args")).unwrap();
    assert!(args.contains("GetCurrentPeriodUsage") && args.contains("GetPlanInfo"));
    assert!(!args.contains("server-token"));
}

#[test]
fn cursor_without_a_login_is_a_login_failure() {
    let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let reply = ask(
        &json!({"type":"cursor_usage"}),
        home.path(),
        bin.path(),
        &[("AGENT_CLI_CREDENTIAL_STORE", "file")],
    );
    assert_eq!(reply["Err"]["kind"], "login");
}

#[test]
fn kimi_usage_is_fetched_where_its_endpoint_listens() {
    let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    script(
        &bin.path().join("kimi"),
        "echo \"Serving at http://127.0.0.1:43210/#token=server-token\"\nexec sleep 30",
    );
    recording_curl(bin.path(), "{\"data\":{\"quota\":{}}}");
    let reply = ask(
        &json!({"type":"kimi_usage","executable":bin.path().join("kimi")}),
        home.path(),
        bin.path(),
        &[],
    );
    assert_eq!(reply["Ok"]["data"]["quota"], json!({}));
    let args = std::fs::read_to_string(bin.path().join("curl.args")).unwrap();
    assert!(args.contains("http://127.0.0.1:43210/api/v1/oauth/usage"));
    assert!(!args.contains("server-token"));
}

/// Fake `curl` answers with a status trailer, as the one on a server would.
fn usage_curl(bin: &Path, body: &str) {
    recording_curl(bin, &format!("{body}\n200"));
}

fn fingerprint_of(reply: &Value) -> &str {
    reply["Ok"]["fingerprint"].as_str().unwrap()
}

#[test]
fn a_provider_account_is_read_from_the_files_on_this_machine() {
    let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    usage_curl(
        bin.path(),
        r#"{"mode":"unrestricted","unit":"USD","balance":"3"}"#,
    );
    std::fs::create_dir_all(home.path().join(".pi/agent")).unwrap();
    std::fs::write(
        home.path().join(".pi/agent/models.json"),
        json!({"providers":{"relay":{"baseUrl":"https://relay.example/x","apiKey":"pi-secret"}}})
            .to_string(),
    )
    .unwrap();
    let request = json!({
        "type": "provider_account",
        "agent": {"provider": "pi", "executable": "/server/pi", "version": "1.0.0"},
        "known": null,
    });
    let reply = ask(&request, home.path(), bin.path(), &[]);
    let readings = &reply["Ok"]["readings"];
    assert_eq!(readings[0]["kind"], "sub2_api");
    assert_eq!(readings[0]["outcome"]["Ok"]["balance"], "3");
    assert!(!reply.to_string().contains("pi-secret"));
    let args = std::fs::read_to_string(bin.path().join("curl.args")).unwrap();
    assert!(args.contains("https://relay.example/v1/usage"));
    assert!(!args.contains("pi-secret"));
    let config = std::fs::read_to_string(bin.path().join("curl.stdin")).unwrap();
    assert!(config.contains("Authorization: Bearer pi-secret"));

    // Told which account it already knows, it has nothing to fetch.
    std::fs::remove_file(bin.path().join("curl.args")).unwrap();
    let again = json!({
        "type": "provider_account",
        "agent": request["agent"],
        "known": fingerprint_of(&reply),
    });
    let reply = ask(&again, home.path(), bin.path(), &[]);
    assert_eq!(reply["Ok"]["state"], "unchanged");
    assert!(!bin.path().join("curl.args").exists());
}

#[test]
fn opencode_says_itself_which_account_is_signed_in() {
    let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    usage_curl(bin.path(), r#"{"usage":{}}"#);
    let exported = bin.path().join("export.json");
    std::fs::write(
        &exported,
        json!([{
            "id": "account", "integrationID": "opencode", "label": "me", "active": true,
            "value": {
                "type": "oauth",
                "access": "access-secret",
                "refresh": "refresh-secret",
                "metadata": {"orgID": "org-1"}
            }
        }])
        .to_string(),
    )
    .unwrap();
    let opencode = bin.path().join("opencode");
    script(
        &opencode,
        &format!(
            "[ \"$*\" = 'auth export --standalone' ] || exit 9\ncat '{}'",
            exported.display()
        ),
    );
    let reply = ask(
        &json!({
            "type": "provider_account",
            "agent": {"provider": "opencode", "executable": opencode, "version": "2.2.5"},
            "known": null,
        }),
        home.path(),
        bin.path(),
        &[],
    );
    assert_eq!(reply["Ok"]["readings"][0]["kind"], "go");
    assert!(!reply.to_string().contains("secret"));
    let config = std::fs::read_to_string(bin.path().join("curl.stdin")).unwrap();
    assert!(config.contains("Authorization: Bearer access-secret"));
    assert!(config.contains("x-opencode-org-id: org-1"));
    assert!(!config.contains("refresh-secret"));
}

#[test]
fn a_deepseek_harness_grant_goes_only_to_its_issuer() {
    let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    usage_curl(bin.path(), r#"{"code":0}"#);
    std::fs::create_dir_all(home.path().join(".dsh")).unwrap();
    std::fs::write(
        home.path().join(".dsh/.credentials.yaml"),
        "records:\n  deepseek-account-platform/default:\n    kind: grant\n    payload: {version: 1, token: grant-secret, issuer: 'https://platform.deepseek.com'}\n",
    )
    .unwrap();
    let reply = ask(
        &json!({
            "type": "provider_account",
            "agent": {"provider": "deepseek-harness", "executable": "/server/dsh", "version": null},
            "known": null,
        }),
        home.path(),
        bin.path(),
        &[],
    );
    assert_eq!(reply["Ok"]["readings"][0]["kind"], "deep_seek_platform");
    assert!(!reply.to_string().contains("grant-secret"));
    let args = std::fs::read_to_string(bin.path().join("curl.args")).unwrap();
    assert!(args.contains("https://platform.deepseek.com/api/v0/users/get_user_summary"));
    let config = std::fs::read_to_string(bin.path().join("curl.stdin")).unwrap();
    assert!(config.contains("x-dsh-auth-token: grant-secret"));
}

#[test]
fn detection_finds_an_agent_on_path_and_reports_its_version() {
    let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    script(
        &bin.path().join("kimi"),
        "if [ \"$1\" = acp ]; then echo 'kimi-code as an agent client protocol'; else echo 'kimi, version 2.5.0'; fi",
    );
    let reply = ask(&json!({"type":"detect"}), home.path(), bin.path(), &[]);
    let probes = reply["Ok"]["probes"].as_array().unwrap();
    let kimi = probes
        .iter()
        .find(|probe| probe["providerId"] == "kimi")
        .unwrap();
    assert_eq!(
        kimi["executable"],
        bin.path().join("kimi").to_str().unwrap()
    );
    assert_eq!(kimi["version"], "2.5.0");
    assert_eq!(kimi["status"], "installed");
    assert!(reply["Ok"]["complete"].as_bool().unwrap());
}

#[test]
fn binaries_that_cannot_load_are_failed_probes_instead_of_successful_installations() {
    let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    for name in ["kimi", "cursor-agent"] {
        script(
            &bin.path().join(name),
            "echo 'version GLIBC_2.28 not found' >&2; exit 1",
        );
    }
    let reply = ask(&json!({"type":"detect"}), home.path(), bin.path(), &[]);
    let probes = reply["Ok"]["probes"].as_array().unwrap();
    for provider in ["kimi", "cursor"] {
        let probe = probes
            .iter()
            .find(|probe| probe["providerId"] == provider)
            .unwrap();
        assert_eq!(probe["status"], "broken");
        assert_eq!(probe["version"], Value::Null);
        assert_eq!(probe["diagnostic"], "agent_runtime_incompatible");
    }
    assert_eq!(reply["Ok"]["complete"], true);
}

#[test]
fn an_upgrade_it_cant_do_itself_is_refused_without_touching_anything() {
    let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let codex = bin.path().join("codex");
    script(&codex, "touch \"$0.ran\"\necho 'codex-cli 1.0.0'");
    let reply = ask(
        &json!({"type":"upgrade","installation":{
            "installationId":"ins_one","providerId":"codex","executable":codex,"fingerprint":"f",
            "status":"ready","version":"1.0.0","versionText":"1.0.0","diagnostic":null,
            "probeElapsedMillis":0,"probedAt":"2026-01-01T00:00:00Z",
        }}),
        home.path(),
        bin.path(),
        &[],
    );
    assert_eq!(reply["Err"]["kind"], "other", "{reply}");
    assert!(!bin.path().join("codex.ran").exists());
}

/// Hands back `installer` as whatever was downloaded, and records what was asked for.
fn downloading_curl(bin: &Path, installer: &str) {
    script(&bin.join("installer"), installer);
    script(
        &bin.join("curl"),
        "echo \"$@\" >> \"$0.args\"\nwhile [ $# -gt 0 ]; do [ \"$1\" = -o ] && cp \"$(dirname \"$0\")/installer\" \"$2\"; shift; done",
    );
}

#[test]
fn an_agent_is_installed_by_its_vendors_script_with_nobody_to_answer_it() {
    let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let found = home.path().join("found");
    downloading_curl(
        bin.path(),
        &format!(
            "mkdir -p \"$HOME/.grok/bin\"\nread answer || echo \"$SHELL\" > {found}",
            found = found.display()
        ),
    );
    let reply = ask(
        &json!({"type":"install","provider":"grok"}),
        home.path(),
        bin.path(),
        &[("SHELL", "/bin/zsh"), ("UNRELATED", "kept out")],
    );
    assert_eq!(reply, json!({"Ok": null}));
    assert!(home.path().join(".grok/bin").is_dir());
    // Run without stdin, so a prompt can't hang it, and able to find the account's shell.
    assert_eq!(std::fs::read_to_string(&found).unwrap(), "/bin/zsh\n");
    let args = std::fs::read_to_string(bin.path().join("curl.args")).unwrap();
    assert!(args.contains("https://x.ai/cli/install.sh") && args.contains("--proto =https"));
}

#[test]
fn a_failed_download_runs_nothing_and_a_failed_installer_says_why() {
    let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let ran = home.path().join("ran");
    script(
        &bin.path().join("curl"),
        "echo 'curl: (6) Could not resolve host' >&2\nexit 6",
    );
    script(
        &bin.path().join("bash"),
        &format!("touch {}", ran.display()),
    );
    let request = json!({"type":"install","provider":"kiro"});
    let reply = ask(&request, home.path(), bin.path(), &[]);
    assert_eq!(reply["Err"]["kind"], "other");
    assert!(
        reply["Err"]["message"]
            .as_str()
            .unwrap()
            .contains("backendText.installUnreachable")
    );
    assert!(!ran.exists());

    std::fs::remove_file(bin.path().join("bash")).unwrap();
    downloading_curl(
        bin.path(),
        "echo 'unzip is required, token=hunter2' >&2\nexit 1",
    );
    let reply = ask(&request, home.path(), bin.path(), &[]);
    let message = reply["Err"]["message"].as_str().unwrap();
    assert!(message.contains("unzip") && !message.contains("hunter2"));
}

#[test]
fn deepseek_harness_uses_a_new_enough_node_and_asks_for_the_prerelease_by_name() {
    let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    script(&bin.path().join("node"), "exit 0");
    script(
        &bin.path().join("npm"),
        "case \"$1\" in root) echo /nonexistent/unwritable;; *) echo \"$@\" >> \"$0.args\";; esac",
    );
    downloading_curl(bin.path(), "exit 1");
    let request = json!({"type":"install","provider":"deepseek-harness"});
    assert_eq!(
        ask(&request, home.path(), bin.path(), &[]),
        json!({"Ok": null})
    );
    let args = std::fs::read_to_string(bin.path().join("npm.args")).unwrap();
    assert!(args.contains("@deepseek-ai/dsh@latest") && args.contains("--prefix"));
    assert!(args.contains(home.path().join(".local").to_str().unwrap()));
    assert!(!bin.path().join("curl.args").exists());
}

#[test]
fn deepseek_harness_says_which_node_it_needs_and_which_is_there() {
    let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let request = json!({"type":"install","provider":"deepseek-harness"});
    script(
        &bin.path().join("node"),
        "case \"$1\" in -v) echo v22.3.0;; *) exit 1;; esac",
    );
    script(&bin.path().join("npm"), "echo ran >> \"$0.args\"");
    let reply = ask(&request, home.path(), bin.path(), &[]);
    let message = reply["Err"]["message"].as_str().unwrap();
    assert!(
        message.contains("backendText.installNodeTooOld") && message.contains("v22.3.0"),
        "{message}"
    );
    assert!(!bin.path().join("npm.args").exists());
    // Nothing else is on this PATH that would count as npm, unless the machine has its own.
    std::fs::remove_file(bin.path().join("npm")).unwrap();
    if !Path::new("/usr/bin/npm").exists() && !Path::new("/bin/npm").exists() {
        let reply = ask(&request, home.path(), bin.path(), &[]);
        let message = reply["Err"]["message"].as_str().unwrap();
        assert!(
            message.contains("backendText.installNodeMissing"),
            "{message}"
        );
    }
}

#[test]
fn an_agent_that_cant_run_on_a_server_is_not_installed() {
    let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    downloading_curl(bin.path(), "exit 0");
    let reply = ask(
        &json!({"type":"install","provider":"zcode"}),
        home.path(),
        bin.path(),
        &[],
    );
    assert_eq!(reply["Err"]["kind"], "capability");
    assert!(!bin.path().join("curl.args").exists());
}

#[test]
fn a_request_it_cant_read_gets_an_answer_anyway() {
    let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let reply = ask(&json!({"type":"nonsense"}), home.path(), bin.path(), &[]);
    assert_eq!(reply["Err"]["kind"], "other");
}
