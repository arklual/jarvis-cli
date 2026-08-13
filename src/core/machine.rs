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

/// Записать машину в `settings.json`, не тронув остальное.
///
/// Файл общий с панелью, и в нём живут чужие ключи — от размеров окна до
/// настроек запуска. Поэтому не «сохранить свой взгляд на настройки», а
/// вписать одну запись в чужой документ.
pub fn upsert_remote(settings: &Value, m: &Machine) -> Value {
    let mut root = settings.as_object().cloned().unwrap_or_default();
    let mut arr = root
        .get("remotes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let entry = serde_json::json!({
        "name": m.name,
        "sshHost": m.ssh_host,
        "jarvisDir": m.dir,
    });
    match arr
        .iter()
        .position(|i| i.get("name").and_then(Value::as_str) == Some(m.name.as_str()))
    {
        // Известную машину правим на месте: её порядок в списке — это порядок,
        // к которому человек привык.
        Some(i) => {
            let mut merged = arr[i].as_object().cloned().unwrap_or_default();
            for (k, v) in entry.as_object().unwrap() {
                merged.insert(k.clone(), v.clone());
            }
            arr[i] = Value::Object(merged);
        }
        None => arr.push(entry),
    }
    root.insert("remotes".into(), Value::Array(arr));
    Value::Object(root)
}

/// Убрать машину из настроек. Возвращает `None`, если такой там и не было.
pub fn remove_remote(settings: &Value, name: &str) -> Option<Value> {
    let mut root = settings.as_object().cloned().unwrap_or_default();
    let arr = root.get("remotes").and_then(Value::as_array).cloned()?;
    let left: Vec<Value> = arr
        .iter()
        .filter(|i| i.get("name").and_then(Value::as_str) != Some(name))
        .cloned()
        .collect();
    if left.len() == arr.len() {
        return None;
    }
    root.insert("remotes".into(), Value::Array(left));
    Some(Value::Object(root))
}

/// Прочитать настройки целиком — чтобы потом вернуть их дополненными.
pub fn read_settings() -> Value {
    std::fs::read_to_string(jarvis_dir().join("settings.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .unwrap_or_else(|| Value::Object(Default::default()))
}

pub fn write_settings(v: &Value) -> Result<(), String> {
    let path = jarvis_dir().join("settings.json");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(v).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("не пишется {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("не переименовать: {e}"))
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
    // Пятнадцать секунд молчания ничего не объясняют — спрашиваем причину.
    probe_home(&Machine {
        dir: "~".into(),
        ..m.clone()
    })?;
    Err(format!(
        "ssh пускает, но туннель не поднялся за 15 секунд. Проверь руками: ssh {} true",
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
    Ok(dir.replacen('~', &probe_home(m)?, 1))
}

/// Спросить у узла его `$HOME` — заодно это проверка, что ssh вообще пускает.
///
/// Раньше любая неудача превращалась в «не узнал $HOME узла»: и запрет по
/// ключу, и неизвестный хост, и опечатка в адресе. Человек читал совет вписать
/// абсолютный путь и делал это — не помогало, потому что дело было не в пути.
/// Причину знает stderr от ssh, и он говорит её прямо.
fn probe_home(m: &Machine) -> Result<String, String> {
    let out = std::process::Command::new("ssh")
        .args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=10",
            &m.ssh_host,
            "printf %s \"$HOME\"",
        ])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("ssh не запустился: {e} (он вообще установлен?)"))?;
    let home = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if home.starts_with('/') {
        return Ok(home);
    }
    Err(ssh_hint(&m.ssh_host, &String::from_utf8_lossy(&out.stderr)))
}

/// Что именно не так со связью — словами и с ближайшим действием.
pub fn ssh_hint(host: &str, stderr: &str) -> String {
    let e = stderr.to_lowercase();
    if e.contains("permission denied") || e.contains("no supported authentication") {
        format!(
            "ssh не пускает на {host} без пароля. Разложи ключ: ssh-copy-id {host} \
             (пароль спросит один раз), потом проверь: ssh {host} true"
        )
    } else if e.contains("host key verification failed") || e.contains("known_hosts") {
        format!("ssh не знает {host} в лицо. Подключись руками один раз: ssh {host} true")
    } else if e.contains("could not resolve") || e.contains("name or service not known") {
        format!("не нашёлся адрес {host} — проверь имя хоста")
    } else if e.contains("connection refused") {
        format!("{host} отказал в соединении: sshd там слушает?")
    } else if e.contains("timed out") || e.contains("timeout") {
        format!("{host} не отвечает: сеть, файрвол или машина спит")
    } else {
        let first = stderr
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("ssh промолчал");
        format!("ssh до {host} не дошёл: {first}")
    }
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
            // Отдельный случай: на ноутбуке узла и не должно быть — агенты
            // живут на сервере. Совет «запусти jarvis-node» тут уводит в
            // сторону, а нужное действие человек уже почти сделал.
            let remotes: Vec<String> = list()
                .into_iter()
                .filter(|x| !x.is_local())
                .map(|x| x.name)
                .collect();
            if !remotes.is_empty() {
                return Err(format!(
                    "на этой машине узла нет ({sock}). Агенты, похоже, на другой — попробуй: {}",
                    remotes
                        .iter()
                        .map(|r| format!("jarvis -m {r} ls"))
                        .collect::<Vec<_>>()
                        .join(" · ")
                ));
            }
            return Err(format!(
                "узла нет на {sock} — проверь, запущен ли jarvis-node, \
                 и верен ли JARVIS_DIR"
            ));
        }
        return Ok((NodeClient::unix(sock), None));
    }
    let t = open_tunnel(m).await?;
    let client = NodeClient::tcp(format!("127.0.0.1:{}", t.port));
    // Туннель к НЕсуществующему сокету поднимается как ни в чём не бывало:
    // ssh узнаёт правду только в момент запроса и молча рвёт соединение.
    // Поэтому здороваемся сразу — иначе человек получил бы «connection reset»
    // на каждую команду и ни одного слова про причину: каталог узла.
    if let Err(e) = client.hello().await {
        return Err(format!(
            "ssh пускает, но узел на {} не отвечает ({e}). \
             Если он живёт в другом каталоге — впиши его: \
             jarvis machine add {} {} --dir /путь/к/каталогу",
            node_sock(&m.dir),
            m.name,
            m.ssh_host
        ));
    }
    Ok((client, Some(t)))
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

    /// Ошибка ssh должна называть причину и следующее действие. «Не узнал
    /// $HOME» на запрет по ключу отправляло чинить не то — и не помогало.
    #[test]
    fn ssh_failures_are_named_by_their_real_cause() {
        let denied = ssh_hint("me@vps", "me@vps: Permission denied (publickey).");
        assert!(denied.contains("ssh-copy-id me@vps"), "{denied}");

        let unknown = ssh_hint(
            "me@vps",
            "ssh: Could not resolve hostname vps: Name or service not known",
        );
        assert!(unknown.contains("адрес"), "{unknown}");

        let hostkey = ssh_hint("me@vps", "Host key verification failed.");
        assert!(hostkey.contains("ssh me@vps true"), "{hostkey}");

        // Незнакомую беду не толкуем — показываем как есть, но не молчим.
        let odd = ssh_hint(
            "me@vps",
            "kex_exchange_identification: read: Connection reset",
        );
        assert!(odd.contains("kex_exchange_identification"), "{odd}");
        assert!(!ssh_hint("me@vps", "").is_empty());
    }

    /// Файл общий с панелью: чужие ключи обязаны пережить нашу правку.
    #[test]
    fn adding_a_machine_keeps_the_rest_of_settings() {
        let before = serde_json::json!({
            "launchDangerous": true,
            "windowWidth": 1200,
            "remotes": [{ "name": "vps", "sshHost": "old@host", "jarvisDir": "~/.jarvis" }]
        });
        let after = upsert_remote(
            &before,
            &Machine {
                name: "vps".into(),
                ssh_host: "me@vps".into(),
                dir: "/srv/jarvis".into(),
            },
        );
        assert_eq!(after["launchDangerous"], serde_json::json!(true));
        assert_eq!(after["windowWidth"], serde_json::json!(1200));
        let remotes = after["remotes"].as_array().unwrap();
        assert_eq!(remotes.len(), 1, "правка, а не второй экземпляр");
        assert_eq!(remotes[0]["sshHost"], serde_json::json!("me@vps"));
        assert_eq!(remotes[0]["jarvisDir"], serde_json::json!("/srv/jarvis"));

        let added = upsert_remote(
            &after,
            &Machine {
                name: "mac".into(),
                ssh_host: "me@mac".into(),
                dir: "~/.jarvis".into(),
            },
        );
        assert_eq!(added["remotes"].as_array().unwrap().len(), 2);
        assert!(parse_remotes(&added).iter().any(|m| m.name == "mac"));
    }

    #[test]
    fn removing_reports_whether_there_was_anything_to_remove() {
        let s = serde_json::json!({ "remotes": [{ "name": "vps", "sshHost": "me@vps" }] });
        let left = remove_remote(&s, "vps").expect("была — убрали");
        assert!(left["remotes"].as_array().unwrap().is_empty());
        assert!(
            remove_remote(&s, "нетакой").is_none(),
            "молчаливого успеха быть не должно"
        );
    }

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
