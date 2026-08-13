//! Оформление вывода: палитра ролями, честная ширина, уважение к терминалу.
//!
//! Здесь был закон «краска одна — зелёный клевер, остальное форма и тон». Он
//! честно защищал от светофора, но давал плоскую картинку: у всего один вес,
//! глазу не за что зацепиться, и человек читает экран строка за строкой вместо
//! того, чтобы увидеть его целиком.
//!
//! Теперь как у pi: несколько СМЫСЛОВЫХ ролей с постоянными цветами (accent,
//! success, warning, error, muted, dim) и подложки для блоков. Роль назначается
//! по значению, а не по желанию покрасить: зелёное — только «сделано», красное
//! — только «сломалось», жёлтое — «нужен человек». Светофора не выходит,
//! потому что цвет не украшение, а сам смысл строки.
//!
//! Цвет включается по возможностям терминала, а не по желанию: `NO_COLOR`,
//! `TERM=dumb` и не-TTY (пайп, `| less`, CI) обязаны получать чистый текст.
//! Где есть truecolor — берём его, иначе честно приближаем палитрой из 256.

use std::io::IsTerminal;

/// Что терминал умеет. Определяется один раз при старте.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Caps {
    pub color: bool,
    /// Терминал умеет 24 бита. Иначе цвет приближается кубом из 256.
    pub truecolor: bool,
    pub unicode: bool,
    pub width: u16,
}

impl Default for Caps {
    fn default() -> Self {
        Self {
            color: false,
            truecolor: false,
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
        let ct = std::env::var("COLORTERM")
            .unwrap_or_default()
            .to_lowercase();
        Self {
            color,
            truecolor: color && (ct.contains("truecolor") || ct.contains("24bit")),
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
    // Ноль — это не «терминал шириной ноль», а «спросить не у кого»: так
    // отвечает pty без размера (cron, script, docker exec -T). Восемьдесят
    // здесь — не догадка, а общее умолчание, с которым вывод остаётся читаемым.
    match crossterm::terminal::size().map(|(w, _)| w) {
        Ok(0) | Err(_) => 80,
        Ok(w) => w.max(20),
    }
}

/// Палитра. Значения — из темы pi: они уже подобраны так, чтобы не резать
/// глаз ни на светлом, ни на тёмном фоне, и чтобы соседние роли различались
/// при беглом взгляде, а не при сравнении.
mod ink {
    pub const TEXT: (u8, u8, u8) = (0xd4, 0xd4, 0xd4);
    pub const MUTED: (u8, u8, u8) = (0x8a, 0x8a, 0x8a);
    pub const DIM: (u8, u8, u8) = (0x66, 0x66, 0x66);
    pub const ACCENT: (u8, u8, u8) = (0x8a, 0xbe, 0xb7);
    pub const OK: (u8, u8, u8) = (0xb5, 0xbd, 0x68);
    pub const WARN: (u8, u8, u8) = (0xf0, 0xc6, 0x74);
    pub const ERR: (u8, u8, u8) = (0xcc, 0x66, 0x66);
    pub const BORDER: (u8, u8, u8) = (0x50, 0x50, 0x50);

    // Подложки. Тёмные настолько, чтобы текст поверх читался без правки цвета.
    pub const BG_HEAD: (u8, u8, u8) = (0x2a, 0x2e, 0x3a);
    pub const BG_SEL: (u8, u8, u8) = (0x3a, 0x3a, 0x4a);
    pub const BG_USER: (u8, u8, u8) = (0x34, 0x35, 0x41);
    pub const BG_TOOL: (u8, u8, u8) = (0x28, 0x28, 0x32);
    pub const BG_ERR: (u8, u8, u8) = (0x3c, 0x28, 0x28);
}

/// Роль текста — назначается по смыслу строки, а не по настроению.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Role {
    /// Обычный текст: цвет терминала, никакой краски.
    Plain,
    /// Основной текст поверх подложки, где родной цвет ненадёжен.
    Text,
    /// Подписи, пути, время — читается, но не спорит с главным.
    Muted,
    /// Совсем фон: то, что можно не читать вовсе.
    Dim,
    /// То, ради чего смотрят на экран.
    Accent,
    /// Сделано и зелено.
    Ok,
    /// Нужен человек.
    Warn,
    /// Сломалось.
    Bad,
    /// Линии и рамки.
    Border,
}

/// Подложка блока. Блок с подложкой — главная находка pi: роль строки видна
/// раньше, чем прочитан текст, и диалог перестаёт быть простынёй.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Bg {
    Head,
    Sel,
    User,
    Tool,
    Bad,
}

fn rgb(role: Role) -> Option<(u8, u8, u8)> {
    Some(match role {
        Role::Plain => return None,
        Role::Text => ink::TEXT,
        Role::Muted => ink::MUTED,
        Role::Dim => ink::DIM,
        Role::Accent => ink::ACCENT,
        Role::Ok => ink::OK,
        Role::Warn => ink::WARN,
        Role::Bad => ink::ERR,
        Role::Border => ink::BORDER,
    })
}

fn bg_rgb(bg: Bg) -> (u8, u8, u8) {
    match bg {
        Bg::Head => ink::BG_HEAD,
        Bg::Sel => ink::BG_SEL,
        Bg::User => ink::BG_USER,
        Bg::Tool => ink::BG_TOOL,
        Bg::Bad => ink::BG_ERR,
    }
}

/// Цвет в SGR: 24 бита там, где умеют, иначе ближайший из 256.
fn sgr(caps: &Caps, (r, g, b): (u8, u8, u8), background: bool) -> String {
    let layer = if background { 48 } else { 38 };
    if caps.truecolor {
        format!("{layer};2;{r};{g};{b}")
    } else {
        format!("{layer};5;{}", xterm256(r, g, b))
    }
}

/// Приближение цвета палитрой xterm-256: кубом 6×6×6 или серой лесенкой —
/// смотря что ближе. Серое важно отдельно: без лесенки все наши приглушённые
/// тона схлопнулись бы в один невнятный куб.
pub fn xterm256(r: u8, g: u8, b: u8) -> u8 {
    let (rf, gf, bf) = (r as i32, g as i32, b as i32);
    let level = |v: i32| -> i32 {
        // Уровни куба: 0, 95, 135, 175, 215, 255.
        const STEPS: [i32; 6] = [0, 95, 135, 175, 215, 255];
        let mut best = 0;
        for (i, s) in STEPS.iter().enumerate() {
            if (v - s).abs() < (v - STEPS[best as usize]).abs() {
                best = i as i32;
            }
        }
        best
    };
    let (ri, gi, bi) = (level(rf), level(gf), level(bf));
    const STEPS: [i32; 6] = [0, 95, 135, 175, 215, 255];
    let cube_err = (STEPS[ri as usize] - rf).pow(2)
        + (STEPS[gi as usize] - gf).pow(2)
        + (STEPS[bi as usize] - bf).pow(2);

    let gray = ((rf * 299 + gf * 587 + bf * 114) / 1000).clamp(0, 255);
    let gi_idx = (((gray - 8) as f32 / 10.0).round() as i32).clamp(0, 23);
    let gray_val = 8 + gi_idx * 10;
    let gray_err = (gray_val - rf).pow(2) + (gray_val - gf).pow(2) + (gray_val - bf).pow(2);

    if gray_err < cube_err {
        (232 + gi_idx) as u8
    } else {
        (16 + 36 * ri + 6 * gi + bi) as u8
    }
}

pub fn paint(caps: &Caps, role: Role, text: &str) -> String {
    if !caps.color || text.is_empty() {
        return text.to_string();
    }
    match role {
        Role::Plain => text.to_string(),
        r => match rgb(r) {
            None => text.to_string(),
            Some(c) => format!("\x1b[{}m{text}\x1b[0m", sgr(caps, c, false)),
        },
    }
}

/// Жирным — там, где важен вес, а не цвет.
pub fn bold(caps: &Caps, text: &str) -> String {
    if !caps.color || text.is_empty() {
        text.to_string()
    } else {
        format!("\x1b[1m{text}\x1b[0m")
    }
}

/// Строка на подложке во всю ширину.
///
/// Подложка обязана доходить до края: блок, оборванный по длине текста, читается
/// как случайная заливка, а не как блок. Внутри — по пробелу с боков, иначе
/// буквы упираются в границу цвета.
pub fn band(caps: &Caps, bg: Bg, text: &str, total: usize) -> String {
    let inner = total.saturating_sub(2);
    let body = format!(" {} ", pad(&truncate(text, inner), inner));
    on_bg(caps, bg, &body)
}

/// Одеть готовую строку в подложку.
///
/// Тонкость, из-за которой блоки обычно и выглядят рвано: любой покрашенный
/// кусок внутри заканчивается полным сбросом `ESC[0m`, а он гасит и фон — от
/// первого же слова подложка обрывается до конца строки. Поэтому после каждого
/// сброса фон возвращается на место.
pub fn on_bg(caps: &Caps, bg: Bg, body: &str) -> String {
    if !caps.color {
        return body.to_string();
    }
    let code = sgr(caps, bg_rgb(bg), true);
    let inner = body.replace("\x1b[0m", &format!("\x1b[0m\x1b[{code}m"));
    format!("\x1b[{code}m{inner}\x1b[0m")
}

/// Разложить текст по строкам заданной ширины, по словам.
///
/// Обрезать сообщение многоточием — значит спрятать ровно то, ради чего его
/// читают. Перенос по словам стоит десяти строк кода и возвращает смысл.
pub fn wrap(text: &str, max: usize) -> Vec<String> {
    if max == 0 {
        return vec![text.to_string()];
    }
    let mut out: Vec<String> = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split_whitespace() {
            let w = width(word);
            if line.is_empty() {
                // Слово длиннее строки (путь, ссылка) рвём по месту: иначе оно
                // одно растянет верстку и всё поедет.
                if w > max {
                    let mut rest = word.to_string();
                    while width(&rest) > max {
                        let head: String = take_width(&rest, max);
                        out.push(head.clone());
                        rest = rest[head.len()..].to_string();
                    }
                    line = rest;
                } else {
                    line = word.to_string();
                }
            } else if width(&line) + 1 + w <= max {
                line.push(' ');
                line.push_str(word);
            } else {
                out.push(std::mem::take(&mut line));
                line = word.to_string();
            }
        }
        out.push(line);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// Взять с начала строки ровно `max` ячеек, не разрывая символ.
fn take_width(s: &str, max: usize) -> String {
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = char_width(c);
        if w + cw > max {
            break;
        }
        out.push(c);
        w += cw;
    }
    out
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

/// Снять краску: строка нужна как текст — например, чтобы положить её на
/// подложку, где чужие сбросы цвета всё испортят.
pub fn strip(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\x1b' {
            for e in it.by_ref() {
                if e.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        out.push(c);
    }
    out
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

/// Значок статуса: форма И цвет. Форма — чтобы читалось без цвета, цвет —
/// чтобы «спрашивает» находилось взглядом, а не чтением всех строк подряд.
pub fn dot(caps: &Caps, kind: &str) -> String {
    let g = |uni: &str, ascii: &str| if caps.unicode { uni } else { ascii }.to_string();
    match kind {
        "waiting" => paint(caps, Role::Warn, &g("◆", "?")),
        "working" => paint(caps, Role::Accent, &g("●", "*")),
        "done" => paint(caps, Role::Ok, &g("✓", "v")),
        "stuck" => paint(caps, Role::Bad, &g("■", "!")),
        _ => paint(caps, Role::Dim, &g("·", ".")),
    }
}

/// Цвет по доле занятого: спокойный до трёх четвертей, тревожный у стены.
pub fn level(pct: u8) -> Role {
    if pct > 90 {
        Role::Bad
    } else if pct > 70 {
        Role::Warn
    } else {
        Role::Ok
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
    // Заполненная часть говорит цветом, пустая молчит: так видно и сколько
    // занято, и насколько это уже страшно.
    let split = filled.min(bar.chars().count());
    let (done, left): (String, String) = (
        bar.chars().take(split).collect(),
        bar.chars().skip(split).collect(),
    );
    format!(
        "{}{}",
        paint(caps, level(pct), &done),
        paint(caps, Role::Dim, &left)
    )
}

/// Заголовок раздела: имя и линия до края. Линия — единственное украшение,
/// которое себя оправдывает: она отделяет разделы, не крича.
pub fn rule(caps: &Caps, title: &str) -> String {
    let line = if caps.unicode { "─" } else { "-" };
    let head = bold(caps, title);
    let used = width(&head) + 1;
    let rest = (caps.width as usize).saturating_sub(used + 1);
    format!("{head} {}", paint(caps, Role::Border, &line.repeat(rest)))
}

/// Шапка окна: имя слева, сводка справа, всё на подложке во всю ширину.
///
/// Полоса, а не строка с линией: у окна должен быть верх, который видно
/// боковым зрением — тогда экран читается как приложение, а не как вывод.
pub fn header(caps: &Caps, left: &str, right: &str, total: usize) -> String {
    let inner = total.saturating_sub(2);
    let l = truncate(left, inner);
    let room = inner.saturating_sub(width(&l) + 1);
    let r = truncate(right, room);
    let gap = inner.saturating_sub(width(&l) + width(&r));
    let body = format!(
        " {}{}{} ",
        bold(caps, &l),
        " ".repeat(gap),
        paint(caps, Role::Muted, &r)
    );
    on_bg(caps, Bg::Head, &body)
}

/// Подсказка клавиши: сама клавиша выделена, объяснение приглушено.
pub fn key(caps: &Caps, k: &str, what: &str) -> String {
    format!(
        "{} {}",
        paint(caps, Role::Accent, k),
        paint(caps, Role::Dim, what)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn colored() -> Caps {
        Caps {
            color: true,
            truecolor: true,
            unicode: true,
            width: 40,
        }
    }

    /// Подложка обязана дожить до правого края. Внутренний сброс краски гасит
    /// фон — если его не восстанавливать, блок обрывается на первом же слове.
    #[test]
    fn background_survives_painted_fragments() {
        let c = colored();
        let inner = format!("{} и обычный текст", paint(&c, Role::Accent, "крашеное"));
        let line = band(&c, Bg::User, &inner, 40);
        assert_eq!(width(&line), 40, "полоса не во всю ширину");
        let bg = "48;2;52;53;65";
        assert!(line.starts_with(&format!("\x1b[{bg}m")));
        assert!(
            line.matches(bg).count() >= 2,
            "фон не вернулся после сброса краски: {line:?}"
        );
        assert!(line.ends_with("\x1b[0m"));
    }

    #[test]
    fn band_without_color_is_just_padded_text() {
        let c = Caps {
            color: false,
            truecolor: false,
            unicode: true,
            width: 40,
        };
        let line = band(&c, Bg::User, "привет", 20);
        assert_eq!(line, " привет             ");
        assert!(!line.contains('\x1b'));
    }

    #[test]
    fn header_puts_the_summary_on_the_right_edge() {
        let c = colored();
        let h = header(&c, "Jarvis · local", "2 в работе", 40);
        assert_eq!(width(&h), 40);
        let plain = strip(&h);
        assert!(plain.starts_with(" Jarvis"), "{plain:?}");
        assert!(plain.ends_with("2 в работе "), "{plain:?}");
    }

    /// Перенос по словам: длинный ответ агента должен читаться целиком, а не
    /// обрываться многоточием на самом интересном месте.
    #[test]
    fn wrap_breaks_on_words_and_keeps_everything() {
        let lines = wrap("раз два три четыре пять шесть", 11);
        assert!(lines.iter().all(|l| width(l) <= 11), "{lines:?}");
        assert_eq!(lines.join(" "), "раз два три четыре пять шесть");
    }

    #[test]
    fn wrap_splits_a_word_longer_than_the_line() {
        let long = "/очень/длинный/путь/который/никуда/не/влезает";
        let lines = wrap(long, 10);
        assert!(lines.iter().all(|l| width(l) <= 10), "{lines:?}");
        assert_eq!(lines.concat(), long, "ни один символ не потерялся");
    }

    #[test]
    fn wrap_keeps_paragraphs_apart() {
        assert_eq!(wrap("первый\nвторой", 20), vec!["первый", "второй"]);
    }

    /// Приближение цвета для терминалов без 24 бит: важно не «попал в лесенку»,
    /// а «попал близко». Проверяем расстояние — на нём и держится вся палитра
    /// там, где truecolor нет.
    #[test]
    fn xterm256_stays_close_to_the_asked_color() {
        // Обратное преобразование индекса в цвет — та же таблица, что у xterm.
        fn back(i: u8) -> (i32, i32, i32) {
            const STEPS: [i32; 6] = [0, 95, 135, 175, 215, 255];
            if i >= 232 {
                let v = 8 + (i as i32 - 232) * 10;
                (v, v, v)
            } else {
                let n = i as i32 - 16;
                (
                    STEPS[(n / 36) as usize],
                    STEPS[((n / 6) % 6) as usize],
                    STEPS[(n % 6) as usize],
                )
            }
        }
        for (r, g, b) in [
            ink::TEXT,
            ink::MUTED,
            ink::DIM,
            ink::ACCENT,
            ink::OK,
            ink::WARN,
            ink::ERR,
            ink::BORDER,
        ] {
            let (br, bg, bb) = back(xterm256(r, g, b));
            let err = (br - r as i32)
                .abs()
                .max((bg - g as i32).abs())
                .max((bb - b as i32).abs());
            assert!(err <= 20, "цвет {r:02x}{g:02x}{b:02x} промахнулся на {err}");
        }
        // Серые роли не должны схлопнуться в один индекс, иначе приглушённое
        // перестанет отличаться от совсем фонового.
        assert_ne!(
            xterm256(ink::MUTED.0, ink::MUTED.1, ink::MUTED.2),
            xterm256(ink::DIM.0, ink::DIM.1, ink::DIM.2)
        );
    }

    fn strip(s: &str) -> String {
        let mut out = String::new();
        let mut it = s.chars().peekable();
        while let Some(c) = it.next() {
            if c == '\x1b' {
                for e in it.by_ref() {
                    if e.is_ascii_alphabetic() {
                        break;
                    }
                }
                continue;
            }
            out.push(c);
        }
        out
    }

    /// Ширина, о которой не удалось спросить, не должна превращаться в верстку
    /// в двадцать колонок: подсказки от неё остаются от слова «под…».
    #[test]
    fn unknown_width_falls_back_to_eighty() {
        assert!(term_width() >= 20);
        let narrow = Caps {
            color: false,
            truecolor: false,
            unicode: true,
            width: term_width(),
        };
        assert!(width(&rule(&narrow, "Заголовок")) <= narrow.width as usize);
    }

    fn caps(color: bool, unicode: bool) -> Caps {
        Caps {
            color,
            truecolor: color,
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
