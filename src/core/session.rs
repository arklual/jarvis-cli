//! Реестр сессий: свёртка конвертов хуков в список того, что происходит.
//!
//! Тот же смысл, что в панели и на телефоне, и та же осознанная скупость:
//! кто работает, кто спрашивает, кто закончил. Полную доску задач и историю
//! ходов терминалу воспроизводить незачем — за ними идут в панель.

use crate::core::node::Recorded;
use crate::core::util::{ellipsize, one_line};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    #[default]
    Idle,
    Working,
    Waiting,
    Done,
    /// Ход сорвался: перегрузка API, сеть, биллинг. Никто не продолжит.
    Failed,
    /// Упёрлись в лимит аккаунта — до сброса окна не поедет.
    Limit,
}

impl Status {
    /// Порядок в списке: кто требует человека — выше.
    pub fn rank(self) -> u8 {
        match self {
            Status::Waiting => 0,
            Status::Limit => 1,
            Status::Failed => 2,
            Status::Working => 3,
            Status::Done => 4,
            Status::Idle => 5,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Status::Waiting => "спрашивает",
            Status::Working => "работает",
            Status::Done => "закончила",
            Status::Failed => "сорвалась",
            Status::Limit => "лимит",
            Status::Idle => "ждёт",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Session {
    pub id: String,
    pub status: Status,
    pub project: Option<String>,
    pub cwd: Option<String>,
    pub agent: String,
    pub pane: Option<String>,
    pub transcript: Option<String>,
    pub detail: String,
    pub question: Option<String>,
    pub updated_at: i64,
}

impl Session {
    pub fn title(&self) -> String {
        self.project
            .clone()
            .or_else(|| self.cwd.clone())
            .unwrap_or_else(|| self.id.chars().take(8).collect())
    }
}

/// Чем закончился `stop-failure` — правила настольного `classify_failure`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    Limit,
    Billing,
    Overloaded,
    Transient,
}

/// Служебные поля: смотреть в них — гадать по путям и идентификаторам.
/// Идентификатор сессии шестнадцатеричный, и «429» в нём — обычное дело.
const STRUCTURAL: &[&str] = &[
    "session_id",
    "cwd",
    "transcript_path",
    "tmux_pane",
    "pane",
    "hook_event_name",
    "notification_type",
    "permission_mode",
    "tool_name",
];

pub fn classify(payload: &Value) -> Failure {
    let Some(obj) = payload.as_object() else {
        return Failure::Transient;
    };
    let raw: String = obj
        .iter()
        .filter(|(k, _)| !STRUCTURAL.contains(&k.as_str()))
        .map(|(_, v)| v.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    let hit = |p: &str| {
        regex::RegexBuilder::new(p)
            .case_insensitive(true)
            .build()
            .map(|r| r.is_match(&raw))
            .unwrap_or(false)
    };
    if hit(r"billing|payment|insufficient|credit") {
        Failure::Billing
    } else if hit(r"rate.?limit|usage limit|quota|429|limit reached|limit_exceeded") {
        Failure::Limit
    } else if hit(r"overload|503|529|capacity") {
        Failure::Overloaded
    } else {
        Failure::Transient
    }
}

/// «Ждёт твоего ввода» — это конец работы, а не вопрос.
fn is_question(event: &str, payload: &Value) -> bool {
    if event == "permission" {
        return true;
    }
    if event != "notification" {
        return false;
    }
    if let Some(t) = payload.get("notification_type").and_then(Value::as_str) {
        return t == "permission_prompt" || t == "elicitation_dialog";
    }
    let msg = payload.get("message").and_then(Value::as_str).unwrap_or("");
    !msg.trim().is_empty()
        && !regex::RegexBuilder::new("waiting for your input")
            .case_insensitive(true)
            .build()
            .map(|r| r.is_match(msg))
            .unwrap_or(false)
}

fn status_for(event: &str, asks: bool, prev: Status, payload: &Value) -> Status {
    match event {
        "prompt" | "pre-tool" | "post-tool" | "session-start" => Status::Working,
        _ if asks => Status::Waiting,
        "notification" => Status::Done,
        // Сорванный ход — НЕ «закончил»: работа стоит, и человек, увидев
        // «закончила», отложил бы её в полной уверенности.
        "stop-failure" => {
            if classify(payload) == Failure::Limit {
                Status::Limit
            } else {
                Status::Failed
            }
        }
        "stop" => Status::Done,
        _ => prev,
    }
}

fn detail_for(event: &str, asks: bool, payload: &Value, prev: &str) -> String {
    let str_of = |k: &str| payload.get(k).and_then(Value::as_str).unwrap_or_default();
    if asks {
        let m = str_of("message");
        return if m.is_empty() {
            "спрашивает".into()
        } else {
            ellipsize(&one_line(m), 120)
        };
    }
    match event {
        "prompt" => {
            let p = str_of("prompt");
            if p.is_empty() {
                prev.to_string()
            } else {
                ellipsize(&one_line(p), 120)
            }
        }
        "pre-tool" => {
            let t = str_of("tool_name");
            if t.is_empty() {
                prev.to_string()
            } else {
                format!("выполняет {t}")
            }
        }
        // Итог берём из самого события: последняя реплика агента и есть то,
        // чем он закончил. Пересказ прошлого хода рядом со значком «готово»
        // («выполняет Bash») читается как ложь, а чужая старая сводка — хуже
        // молчания, поэтому берём только то, что приехало вместе со стопом.
        "stop" => {
            let m = str_of("last_assistant_message");
            let head = m
                .lines()
                .map(|l| l.trim().trim_matches(['#', '*', ' ']))
                .find(|l| !l.is_empty())
                .unwrap_or_default();
            if head.is_empty() {
                "готово".into()
            } else {
                ellipsize(&one_line(head), 120)
            }
        }
        "stop-failure" => match classify(payload) {
            Failure::Limit => "лимит использования — до сброса окна не продолжится".into(),
            Failure::Billing => "ошибка биллинга".into(),
            Failure::Overloaded => "API перегружен — можно повторить".into(),
            Failure::Transient => "ход прервался ошибкой".into(),
        },
        _ => prev.to_string(),
    }
}

/// Применить партию событий к реестру.
pub fn apply(current: &HashMap<String, Session>, batch: &[Recorded]) -> HashMap<String, Session> {
    let mut out = current.clone();
    for rec in batch {
        let env = &rec.envelope;
        let payload = env.get("payload").cloned().unwrap_or(Value::Null);
        let Some(sid) = payload
            .get("session_id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        let event = env.get("event").and_then(Value::as_str).unwrap_or_default();
        if event == "session-end" {
            out.remove(sid);
            continue;
        }
        let prev = out.get(sid).cloned().unwrap_or_else(|| Session {
            id: sid.into(),
            ..Default::default()
        });
        let asks = is_question(event, &payload);
        let cwd = payload
            .get("cwd")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or(prev.cwd.clone());
        let question = if asks {
            payload
                .get("message")
                .or_else(|| payload.get("prompt"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .or(prev.question.clone())
                .or_else(|| Some("ждёт ответа".into()))
        } else if matches!(event, "prompt" | "post-tool" | "stop" | "notification") {
            None
        } else {
            prev.question.clone()
        };
        out.insert(
            sid.to_string(),
            Session {
                id: sid.to_string(),
                status: status_for(event, asks, prev.status, &payload),
                project: cwd
                    .as_deref()
                    .map(|c| {
                        c.trim_end_matches('/')
                            .rsplit('/')
                            .next()
                            .unwrap_or(c)
                            .to_string()
                    })
                    .filter(|p| !p.is_empty())
                    .or(prev.project.clone()),
                cwd,
                agent: env
                    .get("agent")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .filter(|a| !a.is_empty())
                    .unwrap_or_else(|| {
                        if prev.agent.is_empty() {
                            "claude".into()
                        } else {
                            prev.agent.clone()
                        }
                    }),
                pane: env
                    .get("tmux_pane")
                    .and_then(Value::as_str)
                    .filter(|p| !p.is_empty())
                    .map(str::to_string)
                    .or(prev.pane.clone()),
                transcript: payload
                    .get("transcript_path")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or(prev.transcript.clone()),
                detail: detail_for(event, asks, &payload, &prev.detail),
                question,
                updated_at: rec.at.max(prev.updated_at),
            },
        );
    }
    out
}

/// Порядок списка: требующие человека сверху, дальше — по свежести.
pub fn sorted(reg: &HashMap<String, Session>) -> Vec<Session> {
    let mut v: Vec<Session> = reg.values().cloned().collect();
    v.sort_by(|a, b| {
        a.status
            .rank()
            .cmp(&b.status.rank())
            .then(b.updated_at.cmp(&a.updated_at))
    });
    v
}

/// Сколько чего: для строки состояния и уведомлений.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Tally {
    pub waiting: usize,
    pub working: usize,
    pub done: usize,
    pub stuck: usize,
}

pub fn tally(list: &[Session]) -> Tally {
    let mut t = Tally::default();
    for s in list {
        // Категория ровно одна: вопрос снимается только ответом, и сессия с
        // висящим вопросом попадала бы разом и в «ждёт», и в «работает».
        if s.status == Status::Waiting || s.question.is_some() {
            t.waiting += 1;
        } else if matches!(s.status, Status::Limit | Status::Failed) {
            t.stuck += 1;
        } else if s.status == Status::Working {
            t.working += 1;
        } else if s.status == Status::Done {
            t.done += 1;
        }
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    /// «Закончила» рядом с «выполняет Bash» — противоречие на одной строке.
    /// Итог берём из последней реплики агента, приехавшей с тем же событием.
    #[test]
    fn finishing_says_what_was_finished() {
        let reg = apply(
            &HashMap::new(),
            &recs(&[
                r#"{"event":"pre-tool","payload":{"session_id":"a","tool_name":"Bash"}}"#,
                r#"{"event":"stop","payload":{"session_id":"a",
                    "last_assistant_message":"**Готово.**\n\nТесты зелёные, 88 штук."}}"#,
            ]),
        );
        let s = &reg["a"];
        assert_eq!(s.status, Status::Done);
        assert_eq!(
            s.detail, "Готово.",
            "итог — из самого события, не пересказ прошлого хода"
        );
    }

    /// Молчаливый стоп бывает: агент закончил инструментом. Врать нельзя, но и
    /// оставлять «выполняет Bash» под значком готовности — тоже.
    #[test]
    fn silent_finish_says_ready_not_the_last_tool() {
        let reg = apply(
            &HashMap::new(),
            &recs(&[
                r#"{"event":"pre-tool","payload":{"session_id":"a","tool_name":"Bash"}}"#,
                r#"{"event":"stop","payload":{"session_id":"a"}}"#,
            ]),
        );
        assert_eq!(reg["a"].detail, "готово");
    }

    fn rec(body: &str, at: i64) -> Recorded {
        Recorded {
            cursor: at as u64,
            at,
            envelope: serde_json::from_str(body).unwrap(),
        }
    }

    /// Партия событий по порядку — время идёт от 1 и дальше.
    fn recs(bodies: &[&str]) -> Vec<Recorded> {
        bodies
            .iter()
            .enumerate()
            .map(|(i, b)| rec(b, i as i64 + 1))
            .collect()
    }

    #[test]
    fn question_rises_and_is_cleared_by_the_answer() {
        let reg = apply(
            &HashMap::new(),
            &[rec(
                r#"{"event":"prompt","agent":"claude","tmux_pane":"%3",
                      "payload":{"session_id":"a","cwd":"/home/bob/proj","prompt":"сделай"}}"#,
                1,
            )],
        );
        assert_eq!(reg["a"].status, Status::Working);
        assert_eq!(reg["a"].project.as_deref(), Some("proj"));

        let reg = apply(
            &reg,
            &[rec(
                r#"{"event":"notification",
            "payload":{"session_id":"a","message":"Продолжить?"}}"#,
                2,
            )],
        );
        assert_eq!(reg["a"].status, Status::Waiting);
        assert_eq!(reg["a"].question.as_deref(), Some("Продолжить?"));

        let reg = apply(
            &reg,
            &[rec(
                r#"{"event":"prompt","payload":{"session_id":"a","prompt":"да"}}"#,
                3,
            )],
        );
        assert!(reg["a"].question.is_none(), "ответ снимает вопрос");
    }

    #[test]
    fn idle_notification_is_not_a_question() {
        let reg = apply(
            &HashMap::new(),
            &[rec(
                r#"{"event":"notification",
                "payload":{"session_id":"a","message":"Claude is waiting for your input"}}"#,
                1,
            )],
        );
        // «Ждёт ввода» — это «закончил, твой ход», а не вопрос с вариантами.
        assert_eq!(reg["a"].status, Status::Done);
        assert!(reg["a"].question.is_none());
    }

    #[test]
    fn broken_turn_is_not_reported_as_finished() {
        let limit = apply(
            &HashMap::new(),
            &[rec(
                r#"{"event":"stop-failure","payload":{"session_id":"a","error":"429 rate_limit_error"}}"#,
                1,
            )],
        );
        assert_eq!(limit["a"].status, Status::Limit);

        let overload = apply(
            &HashMap::new(),
            &[rec(
                r#"{"event":"stop-failure","payload":{"session_id":"a","error":"API is overloaded (529)"}}"#,
                1,
            )],
        );
        assert_eq!(overload["a"].status, Status::Failed);

        // Служебные поля в разборе не участвуют: «429» в id и каталог
        // credit-scoring не должны объявляться лимитом и биллингом.
        let innocent = apply(
            &HashMap::new(),
            &[rec(
                r#"{"event":"stop-failure","payload":{"session_id":"a429f00d",
                "cwd":"/home/bob/credit-scoring","error":"connection closed"}}"#,
                1,
            )],
        );
        assert_eq!(innocent["a429f00d"].status, Status::Failed);
    }

    #[test]
    fn session_end_removes_the_row() {
        let reg = apply(
            &HashMap::new(),
            &[rec(r#"{"event":"prompt","payload":{"session_id":"a"}}"#, 1)],
        );
        let reg = apply(
            &reg,
            &[rec(
                r#"{"event":"session-end","payload":{"session_id":"a"}}"#,
                2,
            )],
        );
        assert!(reg.is_empty());
    }

    #[test]
    fn sorting_puts_the_ones_needing_you_first() {
        let mk = |id: &str, st: Status, at: i64| Session {
            id: id.into(),
            status: st,
            updated_at: at,
            ..Default::default()
        };
        let mut reg = HashMap::new();
        for s in [
            mk("work", Status::Working, 100),
            mk("ask", Status::Waiting, 1),
            mk("done", Status::Done, 200),
            mk("limit", Status::Limit, 50),
        ] {
            reg.insert(s.id.clone(), s);
        }
        let order: Vec<String> = sorted(&reg).into_iter().map(|s| s.id).collect();
        assert_eq!(order, ["ask", "limit", "work", "done"]);
    }

    #[test]
    fn tally_counts_each_session_once() {
        let stuck = Session {
            id: "1".into(),
            status: Status::Working,
            question: Some("q".into()),
            ..Default::default()
        };
        let t = tally(&[stuck]);
        assert_eq!(
            (t.waiting, t.working),
            (1, 0),
            "вопрос важнее работы и не двоится"
        );
    }
}
