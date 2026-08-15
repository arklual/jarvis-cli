//! Клиент узла jarvis-node — тот же протокол, которым живёт телефон.
//!
//! HTTP/1.1 руками поверх unix-сокета или TCP: протокол крошечный (десяток
//! путей, JSON, Connection: close), и тянуть ради него полноценный HTTP-стек
//! с TLS — лишний вес. TLS здесь не нужен по построению: либо локальный сокет,
//! либо петля через ssh-туннель.

use serde::Deserialize;
use serde_json::Value;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Куда ходить: сокет узла или петлевой TCP (конец ssh-туннеля).
#[derive(Debug, Clone, PartialEq)]
pub enum Endpoint {
    Unix(String),
    Tcp(String), // "127.0.0.1:port"
}

#[derive(Debug, Clone)]
pub struct NodeClient {
    pub endpoint: Endpoint,
}

/// Ответ `GET /hello`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Hello {
    pub version: String,
    pub host: String,
    pub cursor: u64,
    pub buffered: u64,
}

/// Партия событий `GET /events`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct EventsPage {
    pub cursor: u64,
    pub gap: bool,
    pub events: Vec<Recorded>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Recorded {
    pub cursor: u64,
    pub at: i64,
    pub envelope: Value,
}

/// Кусок файла `GET /file`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct FileChunk {
    pub from: u64,
    pub next: u64,
    pub size: u64,
    pub eof: bool,
    pub data: String,
}

impl FileChunk {
    /// Файл переписали с нуля — лента начинается заново. Два признака, потому
    /// что одного мало: узел зажимает `from` к размеру, а перезаписанный файл
    /// успевает перерасти старое смещение (урок мобильного клиента).
    pub fn rewound(&self, asked: u64, known_size: u64) -> bool {
        self.from < asked || (known_size > 0 && self.size < known_size)
    }
}

/// Живые паны `GET /panes`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct PanesReply {
    pub panes: Vec<Pane>,
    pub error: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Pane {
    pub pane: String,
    pub session: String,
    pub cwd: String,
}

/// Проект из оглавления узла.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RemoteProject {
    pub cwd: String,
    pub count: u64,
    #[serde(rename = "lastAt")]
    pub last_at: i64,
}

/// План нажатий для `/keys`.
///
/// Узел ждёт `[{"key":"Escape"}]` и отвечает отказом на `["Escape"]` —
/// перепутать легко, а видно это только по 400 из узла, уже в бою.
pub fn key_plan(key: &str) -> Value {
    serde_json::json!([{ "key": key }])
}

const CONNECT: Duration = Duration::from_secs(5);
const READ: Duration = Duration::from_secs(12);
/// Long-poll `/events` узел держит до 25 с — читаем дольше.
const READ_POLL: Duration = Duration::from_secs(35);

impl NodeClient {
    pub fn unix(path: impl Into<String>) -> Self {
        Self {
            endpoint: Endpoint::Unix(path.into()),
        }
    }

    pub fn tcp(addr: impl Into<String>) -> Self {
        Self {
            endpoint: Endpoint::Tcp(addr.into()),
        }
    }

    pub async fn hello(&self) -> Result<Hello, String> {
        self.get_json("/hello", READ).await
    }

    /// События с курсора; `wait=false` — беглый опрос без long-poll.
    pub async fn events(&self, since: u64) -> Result<EventsPage, String> {
        self.get_json(&format!("/events?since={since}"), READ_POLL)
            .await
    }

    /// Кусок транскрипта. `None` — файла ещё нет.
    pub async fn file(&self, path: &str, from: u64) -> Result<Option<FileChunk>, String> {
        let q = format!("/file?path={}&from={from}", urlencode(path));
        match self.request("GET", &q, None, READ).await {
            Ok((200, body)) => serde_json::from_slice(&body)
                .map(Some)
                .map_err(|e| format!("узел ответил не тем: {e}")),
            Ok((404, _)) => Ok(None),
            Ok((code, body)) => Err(http_err(code, &body)),
            Err(e) => Err(e),
        }
    }

    pub async fn panes(&self) -> Result<PanesReply, String> {
        self.get_json("/panes", READ).await
    }

    pub async fn projects(&self) -> Result<Vec<RemoteProject>, String> {
        #[derive(Deserialize)]
        struct R {
            #[serde(default)]
            projects: Vec<RemoteProject>,
        }
        self.get_json::<R>("/projects", READ)
            .await
            .map(|r| r.projects)
    }

    /// Текст `claude /usage` той машины (узел кэширует на пять минут).
    pub async fn usage_text(&self, fresh: bool) -> Result<String, String> {
        #[derive(Deserialize)]
        struct R {
            #[serde(default)]
            text: String,
            #[serde(default)]
            error: String,
        }
        let r: R = self
            .get_json(
                &format!("/usage{}", if fresh { "?fresh=1" } else { "" }),
                Duration::from_secs(120),
            )
            .await?;
        if r.text.trim().is_empty() {
            Err(if r.error.is_empty() {
                "узел вернул пустой /usage".into()
            } else {
                r.error
            })
        } else {
            Ok(r.text)
        }
    }

    pub async fn reply(&self, pane: &str, text: &str) -> Result<(), String> {
        self.post("/reply", &serde_json::json!({ "pane": pane, "text": text }))
            .await
    }

    /// Клавиши в пану. План собирайте через [`key_plan`] — узел принимает
    /// только объекты `{key}`/`{text}`, а голый список строк отвергает.
    pub async fn keys(&self, pane: &str, keys: Value) -> Result<(), String> {
        self.post("/keys", &serde_json::json!({ "pane": pane, "keys": keys }))
            .await
    }

    /// Закрыть пану вместе с агентом: конец сессии, а не прерывание хода.
    ///
    /// Мёртвая пана отвечает ошибкой tmux — и это нормальный ответ: значит
    /// закрывать было нечего, а сессию всё равно надо забыть.
    pub async fn kill(&self, pane: &str) -> Result<(), String> {
        self.post("/kill", &serde_json::json!({ "pane": pane }))
            .await
    }

    /// Слэш-команда пульта (`/model`, `/effort`) в пану.
    pub async fn control(&self, pane: &str, cmd: &str) -> Result<(), String> {
        self.post("/control", &serde_json::json!({ "pane": pane, "cmd": cmd }))
            .await
    }

    /// Экран паны — то, что видно в терминале сессии прямо сейчас.
    pub async fn screen(&self, pane: &str) -> Result<String, String> {
        #[derive(Deserialize)]
        struct R {
            #[serde(default)]
            screen: String,
        }
        self.get_json::<R>(&format!("/screen?pane={}", urlencode(pane)), READ)
            .await
            .map(|r| r.screen)
    }

    /// Поднять сессию агента: каталог создаётся, tmux — узла. Возвращает пану.
    pub async fn launch(&self, cwd: &str, cmd: &str, name: &str) -> Result<String, String> {
        let body = serde_json::json!({ "cwd": cwd, "cmd": cmd, "name": name });
        let (code, bytes) = self
            .request(
                "POST",
                "/launch",
                Some(body.to_string().into_bytes()),
                Duration::from_secs(60),
            )
            .await?;
        let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        if code == 200 {
            Ok(v.get("pane")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string())
        } else {
            Err(v
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("узел отказал")
                .to_string())
        }
    }

    /* ---------- транспорт ---------- */

    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        read: Duration,
    ) -> Result<T, String> {
        let (code, body) = self.request("GET", path, None, read).await?;
        if code != 200 {
            return Err(http_err(code, &body));
        }
        serde_json::from_slice(&body).map_err(|e| format!("узел ответил не тем: {e}"))
    }

    async fn post(&self, path: &str, body: &Value) -> Result<(), String> {
        let (code, bytes) = self
            .request("POST", path, Some(body.to_string().into_bytes()), READ)
            .await?;
        if code == 200 {
            Ok(())
        } else {
            Err(http_err(code, &bytes))
        }
    }

    async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<Vec<u8>>,
        read: Duration,
    ) -> Result<(u16, Vec<u8>), String> {
        let payload = build_request(method, path, body.as_deref());
        let raw = match &self.endpoint {
            Endpoint::Unix(p) => {
                let mut s = tokio::time::timeout(CONNECT, tokio::net::UnixStream::connect(p))
                    .await
                    .map_err(|_| "узел не отвечает (connect)".to_string())?
                    .map_err(|e| format!("узел недоступен: {e}"))?;
                talk(&mut s, &payload, read).await?
            }
            Endpoint::Tcp(a) => {
                let mut s = tokio::time::timeout(CONNECT, tokio::net::TcpStream::connect(a))
                    .await
                    .map_err(|_| "узел не отвечает (connect)".to_string())?
                    .map_err(|e| format!("узел недоступен: {e}"))?;
                talk(&mut s, &payload, read).await?
            }
        };
        parse_response(&raw)
    }
}

async fn talk<S: AsyncReadExt + AsyncWriteExt + Unpin>(
    stream: &mut S,
    payload: &[u8],
    read: Duration,
) -> Result<Vec<u8>, String> {
    stream
        .write_all(payload)
        .await
        .map_err(|e| format!("узел оборвал запись: {e}"))?;
    let mut out = Vec::with_capacity(4096);
    let fut = async {
        let mut buf = [0u8; 16 * 1024];
        loop {
            match stream.read(&mut buf).await {
                Ok(0) => break Ok(()),
                Ok(n) => out.extend_from_slice(&buf[..n]),
                Err(e) => break Err(format!("узел оборвал чтение: {e}")),
            }
        }
    };
    tokio::time::timeout(read, fut)
        .await
        .map_err(|_| "узел не ответил вовремя".to_string())??;
    Ok(out)
}

/// Собрать HTTP/1.1-запрос. `Connection: close` — по ответу на запрос:
/// протокол мелкий, а корректный keep-alive не стоит своих строк.
fn build_request(method: &str, path: &str, body: Option<&[u8]>) -> Vec<u8> {
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: jarvis\r\nConnection: close\r\nAccept: application/json\r\n"
    );
    if let Some(b) = body {
        head.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            b.len()
        ));
    }
    head.push_str("\r\n");
    let mut out = head.into_bytes();
    if let Some(b) = body {
        out.extend_from_slice(b);
    }
    out
}

/// Разобрать ответ: статус и тело. Тело — по Content-Length, а если его нет —
/// всё до конца соединения (мы просили close).
fn parse_response(raw: &[u8]) -> Result<(u16, Vec<u8>), String> {
    let split = find_headers_end(raw).ok_or("узел ответил не-HTTP")?;
    let head = String::from_utf8_lossy(&raw[..split]);
    let mut lines = head.split("\r\n");
    let status = lines.next().unwrap_or("");
    let code: u16 = status
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .ok_or_else(|| format!("странная строка статуса: {status}"))?;
    let mut body = raw[split + 4..].to_vec();
    for line in lines {
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        if k.eq_ignore_ascii_case("content-length") {
            if let Ok(len) = v.trim().parse::<usize>() {
                body.truncate(len);
            }
        }
        if k.eq_ignore_ascii_case("transfer-encoding") && v.trim().eq_ignore_ascii_case("chunked") {
            body = dechunk(&body);
        }
    }
    Ok((code, body))
}

fn find_headers_end(raw: &[u8]) -> Option<usize> {
    raw.windows(4).position(|w| w == b"\r\n\r\n")
}

/// Расшить chunked-тело: axum отвечает им, когда не знает длины заранее.
fn dechunk(mut body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len());
    while let Some(pos) = body.windows(2).position(|w| w == b"\r\n") {
        let size_line = String::from_utf8_lossy(&body[..pos]);
        let Ok(size) = usize::from_str_radix(size_line.trim(), 16) else {
            break;
        };
        if size == 0 {
            break;
        }
        let start = pos + 2;
        let end = start + size;
        if end > body.len() {
            // недокачали — отдаём, что есть
            out.extend_from_slice(&body[start..]);
            break;
        }
        out.extend_from_slice(&body[start..end]);
        body = body.get(end + 2..).unwrap_or(&[]);
    }
    out
}

fn http_err(code: u16, body: &[u8]) -> String {
    let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    v.get("error")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("узел ответил {code}"))
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Форма плана клавиш — договор с узлом, а не наше усмотрение.
    #[test]
    fn key_plan_is_a_list_of_objects() {
        assert_eq!(key_plan("Escape"), serde_json::json!([{ "key": "Escape" }]));
        assert!(
            key_plan("2")[0].get("key").is_some(),
            "голая строка узлу не подходит"
        );
    }

    /// Разговор с настоящим сокетом: поддельный узел записывает запрос, а мы
    /// сверяем, что ушло. Отправка ответа агенту — то место, где ошибка
    /// молчалива: команда «успешна», а текст не доехал ни до кого.
    #[tokio::test]
    async fn reply_goes_out_as_a_post_with_pane_and_text() {
        let dir = std::env::temp_dir().join(format!("jarvis-cli-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let sock = dir.join("node.sock");
        let _ = std::fs::remove_file(&sock);
        let listener = tokio::net::UnixListener::bind(&sock).unwrap();

        let seen = std::sync::Arc::new(tokio::sync::Mutex::new(String::new()));
        let seen2 = seen.clone();
        tokio::spawn(async move {
            let (mut st, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let n = tokio::io::AsyncReadExt::read(&mut st, &mut buf)
                .await
                .unwrap();
            *seen2.lock().await = String::from_utf8_lossy(&buf[..n]).into_owned();
            let body = b"{\"ok\":true}";
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            tokio::io::AsyncWriteExt::write_all(&mut st, head.as_bytes())
                .await
                .unwrap();
            tokio::io::AsyncWriteExt::write_all(&mut st, body)
                .await
                .unwrap();
        });

        let client = NodeClient::unix(sock.to_string_lossy().into_owned());
        client
            .reply("%7", "привет, агент")
            .await
            .expect("узел ответил ok");

        let req = seen.lock().await.clone();
        assert!(req.starts_with("POST /reply "), "{req}");
        assert!(req.contains("\"pane\":\"%7\""), "{req}");
        // Русский текст обязан доехать как есть — иначе агент получит мусор.
        assert!(req.contains("привет, агент"), "{req}");
        let _ = std::fs::remove_file(&sock);
    }

    #[test]
    fn http_response_is_parsed_with_content_length() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\n\r\n{\"ok\":true}extra-noise";
        let (code, body) = parse_response(raw).unwrap();
        assert_eq!(code, 200);
        assert_eq!(&body, b"{\"ok\":true}");
    }

    #[test]
    fn chunked_body_is_reassembled() {
        let raw =
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nb\r\n{\"ok\":true}\r\n0\r\n\r\n";
        let (code, body) = parse_response(raw).unwrap();
        assert_eq!(code, 200);
        assert_eq!(&body, b"{\"ok\":true}");
    }

    #[test]
    fn errors_prefer_the_node_message() {
        assert_eq!(
            http_err(502, "{\"error\":\"tmux не найден\"}".as_bytes()),
            "tmux не найден"
        );
        assert_eq!(http_err(500, "мусор".as_bytes()), "узел ответил 500");
    }

    #[test]
    fn rewound_needs_both_signals() {
        let c = FileChunk {
            from: 0,
            next: 0,
            size: 10,
            ..Default::default()
        };
        assert!(c.rewound(100, 0), "отдал раньше запрошенного");
        let grown = FileChunk {
            from: 50,
            next: 60,
            size: 40,
            ..Default::default()
        };
        assert!(grown.rewound(50, 90), "файл сжался — его переписали");
        let fine = FileChunk {
            from: 50,
            next: 60,
            size: 90,
            ..Default::default()
        };
        assert!(!fine.rewound(50, 80), "обычный рост — не перезапись");
    }

    #[test]
    fn urlencode_keeps_paths_readable() {
        assert_eq!(urlencode("/home/bob/a b.jsonl"), "/home/bob/a%20b.jsonl");
    }

    #[test]
    fn request_carries_body_and_length() {
        let req = build_request("POST", "/reply", Some(b"{}"));
        let text = String::from_utf8(req).unwrap();
        assert!(text.starts_with("POST /reply HTTP/1.1\r\n"));
        assert!(text.contains("Content-Length: 2"));
        assert!(text.ends_with("\r\n\r\n{}"));
    }
}
