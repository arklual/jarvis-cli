//! Живой экран и уведомления.
//!
//! Экран перерисовывается на месте — курсор в начало и очистка вниз, без
//! альт-экрана: человек уходит из `jarvis` и видит свою историю команд, а не
//! стёртый терминал. То же соображение, что у `top` против `less`.
//!
//! Уведомления печатаются НА ПЕРЕХОДАХ, а не на состоянии: иначе каждая
//! страница событий заново сообщала бы «агент закончил» про то же самое — а
//! это ровно тот вид уведомлений, после которого их выключают.

use crate::app::App;
use crate::core::machine;
use crate::core::session::{self, Session, Status};
use crate::ui::render;
use crate::ui::style::{paint, rule, Role};
use std::collections::HashMap;
use std::io::Write;
use std::time::Duration;

/// Пауза между кругами. Узел держит long-poll сам, так что это пауза только
/// на случай пустых ответов.
const IDLE: Duration = Duration::from_millis(900);

/// Событие для человека: что изменилось.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub session: String,
    pub kind: Status,
    pub text: String,
}

/// Сравнить два снимка реестра и выдать то, о чём стоит сказать.
///
/// Только переходы и только те, что человеку важны: спросил, закончил, встал.
/// Начало работы событием не считается — иначе лента станет шумом.
pub fn changes(before: &HashMap<String, Session>, after: &HashMap<String, Session>) -> Vec<Change> {
    let mut out = Vec::new();
    for (id, now) in after {
        let was = before.get(id).map(|s| s.status);
        if was == Some(now.status) {
            continue;
        }
        let text = match now.status {
            Status::Waiting => now.question.clone().unwrap_or_else(|| "ждёт ответа".into()),
            Status::Done => {
                if now.detail.is_empty() {
                    "работа закончена".into()
                } else {
                    now.detail.clone()
                }
            }
            Status::Limit | Status::Failed => now.detail.clone(),
            _ => continue, // начало работы — не новость
        };
        out.push(Change {
            session: now.title(),
            kind: now.status,
            text,
        });
    }
    out.sort_by_key(|c| c.kind.rank());
    out
}

/// Одна строка уведомления — её же удобно скармливать `say`/`notify-send`.
pub fn notify_line(c: &Change) -> String {
    let word = match c.kind {
        Status::Waiting => "спрашивает",
        Status::Done => "закончил",
        Status::Limit => "упёрся в лимит",
        Status::Failed => "сорвался",
        _ => "изменился",
    };
    format!(
        "{} — {word}: {}",
        c.session,
        crate::core::util::ellipsize(&c.text, 160)
    )
}

/// Живой экран: перерисовываем список, пока не нажали q или Ctrl-C.
pub async fn run(app: &App, machine_name: &str) -> Result<(), String> {
    let m = machine::list()
        .into_iter()
        .find(|m| m.name == machine_name)
        .ok_or_else(|| format!("нет машины «{machine_name}»"))?;
    let (client, _tunnel) = machine::connect(&m).await?;

    // Сырой режим — только чтобы поймать «q» без Enter. Восстанавливается в
    // Drop, иначе брошенный терминал остался бы без эха.
    let raw = RawGuard::enable();
    let mut reg = crate::app::registry(&client).await.unwrap_or_default();
    let mut cursor = client.hello().await.map(|h| h.cursor).unwrap_or(0);
    let mut recent: Vec<Change> = Vec::new();
    // Полоска лимитов внизу — как в панели. Узел отдаёт её из кэша, так что
    // круг экрана она не тормозит; раз в пять минут обновляем.
    let mut limits: Vec<render::Window> = Vec::new();
    let mut limits_at = 0i64;

    loop {
        if crate::core::util::now_ms() - limits_at > 5 * 60_000 {
            // Время ставим ДО запроса: если узел не умеет /usage, незачем
            // ломиться к нему каждый круг.
            limits_at = crate::core::util::now_ms();
            if let Ok(text) = client.usage_text(false).await {
                limits = crate::app::parse_usage(&text);
            }
        }
        draw(app, &m.name, &reg, &recent, &limits);
        if pressed_quit()? {
            break;
        }
        match client.events(cursor).await {
            Ok(page) => {
                cursor = page.cursor;
                if page.gap {
                    // Лента порвалась: пересобираем молча — сравнивать не с
                    // чем, и объявлять новостью всё подряд нечестно.
                    reg = crate::app::registry(&client).await.unwrap_or_default();
                } else if !page.events.is_empty() {
                    let next = session::apply(&reg, &page.events);
                    let fresh = changes(&reg, &next);
                    for c in fresh {
                        recent.push(c);
                    }
                    let extra = recent.len().saturating_sub(5);
                    recent.drain(..extra);
                    reg = next;
                }
            }
            Err(_) => tokio::time::sleep(IDLE).await,
        }
    }
    drop(raw);
    Ok(())
}

fn draw(
    app: &App,
    machine_name: &str,
    reg: &HashMap<String, Session>,
    recent: &[Change],
    limits: &[render::Window],
) {
    let list = session::sorted(reg);
    let mut out = String::new();
    // Курсор в начало и очистка вниз: перерисовка без мигания и без
    // альт-экрана, чтобы после выхода осталась история команд.
    out.push_str("\x1b[H\x1b[J");
    out.push_str(&rule(&app.caps, &format!("Jarvis · {machine_name}")));
    out.push('\n');
    if list.is_empty() {
        out.push_str(&paint(&app.caps, Role::Dim, "Ни одной сессии."));
        out.push('\n');
    } else {
        let col = render::name_column(&list);
        for s in &list {
            out.push_str(&render::session_row(&app.caps, s, col));
            out.push('\n');
        }
    }
    out.push('\n');
    out.push_str(&render::tally_line(&app.caps, &session::tally(&list)));
    out.push('\n');
    if !recent.is_empty() {
        out.push('\n');
        out.push_str(&rule(&app.caps, "недавнее"));
        out.push('\n');
        for c in recent {
            let role = if c.kind == Status::Waiting {
                Role::Accent
            } else {
                Role::Dim
            };
            out.push_str(&paint(&app.caps, role, &notify_line(c)));
            out.push('\n');
        }
    }
    out.push('\n');
    out.push_str(&render::limits_line(&app.caps, limits));
    out.push('\n');
    out.push_str(&paint(&app.caps, Role::Dim, "q — выход"));
    print!("{out}");
    let _ = std::io::stdout().flush();
}

/// Нажата ли «q». Не блокирует: экран должен обновляться и без клавиш.
fn pressed_quit() -> Result<bool, String> {
    use crossterm::event::{poll, read, Event, KeyCode};
    if !poll(Duration::from_millis(120)).map_err(|e| e.to_string())? {
        return Ok(false);
    }
    match read().map_err(|e| e.to_string())? {
        Event::Key(k) => Ok(matches!(k.code, KeyCode::Char('q') | KeyCode::Esc)),
        _ => Ok(false),
    }
}

/// Сырой режим терминала с гарантированным восстановлением.
struct RawGuard(bool);

impl RawGuard {
    fn enable() -> Self {
        Self(crossterm::terminal::enable_raw_mode().is_ok())
    }
}

impl Drop for RawGuard {
    fn drop(&mut self) {
        if self.0 {
            let _ = crossterm::terminal::disable_raw_mode();
        }
        // Курсор на новую строку: иначе приглашение шелла ляжет поверх вывода.
        println!();
    }
}

/// Печатать переходы по мере появления — для связки со звуком или notify-send.
pub async fn notify(app: &App, machine_name: &str, once: bool) -> Result<(), String> {
    let m = machine::list()
        .into_iter()
        .find(|m| m.name == machine_name)
        .ok_or_else(|| format!("нет машины «{machine_name}»"))?;
    let (client, _tunnel) = machine::connect(&m).await?;
    let mut reg = crate::app::registry(&client).await.unwrap_or_default();
    let mut cursor = client.hello().await.map(|h| h.cursor).unwrap_or(0);
    loop {
        let page = match client.events(cursor).await {
            Ok(p) => p,
            Err(e) => {
                if once {
                    return Err(e);
                }
                tokio::time::sleep(IDLE).await;
                continue;
            }
        };
        cursor = page.cursor;
        if page.gap {
            reg = crate::app::registry(&client).await.unwrap_or_default();
            continue;
        }
        if !page.events.is_empty() {
            let next = session::apply(&reg, &page.events);
            for c in changes(&reg, &next) {
                let role = if c.kind == Status::Waiting {
                    Role::Accent
                } else {
                    Role::Dim
                };
                println!("{}", paint(&app.caps, role, &notify_line(&c)));
                let _ = std::io::stdout().flush();
            }
            reg = next;
        }
        if once {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reg(list: &[(&str, Status)]) -> HashMap<String, Session> {
        list.iter()
            .map(|(id, st)| {
                (
                    id.to_string(),
                    Session {
                        id: id.to_string(),
                        project: Some(id.to_string()),
                        status: *st,
                        detail: "подробность".into(),
                        ..Default::default()
                    },
                )
            })
            .collect()
    }

    #[test]
    fn only_transitions_are_reported() {
        let before = reg(&[("a", Status::Working)]);
        let after = reg(&[("a", Status::Working)]);
        assert!(
            changes(&before, &after).is_empty(),
            "состояние без перехода — не новость"
        );
    }

    #[test]
    fn starting_work_is_not_news() {
        let before = reg(&[("a", Status::Idle)]);
        let after = reg(&[("a", Status::Working)]);
        assert!(changes(&before, &after).is_empty());
    }

    #[test]
    fn question_and_finish_and_stall_are_news() {
        let before = reg(&[
            ("a", Status::Working),
            ("b", Status::Working),
            ("c", Status::Working),
        ]);
        let mut after = reg(&[
            ("a", Status::Waiting),
            ("b", Status::Done),
            ("c", Status::Limit),
        ]);
        after.get_mut("a").unwrap().question = Some("Снимать ли тест?".into());
        let list = changes(&before, &after);
        assert_eq!(list.len(), 3);
        // Спрашивающий — первым: он и есть причина смотреть.
        assert_eq!(list[0].kind, Status::Waiting);
        assert!(list[0].text.contains("Снимать"));
    }

    #[test]
    fn notify_line_reads_like_a_sentence() {
        let c = Change {
            session: "jarvis".into(),
            kind: Status::Waiting,
            text: "Снимать ли тест?".into(),
        };
        assert_eq!(notify_line(&c), "jarvis — спрашивает: Снимать ли тест?");
    }

    #[test]
    fn finished_without_detail_still_says_something() {
        let before = reg(&[("a", Status::Working)]);
        let mut after = reg(&[("a", Status::Done)]);
        after.get_mut("a").unwrap().detail = String::new();
        assert_eq!(changes(&before, &after)[0].text, "работа закончена");
    }
}
