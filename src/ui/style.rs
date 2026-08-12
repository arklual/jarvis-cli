//! Оформление вывода: одна краска, честная ширина, уважение к терминалу.
//!
//! Правило то же, что в панели и на телефоне: краска ОДНА — зелёный клевер.
//! Состояния различаются формой, весом и насыщенностью, а не цветом: цветной
//! светофор на списке из десяти сессий превращается в шум, и человек перестаёт
//! замечать именно ту строку, ради которой открыл терминал.
//!
//! Цвет включается по возможностям терминала, а не по желанию: `NO_COLOR`,
//! `TERM=dumb` и не-TTY (пайп, `| less`, CI) обязаны получать чистый текст.

use std::io::IsTerminal;

/// Что терминал умеет. Определяется один раз при старте.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Caps {
    pub color: bool,
    pub unicode: bool,
    pub width: u16,
}

impl Default for Caps {
    fn default() -> Self {
        Self {
            color: false,
            unicode: false,
            width: 80,
        }
    }
}

impl Caps {
    pub fn detect() -> Self {
        let term = std::env::var("TERM").unwrap_or_default();
        let tty = std::io::stdout().is_terminal();
        // NO_COLOR — соглашение, которое принято уважать даже когда очень
        // хочется покрасить: https://no-color.org
        let color =
            tty && std::env::var_os("NO_COLOR").is_none() && term != "dumb" && !term.is_empty();
        let lang = format!(
            "{}{}{}",
            std::env::var("LC_ALL").unwrap_or_default(),
            std::env::var("LC_CTYPE").unwrap_or_default(),
            std::env::var("LANG").unwrap_or_default()
        )
        .to_uppercase();
        Self {
            color,
            unicode: lang.contains("UTF"),
            width: term_width(),
        }
    }
}

fn term_width() -> u16 {
    if let Ok(c) = std::env::var("COLUMNS") {
        if let Ok(n) = c.trim().parse::<u16>() {
            if n >= 20 {
                return n;
            }
        }
    }
    crossterm::terminal::size()
        .map(|(w, _)| w)
        .unwrap_or(80)
        .max(20)
}

/// Роль текста. Ролей мало намеренно: каждая новая роль — это ещё один способ
/// нарисовать «важно», а важным должно оставаться одно.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Role {
    /// Обычный текст.
    Plain,
    /// Приглушённое: подписи, время, пути.
    Dim,
    /// Главное на экране — единственная краска.
    Accent,
    /// Заголовок: вес, а не цвет.
    Head,
    /// Настоящая беда. Красное — только здесь, иначе оно обесценится.
    Bad,
}

pub fn paint(caps: &Caps, role: Role, text: &str) -> String {
    if !caps.color || text.is_empty() {
        return text.to_string();
    }
    let code = match role {
        Role::Plain => return text.to_string(),
        Role::Dim => "2",
        Role::Accent => "38;5;35", // клевер
        Role::Head => "1",
        Role::Bad => "38;5;131", // приглушённый кирпич, не алый
    };
    format!("\x1b[{code}m{text}\x1b[0m")
}

/// Видимая ширина строки: без ANSI-кодов и с учётом широких символов.
///
/// Без этого любая таблица с эмодзи или CJK разъезжается, а строка с краской
/// считается длиннее, чем выглядит.
pub fn width(s: &str) -> usize {
    let mut w = 0usize;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // ANSI-последовательность до финальной буквы
            for e in chars.by_ref() {
                if e.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        w += char_width(c);
    }
    w
}

/// Ширина символа в ячейках терминала.
fn char_width(c: char) -> usize {
    let u = c as u32;
    if u == 0 || (0x300..0x370).contains(&u) {
        return 0; // управляющие и комбинирующие диакритики
    }
    let wide = (0x1100..=0x115F).contains(&u)
        || (0x2E80..=0xA4CF).contains(&u)  // CJK
        || (0xAC00..=0xD7A3).contains(&u)  // хангыль
        || (0xF900..=0xFAFF).contains(&u)
        || (0xFF00..=0xFF60).contains(&u)  // полноширинные формы
        || (0xFFE0..=0xFFE6).contains(&u)
        || (0x1F300..=0x1F64F).contains(&u) // эмодзи
        || (0x1F900..=0x1F9FF).contains(&u);
    if wide {
        2
    } else {
        1
    }
}

/// Обрезать по видимой ширине, добавив многоточие.
pub fn truncate(s: &str, max: usize) -> String {
    if width(s) <= max {
        return s.to_string();
    }
    if max <= 1 {
        return "…".into();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = char_width(c);
        if w + cw > max - 1 {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

/// Дополнить пробелами до ширины (по видимой длине, не по байтам).
pub fn pad(s: &str, to: usize) -> String {
    let w = width(s);
    if w >= to {
        return s.to_string();
    }
    format!("{s}{}", " ".repeat(to - w))
}

/// Значок статуса: форма, а не цвет. В ASCII-терминале — свои знаки.
pub fn dot(caps: &Caps, kind: &str) -> String {
    let g = |uni: &str, ascii: &str| if caps.unicode { uni } else { ascii }.to_string();
    match kind {
        "waiting" => paint(caps, Role::Accent, &g("◆", "?")),
        "working" => paint(caps, Role::Plain, &g("●", "*")),
        "done" => paint(caps, Role::Dim, &g("○", "o")),
        "stuck" => paint(caps, Role::Bad, &g("■", "!")),
        _ => paint(caps, Role::Dim, &g("·", ".")),
    }
}

/// Полоска-мера: `████░░░░░░ 42%`. Заполнение — формой, тревога — краской, но
/// только на настоящей границе.
pub fn meter(caps: &Caps, pct: u8, cells: usize) -> String {
    let pct = pct.min(100);
    let filled = (pct as usize * cells + 50) / 100;
    let (f, e) = if caps.unicode {
        ("█", "░")
    } else {
        ("#", ".")
    };
    let bar = format!(
        "{}{}",
        f.repeat(filled),
        e.repeat(cells.saturating_sub(filled))
    );
    let role = if pct > 90 {
        Role::Bad
    } else if pct > 75 {
        Role::Accent
    } else {
        Role::Dim
    };
    paint(caps, role, &bar)
}

/// Заголовок раздела: имя и линия до края. Линия — единственное украшение,
/// которое себя оправдывает: она отделяет разделы, не крича.
pub fn rule(caps: &Caps, title: &str) -> String {
    let line = if caps.unicode { "─" } else { "-" };
    let head = paint(caps, Role::Head, title);
    let used = width(&head) + 1;
    let rest = (caps.width as usize).saturating_sub(used + 1);
    format!("{head} {}", paint(caps, Role::Dim, &line.repeat(rest)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(color: bool, unicode: bool) -> Caps {
        Caps {
            color,
            unicode,
            width: 80,
        }
    }

    #[test]
    fn no_color_terminal_gets_clean_text() {
        let c = caps(false, true);
        assert_eq!(paint(&c, Role::Accent, "готово"), "готово");
        assert!(!dot(&c, "waiting").contains('\x1b'));
    }

    #[test]
    fn width_ignores_ansi_and_counts_wide_chars() {
        assert_eq!(width("абв"), 3);
        assert_eq!(width("\x1b[2mабв\x1b[0m"), 3, "краска не занимает места");
        assert_eq!(width("日本"), 4, "иероглифы — две ячейки");
    }

    #[test]
    fn truncate_respects_visible_width() {
        assert_eq!(truncate("привет, мир", 7), "привет…");
        assert_eq!(truncate("коротко", 20), "коротко");
        // Обрезка не должна рвать многобайтовый символ.
        assert!(truncate("日本語テキスト", 5).chars().count() <= 5);
    }

    #[test]
    fn pad_aligns_by_visible_width() {
        assert_eq!(pad("аб", 5), "аб   ");
        assert_eq!(
            width(&pad("\x1b[2mаб\x1b[0m", 5)),
            5,
            "крашеное выравнивается верно"
        );
    }

    #[test]
    fn meter_fills_proportionally() {
        let c = caps(false, true);
        assert_eq!(meter(&c, 0, 10), "░░░░░░░░░░");
        assert_eq!(meter(&c, 100, 10), "██████████");
        assert_eq!(meter(&c, 50, 10), "█████░░░░░");
        assert_eq!(width(&meter(&c, 42, 10)), 10, "ширина постоянна");
    }

    #[test]
    fn ascii_terminal_never_gets_unicode() {
        let c = caps(false, false);
        assert_eq!(meter(&c, 50, 4), "##..");
        assert_eq!(dot(&c, "waiting"), "?");
        assert!(rule(&c, "Сессии").chars().all(|ch| ch != '─'));
    }
}
