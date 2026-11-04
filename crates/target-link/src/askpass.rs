//! `ssh` asks for passwords, passphrases, one-time codes and host key decisions by
//! running `SSH_ASKPASS` with the prompt as its argument. That program is this app:
//! `run_if_requested` forwards the question to the connection that started `ssh`.
//!
//! A secret never appears in arguments, environment variables or files. It travels
//! only over this loopback connection, gated by a per-connection token.
use serde::{Deserialize, Serialize};
use std::{net::SocketAddr, sync::Arc};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    task::JoinHandle,
};

const ADDRESS_VARIABLE: &str = "DOGCODES_ASKPASS_ADDRESS";
const TOKEN_VARIABLE: &str = "DOGCODES_ASKPASS_TOKEN";
const MAX_MESSAGE: u64 = 64 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptKind {
    Password,
    Passphrase,
    HostKey,
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AskpassPrompt {
    pub kind: PromptKind,
    /// Written by `ssh` or, for keyboard-interactive, by the server: plain text only.
    pub text: String,
}

impl AskpassPrompt {
    #[must_use]
    pub fn classify(text: &str) -> Self {
        let lower = text.to_ascii_lowercase();
        let kind =
            if lower.contains("authenticity of host") || lower.contains("continue connecting") {
                PromptKind::HostKey
            } else if lower.contains("passphrase") {
                PromptKind::Passphrase
            } else if lower.contains("password") {
                PromptKind::Password
            } else {
                PromptKind::Other
            };
        Self {
            kind,
            text: text.trim().to_owned(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct Question {
    token: String,
    prompt: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct Answer {
    answer: Option<String>,
}

/// Dispatch before anything else starts. `ssh` adds nothing but the prompt to the
/// command line, so the mode is recognized from the variables it inherited.
#[must_use]
pub fn run_if_requested() -> bool {
    let (Some(address), Some(token)) = (
        std::env::var_os(ADDRESS_VARIABLE),
        std::env::var_os(TOKEN_VARIABLE),
    ) else {
        return false;
    };
    let prompt = std::env::args().nth(1).unwrap_or_default();
    let answered = address
        .to_str()
        .zip(token.to_str())
        .and_then(|(address, token)| ask(address, token, &prompt));
    match answered {
        Some(answer) => {
            println!("{answer}");
            std::process::exit(0);
        }
        // Non-zero is how askpass reports cancellation to `ssh`.
        None => std::process::exit(1),
    }
}

fn ask(address: &str, token: &str, prompt: &str) -> Option<String> {
    use std::io::{BufRead, Read, Write};
    let address: SocketAddr = address.parse().ok()?;
    if !address.ip().is_loopback() {
        return None;
    }
    let mut stream =
        std::net::TcpStream::connect_timeout(&address, std::time::Duration::from_secs(5)).ok()?;
    let question = Question {
        token: token.to_owned(),
        prompt: prompt.to_owned(),
    };
    let mut line = serde_json::to_vec(&question).ok()?;
    line.push(b'\n');
    stream.write_all(&line).ok()?;
    let mut reply = String::new();
    // No read timeout: the person may take a while to answer.
    std::io::BufReader::new(stream.take(MAX_MESSAGE))
        .read_line(&mut reply)
        .ok()?;
    serde_json::from_str::<Answer>(&reply).ok()?.answer
}

#[async_trait::async_trait]
pub trait PromptHandler: Send + Sync {
    /// `None` cancels, which makes `ssh` give up on this authentication attempt.
    async fn answer(&self, prompt: AskpassPrompt) -> Option<String>;
}

/// Lives exactly as long as the connection attempt that owns it.
pub(crate) struct AskpassServer {
    address: SocketAddr,
    token: String,
    worker: JoinHandle<()>,
}

impl Drop for AskpassServer {
    fn drop(&mut self) {
        self.worker.abort();
    }
}

impl AskpassServer {
    pub(crate) async fn start(handler: Arc<dyn PromptHandler>) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let token = uuid::Uuid::new_v4().simple().to_string();
        let expected = token.clone();
        let worker = tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let handler = handler.clone();
                let expected = expected.clone();
                tokio::spawn(async move {
                    let (read, mut write) = socket.split();
                    let mut line = String::new();
                    let mut reader = BufReader::new(read.take(MAX_MESSAGE));
                    if reader.read_line(&mut line).await.is_err() {
                        return;
                    }
                    let Ok(question) = serde_json::from_str::<Question>(&line) else {
                        return;
                    };
                    if question.token != expected {
                        return;
                    }
                    let answer = handler
                        .answer(AskpassPrompt::classify(&question.prompt))
                        .await;
                    if let Ok(mut reply) = serde_json::to_vec(&Answer { answer }) {
                        reply.push(b'\n');
                        let _ = write.write_all(&reply).await;
                    }
                });
            }
        });
        Ok(Self {
            address,
            token,
            worker,
        })
    }

    pub(crate) fn apply(&self, command: &mut tokio::process::Command, program: &std::path::Path) {
        command
            .env("SSH_ASKPASS", program)
            .env("SSH_ASKPASS_REQUIRE", "force")
            .env(ADDRESS_VARIABLE, self.address.to_string())
            .env(TOKEN_VARIABLE, &self.token);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed(Option<&'static str>);

    #[async_trait::async_trait]
    impl PromptHandler for Fixed {
        async fn answer(&self, _prompt: AskpassPrompt) -> Option<String> {
            self.0.map(str::to_owned)
        }
    }

    #[test]
    fn prompts_are_classified_by_what_they_ask_for() {
        for (text, kind) in [
            ("dev@box's password: ", PromptKind::Password),
            (
                "Enter passphrase for key '/home/dev/.ssh/id': ",
                PromptKind::Passphrase,
            ),
            (
                "The authenticity of host 'box (10.0.0.1)' can't be established.\nED25519 key fingerprint is SHA256:abc.\nAre you sure you want to continue connecting (yes/no/[fingerprint])? ",
                PromptKind::HostKey,
            ),
            ("Verification code: ", PromptKind::Other),
        ] {
            assert_eq!(AskpassPrompt::classify(text).kind, kind, "{text}");
        }
    }

    async fn exchange(server: &AskpassServer, token: &str) -> String {
        let mut socket = tokio::net::TcpStream::connect(server.address)
            .await
            .unwrap();
        let question = serde_json::to_string(&Question {
            token: token.into(),
            prompt: "password: ".into(),
        })
        .unwrap();
        socket
            .write_all(format!("{question}\n").as_bytes())
            .await
            .unwrap();
        let mut reply = String::new();
        socket.read_to_string(&mut reply).await.unwrap();
        reply
    }

    #[tokio::test]
    async fn only_the_holder_of_the_token_receives_an_answer() {
        let server = AskpassServer::start(Arc::new(Fixed(Some("secret"))))
            .await
            .unwrap();
        assert!(exchange(&server, "wrong").await.is_empty());
        let token = server.token.clone();
        let reply = exchange(&server, &token).await;
        assert_eq!(
            serde_json::from_str::<Answer>(&reply)
                .unwrap()
                .answer
                .as_deref(),
            Some("secret")
        );
    }

    #[tokio::test]
    async fn cancelling_is_distinct_from_an_empty_answer() {
        let server = AskpassServer::start(Arc::new(Fixed(None))).await.unwrap();
        let token = server.token.clone();
        let reply = exchange(&server, &token).await;
        assert!(
            serde_json::from_str::<Answer>(&reply)
                .unwrap()
                .answer
                .is_none()
        );
    }
}
