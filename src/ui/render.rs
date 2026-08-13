//! Как выглядит вывод: строки списков, панели, лимиты.
//!
//! Слой чистых функций: на входе данные, на выходе строки. Никакого ввода-
//! вывода — поэтому всё, что видит человек, проверяется тестами, а не глазами
//! на живом терминале.

use crate::core::session::{Session, Status, Tally};
use crate::core::state::{Bundle, HandState, Iteration, Loop, Run, Verdict};
use crate::core::util::{clock, ellipsize, plural, when};
use crate::ui::style::{
    band, bold, dot, level, meter, pad, paint, truncate, width, Bg, Caps, Role,
};

/// Строка сессии: значок, проект, что происходит, время.
///
/// Формат один на все режимы вывода — человек читает список глазами, а не
/// разбирает колонки: имя выровнено, подробность занимает остаток, время
/// прижато вправо.
pub fn session_row(caps: &Caps, s: &Session, name_col: usize) -> String {
    let kind = match s.status {
        Status::Waiting => "waiting",
        Status::Working => "working",
        Status::Done => "done",
        Status::Failed | Status::Limit => "stuck",
        Status::Idle => "idle",
    };
    // Имя весом, подробность — приглушённо: в списке из десяти строк взгляд
    // ищет проект, а не пересказ последнего хода.
    let title = bold(caps, &pad(&truncate(&s.title(), name_col), name_col));
    let when = paint(caps, Role::Dim, &clock(s.updated_at));
    // Вопрос вытесняет подробность: он и есть причина смотреть на строку.
    let detail = match (&s.question, s.status) {
        (Some(q), _) => paint(caps, Role::Warn, &ellipsize(q, 200)),
        (None, Status::Failed | Status::Limit) => paint(caps, Role::Bad, &s.detail),
        (None, Status::Done) => paint(caps, Role::Muted, &s.detail),
        _ => paint(caps, Role::Muted, &s.detail),
    };
    let head = format!("{} {title} ", dot(caps, kind));
    let tail = if when.is_empty() {
        String::new()
    } else {
        format!(" {when}")
    };
    let room = (caps.width as usize)
        .saturating_sub(width(&head) + width(&tail))
        .max(10);
    format!("{head}{}{tail}", truncate(&detail, room))
}

/// Ширина колонки имени: по самому длинному, но в разумных пределах.
pub fn name_column(list: &[Session]) -> usize {
    list.iter()
        .map(|s| width(&s.title()))
        .max()
        .unwrap_or(10)
        .clamp(8, 24)
}

/// Итоговая строка: «2 ждут ответа · 1 в работе». Тишину проговариваем словом
/// — пустая строка неотличима от «ещё не смотрели».
pub fn tally_line(caps: &Caps, t: &Tally) -> String {
    let mut parts = Vec::new();
    if t.waiting > 0 {
        parts.push(paint(
            caps,
            Role::Warn,
            &format!(
                "{} ответа",
                plural(t.waiting as u64, "ждёт", "ждут", "ждут")
            ),
        ));
    }
    if t.stuck > 0 {
        parts.push(paint(
            caps,
            Role::Bad,
            &plural(t.stuck as u64, "встало", "встали", "встали"),
        ));
    }
    if t.working > 0 {
        parts.push(paint(
            caps,
            Role::Accent,
            &format!("{} в работе", t.working),
        ));
    }
    if t.done > 0 {
        parts.push(paint(
            caps,
            Role::Muted,
            &plural(t.done as u64, "закончило", "закончили", "закончили"),
        ));
    }
    if parts.is_empty() {
        return paint(caps, Role::Dim, "тихо");
    }
    parts.join(" · ")
}

/// Окно лимита: `5ч ████░░ 62% · до 21:59`.
pub struct Window {
    pub label: String,
    pub pct: u8,
    pub reset_at: i64,
}

/// Полоска лимитов. Планка — по УЗКОМУ месту: сессия бывает полупустой, когда
/// неделя уже упирается в стену, и строка с одной сессией врала бы спокойствием.
pub fn limits_line(caps: &Caps, windows: &[Window]) -> String {
    if windows.is_empty() {
        return paint(caps, Role::Dim, "лимиты недоступны");
    }
    let worst = windows.iter().max_by_key(|w| w.pct).unwrap();
    // Каждое окно — своей краской по тому, насколько оно близко к стене; сами
    // подписи приглушены, чтобы в глаза бросалось только число у границы.
    let mut parts: Vec<String> = windows
        .iter()
        .map(|w| {
            format!(
                "{} {}",
                paint(caps, Role::Dim, &w.label),
                paint(caps, level(w.pct), &format!("{}%", w.pct))
            )
        })
        .collect();
    if worst.reset_at > crate::core::util::now_ms() {
        parts.push(paint(
            caps,
            Role::Dim,
            &format!("до {}", when(worst.reset_at)),
        ));
    }
    format!(
        "{} {}",
        meter(caps, worst.pct, 10),
        parts.join(&paint(caps, Role::Border, " · "))
    )
}

/// Одно окно отдельной строкой: `5ч   ██████░░░░  57%  до 20:59`.
///
/// Подпись здесь одна — слева. В строке-полоске подписи идут в хвосте, и
/// печатать обе рядом («5ч ███ 5ч 57%») — верный способ выглядеть небрежно.
pub fn limit_row(caps: &Caps, w: &Window, label_col: usize) -> String {
    let reset = if w.reset_at > crate::core::util::now_ms() {
        format!("  до {}", when(w.reset_at))
    } else {
        String::new()
    };
    format!(
        "{} {}  {}{}",
        pad(&truncate(&w.label, label_col), label_col),
        meter(caps, w.pct, 12),
        paint(caps, level(w.pct), &format!("{:>3}%", w.pct)),
        paint(caps, Role::Dim, &reset)
    )
}

/* ---------- циклы ---------- */

/// Строка цикла в списке: имя, расписание, чем занят.
pub fn loop_row(caps: &Caps, l: &Loop, run: Option<&Run>, name_col: usize) -> String {
    let state = match run {
        None => "не запускался".to_string(),
        Some(r) => match r.state {
            crate::core::state::RunState::Running => {
                let n = r.iterations.len();
                format!("идёт · итерация {n}")
            }
            crate::core::state::RunState::Asking => "спрашивает".into(),
            crate::core::state::RunState::Done => "завершён".into(),
            crate::core::state::RunState::Stopped => r.stop.word().to_string(),
            crate::core::state::RunState::Idle => "ждёт".into(),
        },
    };
    let waiting = run.map(|r| r.pending_review()).unwrap_or(0);
    let kind = match run.map(|r| r.state) {
        Some(crate::core::state::RunState::Asking) => "waiting",
        Some(crate::core::state::RunState::Running) => "working",
        Some(crate::core::state::RunState::Stopped) => "stuck",
        Some(crate::core::state::RunState::Done) => "done",
        _ => "idle",
    };
    let mut tail = vec![paint(caps, Role::Dim, &l.wake_label())];
    // Машина в строке только когда она не эта: «local» у каждого цикла — шум.
    if !l.machine.trim().is_empty() && l.machine != "local" {
        tail.push(paint(caps, Role::Accent, &l.machine));
    }
    if waiting > 0 {
        tail.push(paint(
            caps,
            Role::Accent,
            &format!("{waiting} ждёт взгляда"),
        ));
    }
    // Состояние и расписание разделяем точкой: без неё «не запускался только
    // руками» читается как одна невнятная фраза.
    let sep = paint(caps, Role::Border, " · ");
    let rest = format!("{sep}{}", tail.join(&sep));
    format!(
        "{} {} {}{}",
        dot(caps, kind),
        pad(&truncate(&l.name, name_col), name_col),
        paint(
            caps,
            if kind == "stuck" {
                Role::Bad
            } else {
                Role::Plain
            },
            &state
        ),
        rest
    )
}

/// Строка итерации в журнале.
pub fn iteration_row(caps: &Caps, it: &Iteration) -> String {
    let (word, role) = match it.verdict {
        Verdict::Running => ("идёт", Role::Plain),
        Verdict::Passed => ("прошла", Role::Dim),
        Verdict::Returned => ("возврат критика", Role::Dim),
        Verdict::GateFailed => ("красный гейт", Role::Bad),
        Verdict::Failed => ("сорвалась", Role::Bad),
    };
    let mark = if it.sampled && !it.reviewed {
        paint(caps, Role::Accent, " · посмотри")
    } else {
        String::new()
    };
    let head = format!("{:>3}  {}  ", it.n, paint(caps, role, &pad(word, 16)));
    let room = (caps.width as usize).saturating_sub(width(&head) + width(&mark) + 10);
    let line = format!(
        "{head}{}{mark}  {}",
        truncate(&it.summary, room.max(10)),
        paint(caps, Role::Dim, &fmt_tokens(it.tokens))
    );
    // Сорвавшаяся итерация — полосой: в журнале из двадцати строк её ищут
    // глазами, и красное слово в общем ряду теряется.
    if matches!(it.verdict, Verdict::GateFailed | Verdict::Failed) {
        band(caps, Bg::Bad, &line, caps.width as usize)
    } else {
        format!(" {line}")
    }
}

pub fn fmt_tokens(n: u64) -> String {
    if n >= 1000 {
        format!("{}k", (n as f64 / 1000.0).round() as u64)
    } else {
        n.to_string()
    }
}

/* ---------- связка ---------- */

/// Строка руки на пульте связки.
pub fn hand_row(caps: &Caps, b: &Bundle, hand_id: &str, name_col: usize) -> String {
    let Some(h) = b.hands.iter().find(|h| h.id == hand_id) else {
        return String::new();
    };
    let queue_pos = b.queue().iter().position(|q| q.id == h.id).map(|p| p + 1);
    let kind = match h.state {
        HandState::Ready => "waiting",
        HandState::Working => "working",
        HandState::Conflict | HandState::Failed => "stuck",
        HandState::Merged => "done",
        HandState::New => "idle",
    };
    let line = match h.state {
        HandState::Ready => format!(
            "готов к мержу · очередь #{}",
            queue_pos
                .map(|p| p.to_string())
                .unwrap_or_else(|| "?".into())
        ),
        HandState::Conflict => format!(
            "конфликт: {} · чинит сам, попытка {}",
            h.conflict_files.join(", "),
            h.attempt
        ),
        _ => h.state.word().to_string(),
    };
    let role = match h.state {
        HandState::Conflict | HandState::Failed => Role::Bad,
        HandState::Ready => Role::Accent,
        _ => Role::Plain,
    };
    format!(
        "{} {} {}  {}",
        dot(caps, kind),
        pad(&truncate(&h.name, name_col), name_col),
        paint(caps, role, &line),
        paint(caps, Role::Dim, &h.branch)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Подпись окна печатается один раз: в строке — слева, и больше нигде.
    #[test]
    fn limit_row_names_the_window_once() {
        let c = Caps {
            color: false,
            truecolor: false,
            unicode: true,
            width: 80,
        };
        let w = Window {
            label: "5ч".into(),
            pct: 57,
            reset_at: 0,
        };
        let line = limit_row(&c, &w, 6);
        assert_eq!(line.matches("5ч").count(), 1, "подпись задвоилась: {line}");
        assert!(line.contains("57%"));
    }
    use crate::core::state::{Hand, RunState};

    fn caps() -> Caps {
        Caps {
            color: false,
            truecolor: false,
            unicode: true,
            width: 80,
        }
    }

    fn sess(title: &str, st: Status) -> Session {
        Session {
            id: "s".into(),
            project: Some(title.into()),
            status: st,
            detail: "делает что-то полезное".into(),
            updated_at: 1_700_000_000_000,
            ..Default::default()
        }
    }

    #[test]
    fn session_row_fits_the_terminal_exactly() {
        let c = caps();
        let mut s = sess("проект", Status::Working);
        s.detail = "очень длинная подробность ".repeat(20);
        let row = session_row(&c, &s, 10);
        assert!(
            width(&row) <= c.width as usize,
            "строка вылезла: {}",
            width(&row)
        );
    }

    #[test]
    fn question_wins_over_detail() {
        let c = caps();
        let mut s = sess("проект", Status::Waiting);
        s.question = Some("Снимать ли тест?".into());
        let row = session_row(&c, &s, 10);
        assert!(row.contains("Снимать ли тест?"));
        assert!(!row.contains("полезное"), "вопрос вытесняет подробность");
    }

    #[test]
    fn tally_speaks_silence_out_loud() {
        let c = caps();
        assert_eq!(tally_line(&c, &Tally::default()), "тихо");
        let t = Tally {
            waiting: 2,
            working: 1,
            done: 0,
            stuck: 1,
        };
        let line = tally_line(&c, &t);
        // Числительное согласовано: «2 ждёт» выдаёт наспех собранную строку.
        assert!(line.starts_with("2 ждут ответа"), "{line}");
        assert!(line.contains("1 встало") && line.contains("1 в работе"));
        let one = tally_line(
            &c,
            &Tally {
                waiting: 1,
                ..Default::default()
            },
        );
        assert!(one.starts_with("1 ждёт ответа"), "{one}");
    }

    #[test]
    fn limits_take_the_narrow_window() {
        let c = caps();
        let line = limits_line(
            &c,
            &[
                Window {
                    label: "5ч".into(),
                    pct: 62,
                    reset_at: 0,
                },
                Window {
                    label: "нед".into(),
                    pct: 94,
                    reset_at: 0,
                },
            ],
        );
        // Планка по 94%, но видны оба окна.
        assert!(
            line.contains("5ч 62%") && line.contains("нед 94%"),
            "{line}"
        );
        assert!(
            line.starts_with("█████████"),
            "мера по узкому месту: {line}"
        );
        assert_eq!(limits_line(&c, &[]), "лимиты недоступны");
    }

    #[test]
    fn loop_row_shows_what_needs_the_human() {
        let c = caps();
        let l = Loop {
            name: "ночной test-fix".into(),
            ..Default::default()
        };
        let run = Run {
            state: RunState::Running,
            iterations: vec![Iteration {
                n: 1,
                sampled: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        let row = loop_row(&c, &l, Some(&run), 16);
        assert!(row.contains("итерация 1"));
        assert!(
            row.contains("1 ждёт взгляда"),
            "выборка обязана звать: {row}"
        );
    }

    #[test]
    fn iteration_row_marks_the_sampled_one() {
        let c = caps();
        let it = Iteration {
            n: 7,
            verdict: Verdict::Passed,
            summary: "починил флаки-тест".into(),
            tokens: 21_000,
            sampled: true,
            ..Default::default()
        };
        let row = iteration_row(&c, &it);
        assert!(row.contains("посмотри") && row.contains("21k"));
        assert!(width(&row) <= c.width as usize);
    }

    #[test]
    fn hand_row_tells_the_conflict_in_human_words() {
        let c = caps();
        let b = Bundle {
            hands: vec![Hand {
                id: "h".into(),
                name: "платёжка".into(),
                branch: "team/billing".into(),
                state: HandState::Conflict,
                conflict_files: vec!["shared/types.ts".into()],
                attempt: 2,
                ..Default::default()
            }],
            ..Default::default()
        };
        let row = hand_row(&c, &b, "h", 12);
        assert!(
            row.contains("shared/types.ts") && row.contains("попытка 2"),
            "{row}"
        );
    }

    #[test]
    fn queue_position_is_shown_for_ready_hands() {
        let c = caps();
        let mk = |id: &str, at: i64| Hand {
            id: id.into(),
            name: id.into(),
            state: HandState::Ready,
            ready_at: at,
            ..Default::default()
        };
        let b = Bundle {
            hands: vec![mk("b", 200), mk("a", 100)],
            ..Default::default()
        };
        assert!(hand_row(&c, &b, "a", 8).contains("#1"));
        assert!(hand_row(&c, &b, "b", 8).contains("#2"));
    }

    #[test]
    fn tokens_are_readable() {
        assert_eq!(fmt_tokens(0), "0");
        assert_eq!(fmt_tokens(999), "999");
        assert_eq!(fmt_tokens(21_400), "21k");
    }

    #[test]
    fn name_column_stays_within_reason() {
        assert_eq!(name_column(&[]), 10);
        let long = sess(&"я".repeat(80), Status::Idle);
        assert_eq!(name_column(&[long]), 24, "длинное имя не съедает экран");
    }
}
