//! When Git needs a username, password or passphrase it runs `GIT_ASKPASS` with the
//! question as its argument. That program is this one, on whichever machine runs Git:
//! `run_if_requested` passes the question to the operation that started Git, which
//! gets it to the person. An answer is never written to arguments, variables or files,
//! and this app does not keep it; remembering one is left to Git's credential helper.
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

const ADDRESS_VARIABLE: &str = "DOGCODES_GIT_ASKPASS_ADDRESS";
const TOKEN_VARIABLE: &str = "DOGCODES_GIT_ASKPASS_TOKEN";
const MAX_MESSAGE: u64 = 64 * 1024;
/// Written by Git or by a remote, so it is plain text of a bounded size.
const MAX_PROMPT_CHARS: usize = 1024;
const ANSWER_LIMIT: Duration = Duration::from_mins(5);
/// Why Git stopped: it needed a credential and nobody at the computer could
/// type one. Credentials are never relayed from a phone.
pub const UNATTENDED_CREDENTIAL: &str = "backendText.gitCredentialUnattended";

#[derive(Debug, Deserialize, Serialize)]
struct Question {
    token: String,
    prompt: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct Answer {
    answer: Option<String>,
}

/// Dispatch before anything else starts: Git adds only the prompt to the command line,
/// so this mode is recognized from the variables it inherited.
pub fn run_if_requested() {
    let (Some(address), Some(token)) = (
        std::env::var_os(ADDRESS_VARIABLE),
        std::env::var_os(TOKEN_VARIABLE),
    ) else {
        return;
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
        // Non-zero is how a declined question reaches Git.
        None => std::process::exit(1),
    }
}

fn ask(address: &str, token: &str, prompt: &str) -> Option<String> {
    use std::io::{BufRead, Read, Write};
    let address: SocketAddr = address.parse().ok()?;
    if !address.ip().is_loopback() {
        return None;
    }
    let mut stream = std::net::TcpStream::connect_timeout(&address, Duration::from_secs(5)).ok()?;
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

pub(super) type Asked = (String, oneshot::Sender<Option<String>>);

tokio::task_local! {
    /// Set around a Git operation that has someone to ask.
    pub(super) static ASKER: mpsc::UnboundedSender<Asked>;
    /// Set around work a paired phone asked for: nobody at this computer is
    /// waiting to type a credential, and none is taken from the phone.
    static NOBODY_HERE: ();
}

/// Runs `work` for someone who is not at this computer. Git questions it
/// raises are declined at once instead of waiting for an answer.
pub async fn for_someone_away<F: std::future::Future>(work: F) -> F::Output {
    NOBODY_HERE.scope((), work).await
}

/// Lives as long as the Git command it serves.
pub(super) struct AskpassServer {
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
    /// `None` when this Git operation has nobody to ask, or nothing can be listened on:
    /// Git then fails as it would without a terminal, and says why.
    pub(super) async fn for_this_operation() -> Option<Self> {
        let asker = ASKER.try_with(Clone::clone).ok()?;
        Self::start(asker).await.ok()
    }

    async fn start(asker: mpsc::UnboundedSender<Asked>) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let token = uuid::Uuid::new_v4().simple().to_string();
        let expected = token.clone();
        let worker = tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let asker = asker.clone();
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
                    let (reply, answer) = oneshot::channel();
                    if asker.send((question.prompt, reply)).is_err() {
                        return;
                    }
                    let answer = answer.await.ok().flatten();
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

    /// `ssh` remotes ask through its own variable, so those questions arrive here too.
    pub(super) fn apply(&self, command: &mut tokio::process::Command) -> std::io::Result<()> {
        let program = std::env::current_exe()?;
        command
            .env("GIT_ASKPASS", &program)
            .env("SSH_ASKPASS", &program)
            .env("SSH_ASKPASS_REQUIRE", "force")
            .env(ADDRESS_VARIABLE, self.address.to_string())
            .env(TOKEN_VARIABLE, &self.token);
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialPrompt {
    pub id: u64,
    pub text: String,
    /// Whether what is typed should stay hidden on screen.
    pub secret: bool,
}

struct Waiting {
    task: String,
    prompt: CredentialPrompt,
    reply: oneshot::Sender<Option<String>>,
}

/// What Git is waiting to be told, on the machine with someone to answer.
#[derive(Clone, Default)]
pub struct GitPrompts {
    waiting: Arc<Mutex<BTreeMap<u64, Waiting>>>,
    next: Arc<AtomicU64>,
    /// Tasks whose question was declined because nobody was there.
    unattended: Arc<Mutex<HashSet<String>>>,
}

/// Dropped when Git stops waiting, so the question leaves the screen with it.
struct Asking {
    waiting: Arc<Mutex<BTreeMap<u64, Waiting>>>,
    id: u64,
}

impl Drop for Asking {
    fn drop(&mut self) {
        if let Ok(mut waiting) = self.waiting.lock() {
            waiting.remove(&self.id);
        }
    }
}

impl GitPrompts {
    /// `None` when declined or unanswered for too long.
    pub async fn ask(&self, task: &str, text: &str) -> Option<String> {
        if NOBODY_HERE.try_with(|()| ()).is_ok() {
            self.mark_unattended(task);
            return None;
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let text: String = text.trim().chars().take(MAX_PROMPT_CHARS).collect();
        let lower = text.to_lowercase();
        // Only a name is safe to show: anything unrecognized stays hidden.
        let secret = !lower.contains("username");
        let (reply, answer) = oneshot::channel();
        self.waiting.lock().ok()?.insert(
            id,
            Waiting {
                task: task.to_owned(),
                prompt: CredentialPrompt { id, text, secret },
                reply,
            },
        );
        let _asking = Asking {
            waiting: self.waiting.clone(),
            id,
        };
        tokio::time::timeout(ANSWER_LIMIT, answer)
            .await
            .ok()?
            .ok()
            .flatten()
    }

    /// Records that the computer that asks for this machine had nobody there.
    pub fn mark_unattended(&self, task: &str) {
        if let Ok(mut unattended) = self.unattended.lock() {
            unattended.insert(task.to_owned());
        }
    }

    /// Whether a question for `task` was declined because nobody was there,
    /// since the last call. Each Git operation starts by clearing it.
    #[must_use]
    pub fn take_unattended(&self, task: &str) -> bool {
        self.unattended
            .lock()
            .is_ok_and(|mut unattended| unattended.remove(task))
    }

    /// One at a time, oldest first, for this task.
    #[must_use]
    pub fn current(&self, task: &str) -> Option<CredentialPrompt> {
        self.waiting
            .lock()
            .ok()?
            .values()
            .find(|item| item.task == task)
            .map(|item| item.prompt.clone())
    }

    pub fn answer(&self, task: &str, id: u64, answer: Option<String>) -> Result<(), String> {
        let mut waiting = self.waiting.lock().map_err(|_| "backendText.meaab543b")?;
        if waiting.get(&id).is_none_or(|item| item.task != task) {
            return Err("backendText.m2a4c7e42".into());
        }
        if let Some(item) = waiting.remove(&id) {
            let _ = item.reply.send(answer);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_question_reaches_the_asker_and_the_answer_comes_back() {
        let (asker, mut questions) = mpsc::unbounded_channel();
        let server = AskpassServer::start(asker).await.unwrap();
        let (address, token) = (server.address.to_string(), server.token.clone());
        let client =
            tokio::task::spawn_blocking(move || ask(&address, &token, "Username for 'x': "));
        let (prompt, reply) = questions.recv().await.unwrap();
        assert_eq!(prompt, "Username for 'x': ");
        reply.send(Some("me".into())).unwrap();
        assert_eq!(client.await.unwrap().as_deref(), Some("me"));
    }

    #[tokio::test]
    async fn a_wrong_token_is_not_asked_and_a_declined_question_is_a_refusal() {
        let (asker, mut questions) = mpsc::unbounded_channel();
        let server = AskpassServer::start(asker).await.unwrap();
        let address = server.address.to_string();
        let stranger = address.clone();
        let refused =
            tokio::task::spawn_blocking(move || ask(&stranger, "not the token", "Password: "));
        assert_eq!(refused.await.unwrap(), None);
        let token = server.token.clone();
        let client = tokio::task::spawn_blocking(move || ask(&address, &token, "Password: "));
        let (prompt, reply) = questions.recv().await.unwrap();
        assert_eq!(
            prompt, "Password: ",
            "the stranger's question never arrived"
        );
        reply.send(None).unwrap();
        assert_eq!(client.await.unwrap(), None);
    }

    #[test]
    fn only_a_loopback_address_is_told_anything() {
        assert_eq!(ask("192.0.2.1:1", "token", "Password: "), None);
    }

    #[tokio::test]
    async fn prompts_belong_to_their_task_and_leave_when_git_stops_waiting() {
        let prompts = GitPrompts::default();
        let asking = {
            let prompts = prompts.clone();
            tokio::spawn(async move { prompts.ask("task", "Password for 'x': ").await })
        };
        while prompts.current("task").is_none() {
            tokio::task::yield_now().await;
        }
        let prompt = prompts.current("task").unwrap();
        assert!(prompt.secret);
        assert_eq!(prompts.current("other"), None);
        assert!(
            prompts
                .answer("other", prompt.id, Some("x".into()))
                .is_err()
        );
        prompts
            .answer("task", prompt.id, Some("secret".into()))
            .unwrap();
        assert_eq!(asking.await.unwrap().as_deref(), Some("secret"));
        assert_eq!(prompts.current("task"), None);

        let abandoned = {
            let prompts = prompts.clone();
            tokio::spawn(async move { prompts.ask("task", "Username for 'x': ").await })
        };
        while prompts.current("task").is_none() {
            tokio::task::yield_now().await;
        }
        assert!(!prompts.current("task").unwrap().secret);
        abandoned.abort();
        let _ = abandoned.await;
        assert_eq!(prompts.current("task"), None);
    }

    #[tokio::test]
    async fn work_for_someone_away_declines_at_once_and_says_why() {
        let prompts = GitPrompts::default();
        assert!(!prompts.take_unattended("task"));
        let asked = for_someone_away(prompts.ask("task", "Password for 'x': ")).await;
        assert_eq!(asked, None);
        assert_eq!(prompts.current("task"), None, "nothing was shown");
        assert!(prompts.take_unattended("task"));
        assert!(!prompts.take_unattended("task"), "taken once");
    }
}
