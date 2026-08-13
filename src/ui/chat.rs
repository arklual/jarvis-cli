//! Чат сессии: разбор транскрипта и живое дочитывание.
//!
//! Дочитываем инкрементально, с последнего смещения, а не перечитываем хвост:
//! транскрипт живой сессии — это не реплики, а полные ответы инструментов, и
//! один `Read` большого файла даёт сотни килобайт. Урок мобильного клиента,
//! оплаченный там ростом памяти и подтормаживанием.

use crate::app::App;
use crate::core::node::NodeClient;
use crate::core::util::{clock, ellipsize, one_line};
use crate::ui::style::{band, paint, truncate, width, wrap, Bg, Caps, Role};
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

/// Разметку показываем как текст: в терминале `**Готово**` — это не жирный
/// шрифт, а четыре лишних знака. Убираем только явные маркеры, содержимое
/// строки не трогаем.
fn plain_md(s: &str) -> String {
    let mut out = s.replace("**", "");
    out = out.trim_start_matches('#').trim_start().to_string();
    if let Some(rest) = out.strip_prefix("- ") {
        out = format!("· {rest}");
    }
    out
}

/// Запись ленты блоком — как её видит человек.
///
/// Роли разведены не значком, а формой блока, и это главное, что делает ленту
/// читаемой: реплика человека лежит на подложке во всю ширину, ответ агента —
/// обычный текст с полем, вызов инструмента — узкая полоса другого тона. Роль
/// узнаётся раньше, чем прочитано первое слово, и лента перестаёт быть
/// простынёй одинаковых строк.
///
/// Текст переносится по словам, а не обрезается: в обрезанной реплике прячется
/// ровно то, ради чего её читают.
pub fn block(caps: &Caps, it: &Item, total: usize) -> Vec<String> {
    let total = total.max(12);
    match it.kind {
        Kind::User => {
            // Отступ в две ячейки с каждой стороны: буквы не должны упираться
            // в границу цвета.
            wrap(&plain_md(&it.text), total.saturating_sub(4))
                .into_iter()
                .map(|l| band(caps, Bg::User, &format!(" {l}"), total))
                .collect()
        }
        Kind::Agent => wrap(&plain_md(&it.text), total.saturating_sub(4))
            .into_iter()
            .map(|l| format!("  {}", paint(caps, Role::Text, &l)))
            .collect(),
        Kind::Tool => {
            let mark = if caps.unicode { "⏺" } else { "*" };
            let head = format!(
                "{} {}",
                paint(caps, Role::Accent, mark),
                paint(caps, Role::Text, &it.text)
            );
            let room = total.saturating_sub(width(&head) + 6);
            let line = if it.detail.is_empty() {
                head
            } else {
                format!(
                    "{head}  {}",
                    paint(caps, Role::Muted, &truncate(&it.detail, room.max(8)))
                )
            };
            vec![band(caps, Bg::Tool, &format!(" {line}"), total)]
        }
    }
}

/// Нужен ли воздух перед следующей записью.
///
/// Между разными голосами — пустая строка, между подряд идущими вызовами
/// инструментов — нет: десять полос с воздухом между ними занимают весь экран,
/// а читаются они как один поток работы.
pub fn needs_gap(prev: Option<&Kind>, next: &Kind) -> bool {
    match prev {
        None => false,
        Some(Kind::Tool) => *next != Kind::Tool,
        Some(_) => true,
    }
}

/// Лента целиком: блоки с воздухом там, где он нужен.
pub fn feed_lines(caps: &Caps, items: &[Item], total: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut prev: Option<Kind> = None;
    for it in items {
        if needs_gap(prev.as_ref(), &it.kind) {
            out.push(String::new());
        }
        out.extend(block(caps, it, total));
        prev = Some(it.kind.clone());
    }
    out
}

/// Показать чат и дочитывать, пока не прервут.
/// Лента одного транскрипта, читаемая по кусочку.
///
/// Состояние чтения вынесено в структуру, потому что читателей двое: команда
/// `jarvis chat`, которая печатает поток, и живое окно, которое дочитывает
/// между нажатиями клавиш. Договор о перезаписи файла и об обрезанной первой
/// строке обязан быть один — второй экземпляр этой логики разошёлся бы с
/// первым в первый же месяц.
pub struct Feed {
    path: String,
    offset: u64,
    known: u64,
    rest: String,
    first: bool,
    /// Транскрипта может не быть вовсе: сессия ещё ничего не писала.
    pub missing: bool,
    /// Дочитали до конца файла — можно и подождать.
    pub eof: bool,
}

impl Feed {
    /// Открыть ленту с хвоста: человеку нужен разговор, а не архив.
    pub async fn open(client: &NodeClient, path: &str) -> Result<Self, String> {
        let size = client
            .file(path, u64::MAX)
            .await?
            .map(|c| c.size)
            .unwrap_or(0);
        Ok(Self {
            path: path.to_string(),
            offset: size.saturating_sub(TAIL_BYTES),
            known: size,
            rest: String::new(),
            first: true,
            missing: false,
            eof: false,
        })
    }

    /// Дочитать появившееся. Пустой ответ — нормально: ничего не написали.
    pub async fn poll(&mut self, client: &NodeClient) -> Result<Vec<Item>, String> {
        let Some(chunk) = client.file(&self.path, self.offset).await? else {
            self.missing = true;
            self.eof = true;
            return Ok(Vec::new());
        };
        self.missing = false;
        if chunk.rewound(self.offset, self.known) {
            // Файл переписали (`/clear`, новый rollout): начинаем с начала
            // нового, иначе лента молча замрёт навсегда.
            self.offset = 0;
            self.known = 0;
            self.rest.clear();
            self.first = true;
            self.eof = false;
            return Ok(Vec::new());
        }
        let mut text = chunk.data.clone();
        if self.first && self.offset > 0 {
            // Первая строка почти наверняка обрезана посередине — она из тех
            // байт, что мы решили не читать.
            text = text
                .split_once('\n')
                .map(|(_, r)| r.to_string())
                .unwrap_or_default();
        }
        self.offset = chunk.next;
        self.known = chunk.size;
        self.first = false;
        self.eof = chunk.eof;
        if text.is_empty() {
            return Ok(Vec::new());
        }
        let combined = format!("{}{}", self.rest, text);
        match combined.rfind('\n') {
            None => {
                self.rest = combined;
                Ok(Vec::new())
            }
            Some(cut) => {
                let whole = combined[..cut].to_string();
                self.rest = combined[cut + 1..].to_string();
                Ok(parse(&whole))
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
    let mut feed = Feed::open(client, path).await?;
    let mut last_kind: Option<Kind> = None;
    loop {
        for it in feed.poll(client).await? {
            // Те же блоки, что в окне: одна лента — один вид, где бы человек
            // её ни читал. Воздух считаем по прошлой записи, а она могла
            // приехать в прошлой порции — потому и живёт снаружи цикла.
            if needs_gap(last_kind.as_ref(), &it.kind) {
                app.say(String::new());
            }
            for line in block(&app.caps, &it, app.caps.width as usize) {
                app.say(line);
            }
            last_kind = Some(it.kind.clone());
        }
        if feed.missing && !follow {
            app.dim("транскрипта ещё нет — сессия не слала событий");
            return Ok(());
        }
        if feed.eof {
            if !follow {
                return Ok(());
            }
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
            truecolor: false,
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

    /// Реплика человека и ответ агента должны различаться на глаз мгновенно:
    /// один блок на подложке во всю ширину, другой — текст с полем.
    /// Воздух между голосами есть, между подряд идущими инструментами — нет.
    #[test]
    fn gaps_separate_voices_not_every_line() {
        assert!(!needs_gap(None, &Kind::User), "перед первой записью пусто");
        assert!(
            !needs_gap(Some(&Kind::Tool), &Kind::Tool),
            "поток работы не рвём"
        );
        assert!(needs_gap(Some(&Kind::Tool), &Kind::Agent));
        assert!(needs_gap(Some(&Kind::Agent), &Kind::User));
        assert!(needs_gap(Some(&Kind::User), &Kind::Tool));
    }

    #[test]
    fn user_block_is_a_band_and_agent_block_is_not() {
        let c = Caps {
            color: true,
            truecolor: true,
            unicode: true,
            width: 40,
        };
        let user = block(
            &c,
            &Item {
                kind: Kind::User,
                text: "сделай уже".into(),
                detail: String::new(),
            },
            40,
        );
        let agent = block(
            &c,
            &Item {
                kind: Kind::Agent,
                text: "сделай уже".into(),
                detail: String::new(),
            },
            40,
        );
        assert!(user[0].contains("48;2;"), "у реплики человека нет подложки");
        assert!(!agent[0].contains("48;2;"), "ответ агента залит фоном");
        assert_eq!(width(&user[0]), 40, "подложка не во всю ширину");
    }

    /// Длинный ответ обязан читаться целиком.
    #[test]
    fn long_text_wraps_instead_of_being_cut() {
        let c = Caps {
            color: false,
            truecolor: false,
            unicode: true,
            width: 40,
        };
        let text = "раз два три четыре пять шесть семь восемь девять десять";
        let lines = block(
            &c,
            &Item {
                kind: Kind::Agent,
                text: text.into(),
                detail: String::new(),
            },
            30,
        );
        assert!(lines.len() > 1, "длинный ответ уместился в строку?");
        assert!(lines.iter().all(|l| width(l) <= 30), "{lines:?}");
        let joined = lines
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(joined, text, "перенос потерял слова");
        assert!(
            !lines.iter().any(|l| l.contains('…')),
            "текст обрезан, а не перенесён"
        );
    }

    #[test]
    fn project_line_shows_the_name_not_the_path() {
        let c = caps();
        let line = project_line(&c, "/home/bob/projects/jarvis", 12, 0);
        assert!(line.contains("jarvis") && !line.contains("/home/bob"));
        assert!(line.contains("12 чат."));
    }
}
