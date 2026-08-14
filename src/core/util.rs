//! Мелкие утилиты — те же договоры, что у настольного Jarvis.
//!
//! CLI сознательно делит с ним каталог данных и файлы состояния: циклы и
//! связки, заведённые в панели, видны в терминале и наоборот. Два интерфейса —
//! одно состояние.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Каталог данных Jarvis: $JARVIS_DIR или ~/.jarvis — как у настольного.
pub fn jarvis_dir() -> PathBuf {
    match std::env::var("JARVIS_DIR") {
        Ok(d) if !d.is_empty() => expand_tilde(&d),
        _ => home_dir().join(".jarvis"),
    }
}

/// Раскрыть `~` в начале пути.
///
/// Тильду раскрывает шелл, но не всякий и не всегда: `export JARVIS_DIR=~/.jarvis`
/// в fish оставляет её строкой, и дальше всё честно работает — с каталогом,
/// который буквально называется «~». Раскрываем сами: путь с тильдой человек
/// имел в виду один, и это не «./~».
pub fn expand_tilde(p: &str) -> PathBuf {
    match p.strip_prefix('~') {
        Some("") => home_dir(),
        Some(rest) => match rest.strip_prefix('/') {
            Some(rel) => home_dir().join(rel),
            // «~user/…» нам не по адресу: чужой домашний каталог мы не знаем и
            // гадать не станем — пусть остаётся как есть и честно не найдётся.
            None => PathBuf::from(p),
        },
        None => PathBuf::from(p),
    }
}

/// Путь для человека: домашний каталог — тильдой.
///
/// Полный путь в сообщении об ошибке съедает всю строку и переносится посреди
/// слова; «~/jarvis-node/node.sock» читается с одного взгляда.
pub fn short_path(p: &str) -> String {
    let home = home_dir();
    let home = home.to_string_lossy();
    match p.strip_prefix(home.as_ref()) {
        Some(rest) if home != "/" => format!("~{rest}"),
        _ => p.to_string(),
    }
}

pub fn home_dir() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into()))
}

/// Миллисекунды эпохи — формат времени всех файлов состояния.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Схлопнуть пробелы в один, обрезать края.
pub fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Обрезка по символам: байтовый срез русского текста рвёт UTF-8 на границе.
pub fn ellipsize(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max_chars.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// Одинарные кавычки для POSIX-шелла (экранируя `'`).
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// «14:05» из миллисекунд эпохи — местное время.
pub fn clock(ms: i64) -> String {
    if ms <= 0 {
        return String::new();
    }
    use chrono::TimeZone;
    chrono::Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|d| d.format("%H:%M").to_string())
        .unwrap_or_default()
}

/// Убрать разметку из строки, которую покажут одной строкой.
///
/// В терминале `**Готово**` — не жирный шрифт, а четыре лишних знака, и в
/// списке сессий они лезут в глаза первыми. Курсив снимаем только парный:
/// «2 * 2» обязано остаться умножением.
pub fn plain_text(s: &str) -> String {
    let no_bold = s.replace("**", "").replace("__", "");
    let chars: Vec<char> = no_bold.chars().collect();
    let mut out = String::with_capacity(no_bold.len());
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '*' && chars.get(i + 1).is_some_and(|c| !c.is_whitespace()) {
            let mut j = i + 1;
            let mut close = None;
            while j < chars.len() {
                if chars[j] == '*' {
                    if chars.get(j - 1).is_some_and(|c| !c.is_whitespace()) {
                        close = Some(j);
                    }
                    break;
                }
                j += 1;
            }
            if let Some(c) = close {
                out.extend(chars[i + 1..c].iter());
                i = c + 1;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Русское число словами: 1 сессия, 2 сессии, 5 сессий.
///
/// «3 сессий» и «2 ждёт» — мелочь, но именно из таких мелочей складывается
/// ощущение, что программу писали наспех.
pub fn plural(n: u64, one: &str, few: &str, many: &str) -> String {
    let (d10, d100) = (n % 10, n % 100);
    let word = if (11..=14).contains(&d100) {
        many
    } else if d10 == 1 {
        one
    } else if (2..=4).contains(&d10) {
        few
    } else {
        many
    };
    format!("{n} {word}")
}

/// Момент словами: «20:59» сегодня, «завтра 09:00», «18 авг 22:59» дальше.
///
/// Одни часы годятся только для «сегодня». Недельное окно сбрасывается через
/// шесть дней, и голое «до 22:59» читалось бы как «вечером» — это враньё в
/// самом чувствительном месте, где человек решает, ждать ему или работать.
pub fn when(ms: i64) -> String {
    if ms <= 0 {
        return String::new();
    }
    use chrono::TimeZone;
    let Some(d) = chrono::Local.timestamp_millis_opt(ms).single() else {
        return String::new();
    };
    let today = chrono::Local
        .timestamp_millis_opt(now_ms())
        .single()
        .map(|n| n.date_naive());
    let hm = d.format("%H:%M").to_string();
    match today {
        Some(t) if d.date_naive() == t => hm,
        Some(t) if d.date_naive() == t.succ_opt().unwrap_or(t) => format!("завтра {hm}"),
        _ => {
            const MONTHS: [&str; 12] = [
                "янв", "фев", "мар", "апр", "мая", "июн", "июл", "авг", "сен", "окт", "ноя", "дек",
            ];
            use chrono::Datelike;
            let m = MONTHS[(d.month0() as usize).min(11)];
            format!("{} {m} {hm}", d.day())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilde_becomes_the_home_directory() {
        let home = home_dir();
        assert_eq!(expand_tilde("~/.jarvis"), home.join(".jarvis"));
        assert_eq!(expand_tilde("~"), home);
        assert_eq!(expand_tilde("/srv/jarvis"), PathBuf::from("/srv/jarvis"));
        // Чужой дом не выдумываем.
        assert_eq!(expand_tilde("~bob/.jarvis"), PathBuf::from("~bob/.jarvis"));
    }

    /// Разметка не должна лезть в список: там читают смысл, а не звёздочки.
    #[test]
    fn plain_text_drops_markers_but_keeps_multiplication() {
        assert_eq!(plain_text("**Готово**: 5,8 ГБ"), "Готово: 5,8 ГБ");
        assert_eq!(plain_text("это *важно* сегодня"), "это важно сегодня");
        assert_eq!(plain_text("площадь = 2 * 2"), "площадь = 2 * 2");
        assert_eq!(plain_text("без разметки"), "без разметки");
    }

    #[test]
    fn plural_follows_russian_rules_including_the_teens() {
        assert_eq!(plural(1, "сессия", "сессии", "сессий"), "1 сессия");
        assert_eq!(plural(3, "сессия", "сессии", "сессий"), "3 сессии");
        assert_eq!(plural(5, "сессия", "сессии", "сессий"), "5 сессий");
        // Одиннадцать — не «одна»: подвох, на котором ломается наивная проверка.
        assert_eq!(plural(11, "сессия", "сессии", "сессий"), "11 сессий");
        assert_eq!(plural(21, "сессия", "сессии", "сессий"), "21 сессия");
        assert_eq!(plural(112, "сессия", "сессии", "сессий"), "112 сессий");
        assert_eq!(plural(0, "сессия", "сессии", "сессий"), "0 сессий");
    }

    #[test]
    fn when_says_the_date_once_it_is_not_today() {
        let now = now_ms();
        assert_eq!(when(now), clock(now), "сегодня — просто часы");
        let tomorrow = when(now + 26 * 3_600_000);
        assert!(
            tomorrow.starts_with("завтра ") || tomorrow.contains(' '),
            "{tomorrow}"
        );
        let week = when(now + 6 * 24 * 3_600_000);
        assert!(
            week.chars().any(|c| c.is_alphabetic()),
            "через неделю нужен месяц, а не одни часы: {week}"
        );
        assert!(when(0).is_empty());
    }

    #[test]
    fn ellipsize_respects_characters_not_bytes() {
        assert_eq!(ellipsize("привет", 10), "привет");
        assert_eq!(ellipsize("привет, мир", 7), "привет…");
    }

    #[test]
    fn shell_quote_survives_quotes() {
        assert_eq!(shell_quote("а'б"), r"'а'\''б'");
    }
}
