//! Вопросы в терминале: строка — вопрос, Enter — согласие с умолчанием.
//!
//! Конструктор в панели делался ради одного: команды не вводят из головы. В
//! терминале соблазн обратный — попросить всё флагами и получить строку в
//! двести знаков, которую никто не напишет дважды. Поэтому здесь диалог: выбор
//! номером, умолчание в скобках, каталог заготовок вместо памяти.
//!
//! Разбор ответов вынесен в чистые функции — их и проверяют тесты; ввод-вывод
//! остаётся тонкой оболочкой вокруг них.

use crate::ui::style::{paint, rule, truncate, width, Caps, Role};
use std::io::{IsTerminal, Write};

/// Диалог возможен только у живого терминала. В пайпе спрашивать некого:
/// молчаливое ожидание ввода выглядит как зависание.
pub fn interactive() -> bool {
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

/// Пункт списка: что выбирают и почему.
pub struct Choice {
    pub label: String,
    pub hint: String,
    pub group: String,
}

impl Choice {
    pub fn new(label: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            hint: hint.into(),
            group: String::new(),
        }
    }

    pub fn in_group(mut self, group: impl Into<String>) -> Self {
        self.group = group.into();
        self
    }
}

/* ---------- разбор ответов ---------- */

/// «да»/«нет» на любом привычном языке. Пусто — умолчание.
pub fn parse_yes(input: &str, default_yes: bool) -> Option<bool> {
    let s = input.trim().to_lowercase();
    if s.is_empty() {
        return Some(default_yes);
    }
    match s.as_str() {
        "д" | "да" | "y" | "yes" | "ага" | "+" | "1" => Some(true),
        "н" | "нет" | "n" | "no" | "-" | "0" => Some(false),
        _ => None,
    }
}

/// Число с людскими суффиксами: `200k`, `1.5к`, `300 000`.
pub fn parse_number(input: &str, default: u64) -> Option<u64> {
    let s = input.trim().to_lowercase().replace([' ', '_', ' '], "");
    if s.is_empty() {
        return Some(default);
    }
    let (num, mult) = match s.strip_suffix(['k', 'к']) {
        Some(head) => (head, 1000.0),
        None => match s.strip_suffix(['m', 'м']) {
            Some(head) => (head, 1_000_000.0),
            None => (s.as_str(), 1.0),
        },
    };
    num.replace(',', ".")
        .parse::<f64>()
        .ok()
        .filter(|v| *v >= 0.0)
        .map(|v| (v * mult).round() as u64)
}

/// Номер пункта: 1..=len. Пусто — умолчание (уже как индекс).
pub fn parse_choice(input: &str, len: usize, default: usize) -> Option<usize> {
    let s = input.trim();
    if s.is_empty() {
        return (default < len).then_some(default);
    }
    s.parse::<usize>()
        .ok()
        .filter(|n| (1..=len).contains(n))
        .map(|n| n - 1)
}

/// Несколько номеров: «1, 3 5». Пусто — ничего не выбрано, и это законный ответ.
pub fn parse_many(input: &str, len: usize) -> Option<Vec<usize>> {
    let mut out = Vec::new();
    for part in input
        .split([',', ' ', ';'])
        .filter(|p| !p.trim().is_empty())
    {
        let n = part
            .trim()
            .parse::<usize>()
            .ok()
            .filter(|n| (1..=len).contains(n))?;
        if !out.contains(&(n - 1)) {
            out.push(n - 1);
        }
    }
    Some(out)
}

/// Как выглядит вопрос: сам вопрос и умолчание в скобках.
pub fn question(caps: &Caps, q: &str, default: &str) -> String {
    if default.is_empty() {
        format!("  {q}: ")
    } else {
        format!(
            "  {q} {}: ",
            paint(caps, Role::Dim, &format!("[{}]", truncate(default, 48)))
        )
    }
}

/* ---------- ввод-вывод ---------- */

fn read_line() -> Result<String, String> {
    let mut s = String::new();
    let n = std::io::stdin()
        .read_line(&mut s)
        .map_err(|e| format!("не читается ввод: {e}"))?;
    if n == 0 {
        // Ctrl-D: человек передумал. Это не ошибка, но и продолжать нечего.
        return Err("ввод закончился — отменяю".into());
    }
    Ok(s.trim().to_string())
}

fn put(text: &str) {
    print!("{text}");
    let _ = std::io::stdout().flush();
}

/// Свободный ответ. Пустой при пустом умолчании — переспрашиваем.
pub fn ask(caps: &Caps, q: &str, default: &str) -> Result<String, String> {
    loop {
        put(&question(caps, q, default));
        let a = read_line()?;
        let v = if a.is_empty() { default.to_string() } else { a };
        if !v.trim().is_empty() {
            return Ok(v.trim().to_string());
        }
        put(&paint(caps, Role::Dim, "  пусто — так нельзя, ответь\n"));
    }
}

pub fn yes(caps: &Caps, q: &str, default_yes: bool) -> Result<bool, String> {
    let d = if default_yes { "да" } else { "нет" };
    loop {
        put(&question(caps, q, d));
        match parse_yes(&read_line()?, default_yes) {
            Some(v) => return Ok(v),
            None => put(&paint(caps, Role::Dim, "  ответь «да» или «нет»\n")),
        }
    }
}

pub fn number(caps: &Caps, q: &str, default: u64) -> Result<u64, String> {
    let d = crate::ui::render::fmt_tokens(default);
    loop {
        put(&question(caps, q, &d));
        match parse_number(&read_line()?, default) {
            Some(v) => return Ok(v),
            None => put(&paint(caps, Role::Dim, "  нужно число, можно с «k»\n")),
        }
    }
}

/// Показать список и вернуть выбранный индекс.
pub fn choose(caps: &Caps, title: &str, items: &[Choice], default: usize) -> Result<usize, String> {
    list(caps, title, items);
    loop {
        put(&question(caps, "номер", &(default + 1).to_string()));
        match parse_choice(&read_line()?, items.len(), default) {
            Some(i) => return Ok(i),
            None => put(&paint(
                caps,
                Role::Dim,
                &format!("  номер от 1 до {}\n", items.len()),
            )),
        }
    }
}

/// То же, но можно выбрать несколько — или ни одного.
pub fn choose_many(caps: &Caps, title: &str, items: &[Choice]) -> Result<Vec<usize>, String> {
    list(caps, title, items);
    loop {
        put(&question(caps, "номера через запятую", "пусто — ни одного"));
        let line = read_line()?;
        match parse_many(&line, items.len()) {
            Some(v) => return Ok(v),
            None => put(&paint(
                caps,
                Role::Dim,
                &format!("  номера от 1 до {}, через запятую\n", items.len()),
            )),
        }
    }
}

/// Показать список без вопроса — каталог заготовок сам по себе полезен.
pub fn list(caps: &Caps, title: &str, items: &[Choice]) {
    // Пустая строка перед списком: вопросы идут подряд, и без воздуха каталог
    // слипается с предыдущим ответом в сплошную стену.
    println!();
    println!("{}", rule(caps, title));
    let num_col = items.len().to_string().len();
    let label_col = items
        .iter()
        .map(|c| width(&c.label))
        .max()
        .unwrap_or(10)
        .clamp(8, 34);
    let mut group = String::new();
    for (i, c) in items.iter().enumerate() {
        if c.group != group {
            group = c.group.clone();
            if !group.is_empty() {
                println!("  {}", paint(caps, Role::Dim, &group));
            }
        }
        let head = format!(
            "  {:>w$}  {}",
            paint(caps, Role::Accent, &(i + 1).to_string()),
            crate::ui::style::pad(&truncate(&c.label, label_col), label_col),
            w = num_col,
        );
        if c.hint.is_empty() {
            println!("{head}");
        } else {
            let room = (caps.width as usize).saturating_sub(width(&head) + 2);
            println!(
                "{head}  {}",
                paint(caps, Role::Dim, &truncate(&c.hint, room.max(10)))
            );
        }
    }
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
    fn yes_and_no_in_both_languages() {
        for s in ["да", "Д", "y", "YES", "+", "1", "ага"] {
            assert_eq!(parse_yes(s, false), Some(true), "{s}");
        }
        for s in ["нет", "н", "n", "NO", "-", "0"] {
            assert_eq!(parse_yes(s, true), Some(false), "{s}");
        }
        assert_eq!(parse_yes("", true), Some(true), "пусто — умолчание");
        assert_eq!(parse_yes("может быть", true), None, "непонятое не толкуем");
    }

    #[test]
    fn numbers_accept_human_suffixes() {
        assert_eq!(parse_number("200k", 0), Some(200_000));
        assert_eq!(parse_number("1.5к", 0), Some(1500));
        assert_eq!(parse_number("300 000", 0), Some(300_000));
        assert_eq!(parse_number("", 20), Some(20));
        assert_eq!(parse_number("много", 20), None);
        assert_eq!(
            parse_number("-5", 20),
            None,
            "отрицательная стена — бессмыслица"
        );
    }

    #[test]
    fn choice_numbers_start_at_one_for_humans() {
        assert_eq!(parse_choice("1", 3, 2), Some(0));
        assert_eq!(parse_choice("3", 3, 0), Some(2));
        assert_eq!(parse_choice("", 3, 1), Some(1), "пусто — умолчание");
        assert_eq!(parse_choice("0", 3, 0), None);
        assert_eq!(parse_choice("4", 3, 0), None);
        assert_eq!(parse_choice("да", 3, 0), None);
    }

    #[test]
    fn many_takes_commas_spaces_and_refuses_junk() {
        assert_eq!(parse_many("1, 3 5", 5), Some(vec![0, 2, 4]));
        assert_eq!(parse_many("2,2", 5), Some(vec![1]), "повтор — один раз");
        assert_eq!(
            parse_many("", 5),
            Some(vec![]),
            "ни одного — законный ответ"
        );
        assert_eq!(
            parse_many("1,9", 5),
            None,
            "чужой номер отменяет весь ответ"
        );
    }

    /// Умолчание видно в самом вопросе — иначе Enter жмут вслепую.
    #[test]
    fn question_shows_the_default() {
        let q = question(&caps(), "Имя", "ночной обход");
        assert!(q.contains("Имя") && q.contains("[ночной обход]"), "{q}");
        assert!(q.ends_with(": "));
        assert!(!question(&caps(), "Цель", "").contains('['));
    }
}
