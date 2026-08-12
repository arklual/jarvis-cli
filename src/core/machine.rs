//! Машины: эта и узлы из общих настроек Jarvis.
//!
//! Список узлов читается из того же `settings.json`, что и у настольной
//! версии, — заводить второй реестр значило бы развести правды. Туннель к узлу
//! поднимаем сами: `ssh -N -L 127.0.0.1:port:<сокет>` — тот же приём, что в
//! панели, и та же причина не изобретать своё: аутентификация целиком ssh-шная,
//! CLI не хранит ни одного секрета.

use crate::core::node::NodeClient;
use crate::core::util::{jarvis_dir, shell_quote};
use serde_json::Value;
use std::process::Stdio;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub struct Machine {
    /// `local` или имя узла из настроек.
    pub name: String,
    /// Пусто у локальной; ssh-хост у узла.
    pub ssh_host: String,
    /// Каталог Jarvis на той машине.
    pub dir: String,
}

impl Machine {
    pub fn is_local(&self) -> bool {
        self.ssh_host.is_empty()
    }

    pub fn local() -> Self {
        Self {
            name: "local".into(),
            ssh_host: String::new(),
            dir: jarvis_dir().to_string_lossy().into_owned(),
        }
    }
}

/// Прочитать список машин: локальная всегда первой.
pub fn list() -> Vec<Machine> {
    let mut out = vec![Machine::local()];
    let settings = std::fs::read_to_string(jarvis_dir().join("settings.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .unwrap_or(Value::Null);
    out.extend(parse_remotes(&settings));
    out
}

/// Разбор ключа `remotes` — та же скупость, что в настольном: кривые записи
/// пропускаются молча, список правится руками и обязан переживать опечатку.
pub fn parse_remotes(settings: &Value) -> Vec<Machine> {
    let Some(arr) = settings.get("remotes").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out: Vec<Machine> = Vec::new();
    for item in arr {
        let get = |k: &str| {
            item.get(k)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
        };
        let name = get("name").to_string();
        let ssh_host = get("sshHost").to_string();
        if name.is_empty() || ssh_host.is_empty() || name == "local" {
            continue; // неадресуемо или спорит с локальной
        }
        if out.iter().any(|m| m.name == name) {
            continue;
        }
        let dir = {
            let d = get("jarvisDir").trim_end_matches('/');
            if d.is_empty() {
                "~/.jarvis".to_string()
            } else {
                d.to_string()
            }
        };
        out.push(Machine {
            name,
            ssh_host,
            dir,
        });
    }
    out
}

/// Путь к сокету узла на той машине.
///
/// Именно `node.sock`, а не `run.sock`: на одной машине узел и демон панели
/// живут рядом, и общий сокет означал бы, что один принимает запросы другого.
pub fn node_sock(dir: &str) -> String {
    format!("{}/node.sock", dir.trim_end_matches('/'))
}

/// Живой туннель к узлу. Пока структура жива — жив и ssh.
pub struct Tunnel {
    child: std::process::Child,
    pub port: u16,
}

impl Drop for Tunnel {
    /// ssh не должен пережить владельца: иначе на машине копятся осиротевшие
    /// туннели, а порты остаются занятыми до конца сессии терминала.
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Поднять туннель к узлу и дождаться, пока форвард начнёт принимать.
///
/// Ждём именно приёма, а не «поспим и понадеемся»: ssh открывает локальный
/// порт только после аутентификации, и слепая пауза — источник плавающих
/// «connection refused» (проверено на настольной версии).
pub async fn open_tunnel(m: &Machine) -> Result<Tunnel, String> {
    if m.is_local() {
        return Err("локальной машине туннель не нужен".into());
    }
    let port = free_port().ok_or("не нашёл свободный порт")?;
    let forward = format!("127.0.0.1:{port}:{}", node_sock(&expand_home(&m.dir, m)?));
    let child = std::process::Command::new("ssh")
        .args([
            "-N",
            "-o",
            "BatchMode=yes",
            "-o",
            "ExitOnForwardFailure=yes",
            "-o",
            "ServerAliveInterval=15",
            "-L",
            &forward,
            &m.ssh_host,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("ssh не запустился: {e} (он вообще установлен?)"))?;
    let tunnel = Tunnel { child, port };
    for _ in 0..60 {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            return Ok(tunnel);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Err(format!(
        "ssh не открыл туннель за 15 секунд. Проверь руками: ssh {} true",
        m.ssh_host
    ))
}

/// Раскрыть `~` в каталоге узла: ни ssh, ни sshd не делают этого в `-L`, и
/// туннель с тильдой поднимается, но молча отдаёт «connection reset» на каждый
/// запрос — худший вид поломки.
fn expand_home(dir: &str, m: &Machine) -> Result<String, String> {
    if !dir.starts_with('~') {
        return Ok(dir.to_string());
    }
    let out = std::process::Command::new("ssh")
        .args(["-o", "BatchMode=yes", &m.ssh_host, "printf %s \"$HOME\""])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("ssh не ответил: {e}"))?;
    let home = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !home.starts_with('/') {
        return Err(format!(
            "не узнал $HOME узла — впиши абсолютный каталог вместо {dir}"
        ));
    }
    Ok(dir.replacen('~', &home, 1))
}

fn free_port() -> Option<u16> {
    std::net::TcpListener::bind("127.0.0.1:0")
        .ok()
        .and_then(|l| l.local_addr().ok())
        .map(|a| a.port())
}

/// Клиент к машине: локально — сокет, к узлу — свежий туннель.
///
/// Туннель возвращается вместе с клиентом: пока владелец держит его, ssh жив.
pub async fn connect(m: &Machine) -> Result<(NodeClient, Option<Tunnel>), String> {
    if m.is_local() {
        let sock = node_sock(&m.dir);
        if !std::path::Path::new(&sock).exists() {
            return Err(format!(
                "узла нет на {sock} — проверь, запущен ли jarvis-node, \
                 и верен ли JARVIS_DIR"
            ));
        }
        return Ok((NodeClient::unix(sock), None));
    }
    let t = open_tunnel(m).await?;
    Ok((NodeClient::tcp(format!("127.0.0.1:{}", t.port)), Some(t)))
}

/// Выполнить команду на машине: локально через шелл, на узле — по ssh.
pub async fn run(m: &Machine, cwd: &str, cmd: &str, timeout: Duration) -> (i32, String) {
    let mut c = if m.is_local() {
        let mut c = tokio::process::Command::new("/bin/sh");
        c.arg("-lc").arg(cmd).current_dir(cwd);
        c
    } else {
        let full = format!("cd {} && {{ {cmd}\n}}", shell_quote(cwd));
        let mut c = tokio::process::Command::new("ssh");
        c.args([
            "-o",
            "BatchMode=yes",
            // Локаль этой машины не должна ехать на узел: там её может не
            // быть, и bash ругался бы в stderr на каждый запуск.
            "-o",
            "SendEnv=-LC_*",
            "-o",
            "SendEnv=-LANG",
        ])
        .arg(&m.ssh_host)
        .arg("bash")
        .arg("-lc")
        .arg(shell_quote(&full));
        c
    };
    c.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let Ok(Ok(out)) = tokio::time::timeout(timeout, c.output()).await else {
        return (-1, format!("не уложилось в {} с", timeout.as_secs()));
    };
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    let err = String::from_utf8_lossy(&out.stderr);
    if !err.trim().is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(err.trim_end());
    }
    (out.status.code().unwrap_or(-1), text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remotes_skip_broken_entries() {
        let v: Value = serde_json::from_str(
            r#"{ "remotes": [
                { "name": "vps", "sshHost": "user@vps", "jarvisDir": "/home/u/.jarvis/" },
                { "name": "", "sshHost": "x" },
                { "name": "noHost" },
                { "name": "vps", "sshHost": "duplicate" },
                { "name": "local", "sshHost": "спорит с локальной" },
                "не объект"
            ]}"#,
        )
        .unwrap();
        let list = parse_remotes(&v);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "vps");
        assert_eq!(list[0].dir, "/home/u/.jarvis", "хвостовой слэш срезан");
    }

    #[test]
    fn missing_dir_falls_back_to_default() {
        let v: Value =
            serde_json::from_str(r#"{ "remotes": [{ "name": "n", "sshHost": "h" }] }"#).unwrap();
        assert_eq!(parse_remotes(&v)[0].dir, "~/.jarvis");
    }

    #[test]
    fn no_remotes_key_is_not_an_error() {
        assert!(parse_remotes(&Value::Null).is_empty());
        assert!(parse_remotes(&serde_json::json!({ "remotes": "строка" })).is_empty());
    }

    #[test]
    fn socket_path_is_stable() {
        assert_eq!(node_sock("/home/u/.jarvis/"), "/home/u/.jarvis/node.sock");
    }

    #[tokio::test]
    async fn local_run_executes_in_the_directory() {
        let m = Machine::local();
        let (code, out) = run(&m, "/tmp", "pwd", Duration::from_secs(5)).await;
        assert_eq!(code, 0);
        assert!(out.contains("tmp"), "{out}");
    }
}
