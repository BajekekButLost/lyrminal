use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{mpsc, oneshot};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "kebab-case")]
pub enum Request {
    NowPlaying,
    ReloadConfig,
    LibraryChanged(LibraryChange),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LibraryChange {
    pub added: Vec<String>,
    pub changed: Vec<String>,
    pub removed: Vec<String>,
}

impl LibraryChange {
    pub fn forget_choices(&self, choices: &mut std::collections::HashMap<String, crate::providers::Candidate>) -> bool {
        let local = crate::providers::local::ID;
        let before = choices.len();
        choices.retain(|key, c| !self.added.contains(key) && !(c.provider == local && self.removed.contains(&c.id)));
        choices.len() != before
    }
}

pub struct Incoming {
    pub request: Request,
    pub reply: oneshot::Sender<Value>,
}

pub fn ok(mut v: Value) -> Value {
    if let Some(map) = v.as_object_mut() {
        map.insert("ok".into(), Value::Bool(true));
    }
    v
}

pub fn error(msg: impl Into<String>) -> Value {
    json!({ "ok": false, "error": msg.into() })
}

pub struct Server {
    path: PathBuf,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn alive(path: &Path) -> bool {
    std::os::unix::net::UnixStream::connect(path).is_ok()
}

pub fn serve(path: PathBuf, tx: mpsc::UnboundedSender<Incoming>) -> Option<Server> {
    if path.exists() {
        if alive(&path) {
            return None;
        }
        let _ = std::fs::remove_file(&path);
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let listener = tokio::net::UnixListener::bind(&path).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let tx = tx.clone();
            tokio::spawn(handle(stream, tx));
        }
    });
    Some(Server { path })
}

async fn handle(stream: UnixStream, tx: mpsc::UnboundedSender<Incoming>) {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let answer = match serde_json::from_str::<Request>(&line) {
            Ok(request) => {
                let (reply, rx) = oneshot::channel();
                if tx.send(Incoming { request, reply }).is_err() {
                    return;
                }
                rx.await.unwrap_or_else(|_| error("Lyrminal is shutting down"))
            }
            Err(e) => error(format!("bad request: {e}")),
        };
        let mut text = answer.to_string();
        text.push('\n');
        if write.write_all(text.as_bytes()).await.is_err() {
            return;
        }
    }
}

pub struct Client {
    lines: tokio::io::Lines<BufReader<OwnedReadHalf>>,
    write: OwnedWriteHalf,
}

pub const NOT_RUNNING: &str = "Lyrminal isn't running";

impl Client {
    pub async fn connect(path: &Path) -> Result<Self, String> {
        let stream = UnixStream::connect(path).await.map_err(|_| NOT_RUNNING.to_string())?;
        let (read, write) = stream.into_split();
        Ok(Self {
            lines: BufReader::new(read).lines(),
            write,
        })
    }

    pub async fn call(&mut self, request: &Request) -> Result<Value, String> {
        let mut text = serde_json::to_string(request).map_err(|e| e.to_string())?;
        text.push('\n');
        self.write.write_all(text.as_bytes()).await.map_err(|_| NOT_RUNNING.to_string())?;
        let line = self
            .lines
            .next_line()
            .await
            .map_err(|_| NOT_RUNNING.to_string())?
            .ok_or_else(|| NOT_RUNNING.to_string())?;
        let v: Value = serde_json::from_str(&line).map_err(|e| format!("unexpected answer from Lyrminal ({e})"))?;
        if v["ok"].as_bool() == Some(true) {
            Ok(v)
        } else {
            Err(v["error"].as_str().unwrap_or("Lyrminal could not answer").to_string())
        }
    }
}

pub async fn call(request: &Request) -> Result<Value, String> {
    Client::connect(&crate::config::socket_path()).await?.call(request).await
}

pub fn call_blocking(request: &Request) -> Result<Value, String> {
    use std::io::{BufRead, Write};
    let stream = std::os::unix::net::UnixStream::connect(crate::config::socket_path()).map_err(|_| NOT_RUNNING.to_string())?;
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    let mut text = serde_json::to_string(request).map_err(|e| e.to_string())?;
    text.push('\n');
    (&stream).write_all(text.as_bytes()).map_err(|_| NOT_RUNNING.to_string())?;
    let mut line = String::new();
    std::io::BufReader::new(&stream)
        .read_line(&mut line)
        .map_err(|_| "Lyrminal did not answer".to_string())?;
    let v: Value = serde_json::from_str(&line).map_err(|e| format!("unexpected answer from Lyrminal ({e})"))?;
    if v["ok"].as_bool() == Some(true) {
        Ok(v)
    } else {
        Err(v["error"].as_str().unwrap_or("Lyrminal could not answer").to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn answers_requests_over_the_socket() {
        let path = std::env::temp_dir().join(format!("lyrminal-ipc-{}.sock", std::process::id()));
        let (tx, mut rx) = mpsc::unbounded_channel::<Incoming>();
        let server = serve(path.clone(), tx).unwrap();
        tokio::spawn(async move {
            while let Some(m) = rx.recv().await {
                let _ = m.reply.send(ok(json!({ "title": "Song" })));
            }
        });
        let (tx2, _rx2) = mpsc::unbounded_channel::<Incoming>();
        assert!(serve(path.clone(), tx2).is_none());
        let mut client = Client::connect(&path).await.unwrap();
        for _ in 0..2 {
            let v = client.call(&Request::NowPlaying).await.unwrap();
            assert_eq!(v["title"], "Song");
        }
        drop(server);
        assert!(!path.exists());
        assert_eq!(Client::connect(&path).await.err().as_deref(), Some(NOT_RUNNING));
    }
}
