//! Installs an agent on this machine, without a terminal, the way its vendor says to.
//! What runs is fixed here: the asker only names an agent.
use crate::{operation::Failure, updates};
use agent_domain::user_text;
use agent_runtime::run_bounded;
use std::path::Path;

/// A vendor's script, saved first so that a dropped download never runs half of it.
fn official(interpreter: &str, url: &str, arguments: &str) -> String {
    format!(
        r#"f="$(mktemp)"; trap 'rm -f "$f"' EXIT; curl -fsSL --proto =https --tlsv1.2 {url} -o "$f"; {interpreter} "$f" {arguments}"#
    )
}

/// Upgrades recognize an npm install under Node's own prefix, so that's the first choice
/// when it can be written to. System Node usually can't be without root.
fn npm_or(package: &str, otherwise: &str) -> String {
    format!(
        r#"if command -v npm >/dev/null 2>&1 && [ -w "$(npm root -g)" ]; then npm install -g {package}; else {otherwise}; fi"#
    )
}

/// Published only to npm. `latest` is a prerelease needing a newer Node than servers tend
/// to have, and older Node's npm can't even find it, so that's checked first and said plainly.
/// `~/.local/bin` is where it's looked for.
const DEEPSEEK_HARNESS: &str = r#"
if ! command -v node >/dev/null 2>&1 || ! command -v npm >/dev/null 2>&1; then
  echo "dogcodes-node-required: none" >&2; exit 1
fi
if ! node -e 'const [a,b]=process.versions.node.split(".").map(Number);process.exit(a>24||(a===24&&b>=9)?0:1)'; then
  echo "dogcodes-node-required: $(node -v)" >&2; exit 1
fi
if [ -w "$(npm root -g)" ]; then npm install -g @deepseek-ai/dsh@latest
else npm install -g --prefix "$HOME/.local" @deepseek-ai/dsh@latest; fi
"#;

fn script(provider: &str) -> Option<String> {
    let steps = match provider {
        "antigravity" => official("bash", "https://antigravity.google/cli/install.sh", ""),
        "claude-code" => official("bash", "https://claude.ai/install.sh", ""),
        "cursor" => official("bash", "https://cursor.com/install", ""),
        "grok" => official("bash", "https://x.ai/cli/install.sh", ""),
        "kimi" => official("bash", "https://code.kimi.com/kimi-code/install.sh", ""),
        // Without a terminal it would refuse to replace one this scan couldn't find.
        "kiro" => official("bash", "https://cli.kiro.dev/install", "--force"),
        "opencode" => official("bash", "https://opencode.ai/install", ""),
        "pi" => official("sh", "https://pi.dev/install.sh", ""),
        "codex" => npm_or(
            "@openai/codex",
            &official(
                "CODEX_NON_INTERACTIVE=1 sh",
                "https://chatgpt.com/codex/install.sh",
                "",
            ),
        ),
        "codebuddy" => npm_or(
            "@tencent-ai/codebuddy-code",
            &official("bash", "https://www.codebuddy.cn/cli/install.sh", ""),
        ),
        "deepseek-harness" => DEEPSEEK_HARNESS.to_owned(),
        _ => return None,
    };
    Some(format!("set -e; {steps}"))
}

/// Raw output is never passed on, as it may contain account details or a proxy's password.
fn explain(code: Option<i32>, stdout: &str, stderr: &str) -> String {
    if let Some(found) = stderr
        .lines()
        .find_map(|line| line.strip_prefix("dogcodes-node-required: "))
    {
        let current = found
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.')
            .then_some(found)
            .filter(|found| *found != "none");
        return current.map_or_else(
            || "backendText.installNodeMissing".to_owned(),
            |current| user_text("backendText.installNodeTooOld", &[current]),
        );
    }
    let output = format!("{stdout}\n{stderr}").to_ascii_lowercase();
    let says = |needles: &[&str]| needles.iter().any(|needle| output.contains(needle));
    // Tools that report how a connection failed, by name or by curl's own exit status.
    let unreachable = matches!(code, Some(6 | 7 | 28 | 35 | 52 | 56 | 60))
        || says(&[
            "could not resolve",
            "failed to connect",
            "timed out",
            "curl: (",
            "econnreset",
            "enotfound",
            "etimedout",
            "eai_again",
            "fetch failed",
        ]);
    if unreachable {
        "backendText.installUnreachable".into()
    } else if says(&[
        "npm: not found",
        "npm: command not found",
        "node: not found",
        "node.js is required",
        "npm is required",
        "requires node",
    ]) {
        "backendText.installNeedNode".into()
    } else if output.contains("unzip")
        && says(&["required", "missing", "not found", "not installed"])
    {
        "backendText.installNeedUnzip".into()
    } else if code == Some(127) || says(&["curl: not found", "bash: not found", "tar: not found"]) {
        "backendText.installMissingTools".into()
    } else if says(&["no space left", "enospc"]) {
        "backendText.installDiskFull".into()
    } else if says(&["permission denied", "eacces"]) {
        "backendText.installNoPermission".into()
    } else if says(&[
        "glibc",
        "musl is not supported",
        "unsupported",
        "not supported",
    ]) {
        "backendText.installUnsupported".into()
    } else {
        code.map_or_else(
            || "backendText.installFailed".to_owned(),
            |code| user_text("backendText.installFailedCode", &[&code.to_string()]),
        )
    }
}

/// # Errors
/// When it isn't an agent installed this way, or its installer didn't succeed.
pub async fn install(provider: &str) -> Result<(), Failure> {
    let script = script(provider)
        .ok_or_else(|| Failure::new("capability", "backendText.installNotOnServers"))?;
    let mut spec = updates::spec(Path::new("/bin/sh"), vec!["-c".into(), script], true);
    // Installers put themselves on PATH through whichever shell this account logs in with.
    spec.env_allowlist.extend(
        [
            "SHELL",
            "USER",
            "LOGNAME",
            "LANG",
            "ZDOTDIR",
            "XDG_CONFIG_HOME",
            "NVM_DIR",
        ]
        .map(str::to_owned),
    );
    spec.stdout_limit = 256 * 1024;
    spec.stderr_limit = 256 * 1024;
    let output = run_bounded(&spec)
        .await
        .map_err(|_| Failure::new("other", "backendText.installTimedOut"))?;
    if output.success {
        Ok(())
    } else {
        Err(Failure::new(
            "other",
            &explain(output.exit_code, &output.stdout, &output.stderr),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_agent_that_runs_on_a_server_can_be_installed() {
        for manifest in crate::detect::candidates(agent_registry::Platform::Linux, &[], None) {
            let script = script(manifest.0.id.as_str());
            assert!(script.is_some(), "{} has no installer", manifest.0.id);
            assert!(script.unwrap().starts_with("set -e; "));
        }
        assert!(script("zcode").is_none());
        assert!(script("; rm -rf /").is_none());
    }

    #[test]
    fn output_decides_the_message_but_is_never_part_of_it() {
        let secret = "https://user:hunter2@proxy";
        for (code, stderr, expected) in [
            (Some(1), "sh: npm: not found", "backendText.installNeedNode"),
            (
                Some(1),
                "dogcodes-node-required: v22.1.0",
                "backendText.installNodeTooOld",
            ),
            (
                Some(1),
                "dogcodes-node-required: none",
                "backendText.installNodeMissing",
            ),
            (
                Some(1),
                "dogcodes-node-required: $(hunter2 x)",
                "backendText.installNodeMissing",
            ),
            (Some(35), "", "backendText.installUnreachable"),
            (
                Some(1),
                "curl: (22) The requested URL returned error: 404",
                "backendText.installUnreachable",
            ),
            (
                Some(1),
                "npm error code ENOTFOUND",
                "backendText.installUnreachable",
            ),
            (
                Some(1),
                "version `GLIBC_2.28' not found",
                "backendText.installUnsupported",
            ),
            (
                Some(1),
                "Error: unzip is required",
                "backendText.installNeedUnzip",
            ),
            (
                Some(6),
                "curl: (6) Could not resolve host",
                "backendText.installUnreachable",
            ),
            (
                Some(1),
                "EACCES: permission denied",
                "backendText.installNoPermission",
            ),
            (Some(3), "something else", "backendText.installFailedCode"),
        ] {
            let message = explain(code, secret, stderr);
            assert!(message.contains(expected), "{message}");
            assert!(!message.contains("hunter2"));
        }
    }
}
