//! Чат сессии: разбор транскрипта и живое дочитывание.
//!
//! Дочитываем инкрементально, с последнего смещения, а не перечитываем хвост:
//! транскрипт живой сессии — это не реплики, а полные ответы инструментов, и
//! один `Read` большого файла даёт сотни килобайт. Урок мобильного клиента,
//! оплаченный там ростом памяти и подтормаживанием.

use crate::app::App;
use crate::core::node::NodeClient;
use crate::core::util::{clock, ellipsize, one_line};
use crate::ui::style::{paint, truncate, width, Caps, Role};
use serde_json::Value;
use std::time::Duration;

/// Первый заход берёт хвост файла: человеку нужен разговор, а не архив.
const TAIL_BYTES: u64 = 256 * 1024;
const IDLE: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    User,
    Agent,
    Tool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub kind: Kind,
    pub text: String,
    /// Подпись инструмента: чем он занят.
    pub detail: String,
}

/// Разбор JSONL Claude Code. Формат внутренний и дрейфует — читаем defensive:
/// битая строка пропускается, неизвестное поле игнорируется, падать нельзя.
pub fn parse(text: &str) -> Vec<Item> {
    let mut out = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v.get("isSidechain")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            || v.get("isMeta").and_then(Value::as_bool).unwrap_or(false)
        {
            continue;
        }
        let Some(msg) = v.get("message") else {
            continue;
        };
        let role = v.get("type").and_then(Value::as_str).unwrap_or("");
        let Some(content) = msg.get("content") else {
            continue;
        };
        // Content бывает строкой и массивом блоков — обе формы живые.
        if let Some(s) = content.as_str() {
            if role == "user" && !s.trim().is_empty() {
                out.push(Item {
                    kind: Kind::User,
                    text: s.trim().into(),
                    detail: String::new(),
                });
            }
            continue;
        }
        let Some(blocks) = content.as_array() else {
            continue;
        };
        for b in blocks {
            let btype = b.get("type").and_then(Value::as_str).unwrap_or("");
            match (role, btype) {
                ("user", "text") => {
                    if let Some(t) = b.get("text").and_then(Value::as_str) {
                        if !t.trim().is_empty() {
                            out.push(Item {
                                kind: Kind::User,
                                text: t.trim().into(),
                                detail: String::new(),
                            });
                        }
                    }
                }
                ("assistant", "text") => {
                    if let Some(t) = b.get("text").and_then(Value::as_str) {
                        if !t.trim().is_empty() {
                            out.push(Item {
                                kind: Kind::Agent,
                                text: t.trim().into(),
                                detail: String::new(),
                            });
                        }
                    }
                }
                ("assistant", "tool_use") => {
                    let name = b.get("name").and_then(Value::as_str).unwrap_or("tool");
                    out.push(Item {
                        kind: Kind::Tool,
                        text: name.to_string(),
                        detail: tool_detail(b.get("input")),
                    });
                }
                _ => {}
            }
        }
    }
    out
}

/// Подпись вызова инструмента: голое «Bash» не говорит ничего, «Bash · npm
/// test» говорит всё. Ключи перебираются по убыванию содержательности.
fn tool_detail(input: Option<&Value>) -> String {
    let Some(v) = input else { return String::new() };
    for key in [
        "command",
        "file_path",
        "path",
        "pattern",
        "description",
        "prompt",
        "url",
    ] {
        if let Some(s) = v.get(key).and_then(Value::as_str) {
            if !s.trim().is_empty() {
                let short = if key == "file_path" || key == "path" {
                    s.rsplit('/').next().unwrap_or(s).to_string()
                } else {
                    one_line(s)
                };
                return ellipsize(&short, 70);
            }
        }
    }
    String::new()
}

/// Одна запись ленты — как её видит человек.
pub fn item_line(caps: &Caps, it: &Item) -> String {
    match it.kind {
        // Роли разведены формой и отступом: «ты» прижато влево и помечено,
        // агент — обычным текстом. Диалог должен читаться как диалог.
        Kind::User => {
            let head = paint(caps, Role::Accent, "› ");
            let room = (caps.width as usize).saturating_sub(2);
            format!("{head}{}", truncate(&one_line(&it.text), room))
        }
        Kind::Agent => {
            let room = (caps.width as usize).saturating_sub(2);
            format!("  {}", truncate(&one_line(&it.text), room))
        }
        Kind::Tool => {
            let name = paint(caps, Role::Dim, &format!("  · {}", it.text));
            if it.detail.is_empty() {
                name
            } else {
                let room = (caps.width as usize).saturating_sub(width(&name) + 3);
                format!(
                    "{name} {}",
                    paint(caps, Role::Dim, &truncate(&it.detail, room.max(8)))
                )
            }
        }
    }
}

/// Показать чат и дочитывать, пока не прервут.
pub async fn tail(
    app: &App,
    client: &NodeClient,
    path: &str,
    title: &str,
    follow: bool,
) -> Result<(), String> {
    app.say(crate::ui::style::rule(&app.caps, title));

    let head = client.file(path, u64::MAX).await?;
    let size = head.map(|c| c.size).unwrap_or(0);
    let mut offset = size.saturating_sub(TAIL_BYTES);
    let mut known = size;
    let mut rest = String::new();
    let mut first = true;

    loop {
        let Some(chunk) = client.file(path, offset).await? else {
            if !follow {
                app.dim("транскрипта ещё нет — сессия не слала событий");
                return Ok(());
            }
            tokio::time::sleep(IDLE).await;
            continue;
        };
        if chunk.rewound(offset, known) {
            // Файл переписали (`/clear`, новый rollout): начинаем заново с
            // начала нового, иначе лента молча замрёт навсегда.
            offset = 0;
            known = 0;
            rest.clear();
            first = true;
            continue;
        }
        let mut text = chunk.data.clone();
        if first && offset > 0 {
            // Первая строка почти наверняка обрезана посередине — она из тех
            // байт, что мы решили не читать.
            text = text
                .split_once('\n')
                .map(|(_, r)| r.to_string())
                .unwrap_or_default();
        }
        offset = chunk.next;
        known = chunk.size;
        first = false;

        if !text.is_empty() {
            let combined = format!("{rest}{text}");
            match combined.rfind('\n') {
                None => rest = combined,
                Some(cut) => {
                    let whole = &combined[..cut];
                    rest = combined[cut + 1..].to_string();
                    for it in parse(whole) {
                        app.say(item_line(&app.caps, &it));
                    }
                }
            }
        }
        if !follow && chunk.eof {
            return Ok(());
        }
        if chunk.eof {
            tokio::time::sleep(IDLE).await;
        }
    }
}

/// Строка проекта в списке.
pub fn project_line(caps: &Caps, cwd: &str, count: u64, last_at: i64) -> String {
    let name = cwd.trim_end_matches('/').rsplit('/').next().unwrap_or(cwd);
    format!(
        "{}  {}  {}",
        crate::ui::style::pad(&truncate(name, 22), 22),
        paint(caps, Role::Dim, &format!("{count} чат.")),
        paint(caps, Role::Dim, &clock(last_at))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps() -> Caps {
        Caps {
            color: false,
            unicode: true,
            width: 80,
        }
    }

    #[test]
    fn parses_both_content_shapes() {
        let jsonl = [
            r#"{"type":"user","message":{"content":"привет"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"здравствуй"}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"cargo test"}}]}}"#,
        ]
        .join("\n");
        let items = parse(&jsonl);
        assert_eq!(items.len(), 3);
        assert_eq!(
            items[0],
            Item {
                kind: Kind::User,
                text: "привет".into(),
                detail: String::new()
            }
        );
        assert_eq!(items[1].kind, Kind::Agent);
        assert_eq!(items[2].detail, "cargo test");
    }

    #[test]
    fn broken_lines_are_skipped_not_fatal() {
        let jsonl = "не json\n{}\n{\"type\":\"user\",\"message\":{\"content\":\"ок\"}}";
        assert_eq!(parse(jsonl).len(), 1);
        assert!(parse("").is_empty());
    }

    #[test]
    fn sidechain_and_meta_are_hidden() {
        let jsonl = r#"{"type":"user","isSidechain":true,"message":{"content":"внутреннее"}}"#;
        assert!(
            parse(jsonl).is_empty(),
            "субагентская ветка — не разговор человека"
        );
    }

    #[test]
    fn file_tool_shows_the_basename() {
        let jsonl = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Edit","input":{"file_path":"/very/long/path/main.rs"}}]}}"#;
        assert_eq!(
            parse(jsonl)[0].detail,
            "main.rs",
            "в ленте нужно имя, а не весь путь"
        );
    }

    #[test]
    fn lines_never_exceed_the_terminal() {
        let c = caps();
        let long = "очень длинный текст ".repeat(30);
        for it in [
            Item {
                kind: Kind::User,
                text: long.clone(),
                detail: String::new(),
            },
            Item {
                kind: Kind::Agent,
                text: long.clone(),
                detail: String::new(),
            },
            Item {
                kind: Kind::Tool,
                text: "Bash".into(),
                detail: long,
            },
        ] {
            assert!(
                width(&item_line(&c, &it)) <= c.width as usize,
                "{:?} вылезла",
                it.kind
            );
        }
    }

    #[test]
    fn user_and_agent_look_different() {
        let c = caps();
        let user = item_line(
            &c,
            &Item {
                kind: Kind::User,
                text: "я".into(),
                detail: String::new(),
            },
        );
        let agent = item_line(
            &c,
            &Item {
                kind: Kind::Agent,
                text: "я".into(),
                detail: String::new(),
            },
        );
        assert_ne!(user, agent, "голоса обязаны различаться на глаз");
        assert!(user.starts_with('›'));
    }

    #[test]
    fn project_line_shows_the_name_not_the_path() {
        let c = caps();
        let line = project_line(&c, "/home/bob/projects/jarvis", 12, 0);
        assert!(line.contains("jarvis") && !line.contains("/home/bob"));
        assert!(line.contains("12 чат."));
    }
}
