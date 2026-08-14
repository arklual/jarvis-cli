//! Вопросы в терминале: строка — вопрос, Enter — согласие с умолчанием.
//!
//! Конструктор в панели делался ради одного: команды не вводят из головы. В
//! терминале соблазн обратный — попросить всё флагами и получить строку в
//! двести знаков, которую никто не напишет дважды. Поэтому здесь диалог: выбор
//! номером, умолчание в скобках, каталог заготовок вместо памяти.
//!
//! Разбор ответов вынесен в чистые функции — их и проверяют тесты; ввод-вывод
//! остаётся тонкой оболочкой вокруг них.

use crate::ui::style::{pad, paint, rule, truncate, width, Caps, Role};
use crossterm::event::{KeyCode, KeyModifiers};
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
///
/// Если терминал живой — выбираем стрелками, как в pi (SelectList): список
/// перед глазами, набранное отбирает строки, Enter берёт. Ввод номера остаётся
/// запасным путём — он нужен там, где стрелки не доедут (пайп, чужой терминал).
pub fn choose(caps: &Caps, title: &str, items: &[Choice], default: usize) -> Result<usize, String> {
    if interactive() {
        if let Some(picked) = pick(caps, title, items, default)? {
            return Ok(picked);
        }
        return Err("выбор отменён".into());
    }
    choose_by_number(caps, title, items, default)
}

/// Что делает клавиша в списке.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Pick {
    Up,
    Down,
    Top,
    Bottom,
    Take,
    Cancel,
    Back,
    Clear,
    Type(char),
}

/// Раскладка списка. Отдельной функцией — чтобы проверять её тестом, а не
/// глазами через pty.
///
/// Ctrl+N/Ctrl+P ходят по списку, Ctrl+J — это тот же Enter (в сыром режиме
/// перевод строки приходит именно так), Ctrl+D — конец ввода, то есть отмена.
/// Остальные сочетания с Ctrl молчат: раньше Ctrl+D добавлял в отбор букву «d»
/// и список внезапно оказывался пустым.
fn pick_act(code: KeyCode, mods: KeyModifiers) -> Option<Pick> {
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    match code {
        KeyCode::Up => Some(Pick::Up),
        KeyCode::Down => Some(Pick::Down),
        KeyCode::Home | KeyCode::PageUp => Some(Pick::Top),
        KeyCode::End | KeyCode::PageDown => Some(Pick::Bottom),
        KeyCode::Enter | KeyCode::Tab => Some(Pick::Take),
        KeyCode::Esc => Some(Pick::Cancel),
        KeyCode::Backspace => Some(Pick::Back),
        KeyCode::Char(c) if ctrl => match c {
            'n' => Some(Pick::Down),
            'p' => Some(Pick::Up),
            'j' | 'm' => Some(Pick::Take),
            'c' | 'd' | 'g' => Some(Pick::Cancel),
            'u' | 'w' => Some(Pick::Clear),
            'a' => Some(Pick::Top),
            'e' => Some(Pick::Bottom),
            _ => None,
        },
        KeyCode::Char(c) => Some(Pick::Type(c)),
        _ => None,
    }
}

/// С какой строки показывать окно списка, чтобы выбранное было видно.
///
/// Держим выбранное в середине, как SelectList в pi: прижатый к краю выбор не
/// даёт увидеть, что идёт следом. У краёв списка окно упирается — сверху в
/// начало, снизу в конец, иначе под списком висел бы пустой хвост.
fn window_start(pos: usize, len: usize, room: usize) -> usize {
    if len <= room {
        return 0;
    }
    pos.saturating_sub(room / 2).min(len - room)
}

/// Интерактивный выбор: стрелки, отбор набором, Enter.
///
/// `Ok(None)` — человек передумал (Esc).
fn pick(
    caps: &Caps,
    title: &str,
    items: &[Choice],
    default: usize,
) -> Result<Option<usize>, String> {
    use crossterm::event::{poll, read, Event};
    if items.is_empty() {
        return Ok(None);
    }
    let raw = crossterm::terminal::enable_raw_mode().is_ok();
    print!("\x1b[?25l");
    let mut filter = String::new();
    let mut at = default.min(items.len() - 1);
    let mut drawn = 0usize;
    let result = loop {
        // Отбор по похожести — тот же, что в окне: человек набирает три буквы,
        // а не листает двадцать строк.
        let shown: Vec<usize> = (0..items.len())
            .filter(|i| {
                filter.is_empty()
                    || crate::ui::slash::fuzzy_score(&filter, &items[*i].label).is_some()
                    || crate::ui::slash::fuzzy_score(&filter, &items[*i].hint).is_some()
            })
            .collect();
        if !shown.contains(&at) {
            at = shown.first().copied().unwrap_or(0);
        }
        let mut out = String::new();
        if drawn > 1 {
            // Возвращаемся на начало прошлой отрисовки. Курсор стоит НА
            // последней строке (после неё перевода нет), поэтому вверх идём
            // на строку меньше — иначе список печатался бы заново каждый раз.
            out.push_str(&format!("\r\x1b[{}A", drawn - 1));
        } else {
            out.push('\r');
        }
        out.push_str(&format!("{}\x1b[K\r\n", rule(caps, title)));
        let mut lines = 1usize;
        let room = 12.min(shown.len().max(1));
        // Окно едет за выбранным, держа его В СЕРЕДИНЕ, как SelectList в pi:
        // прижатый к краю выбор не даёт увидеть, что идёт следом.
        let pos = shown.iter().position(|i| *i == at).unwrap_or(0);
        let from = window_start(pos, shown.len(), room);
        // Названия — колонкой: неровный левый край второго столбца читается
        // как список случайных строк.
        let labelw = shown
            .iter()
            .skip(from)
            .take(room)
            .map(|i| width(&items[*i].label))
            .max()
            .unwrap_or(12)
            .clamp(12, 32);
        for i in shown.iter().copied().skip(from).take(room) {
            let c = &items[i];
            let mark = if i == at { "▸" } else { " " };
            let label = if i == at {
                paint(caps, Role::Accent, &c.label)
            } else {
                paint(caps, Role::Text, &c.label)
            };
            let room_for_hint = (caps.width as usize).saturating_sub(labelw + 8);
            let hint = if c.hint.is_empty() {
                String::new()
            } else {
                format!(
                    "  {}",
                    paint(caps, Role::Dim, &truncate(&c.hint, room_for_hint.max(12)))
                )
            };
            out.push_str(&format!("  {mark} {}{hint}\x1b[K\r\n", pad(&label, labelw)));
            lines += 1;
        }
        // Сколько всего и где мы — только когда список не поместился целиком.
        if shown.len() > room {
            out.push_str(&format!(
                "    {}\x1b[K\r\n",
                paint(caps, Role::Dim, &format!("{}/{}", pos + 1, shown.len()))
            ));
            lines += 1;
        }
        if shown.is_empty() {
            out.push_str(&format!(
                "  {}\x1b[K\r\n",
                paint(caps, Role::Muted, "ничего не подходит")
            ));
            lines += 1;
        }
        out.push_str(&format!(
            "  {}{}\x1b[K",
            paint(
                caps,
                Role::Dim,
                "↑↓ выбор · ↵ взять · esc отмена · набирай — отбор  "
            ),
            paint(caps, Role::Accent, &filter)
        ));
        // Список ужимается по мере отбора — хвост прошлого кадра надо стереть,
        // иначе под однострочным списком остаётся висеть десяток чужих строк.
        out.push_str("\x1b[J");
        lines += 1;
        print!("{out}");
        let _ = std::io::stdout().flush();
        drawn = lines;

        if !poll(std::time::Duration::from_millis(200)).unwrap_or(false) {
            continue;
        }
        let ev = match read() {
            Ok(ev) => ev,
            // Ввод кончился (труба закрылась) — читать дальше нечего, и крутить
            // пустой цикл на закрытом stdin значит съесть ядро целиком.
            Err(_) => break None,
        };
        let Event::Key(k) = ev else { continue };
        match pick_act(k.code, k.modifiers) {
            Some(Pick::Cancel) => break None,
            Some(Pick::Take) => break Some(at),
            Some(Pick::Up) => {
                let pos = shown.iter().position(|i| *i == at).unwrap_or(0);
                // По кругу: у списка из трёх пунктов «вверх» с первого должно
                // приводить к последнему, а не упираться.
                at = *shown
                    .get(if pos == 0 {
                        shown.len().saturating_sub(1)
                    } else {
                        pos - 1
                    })
                    .unwrap_or(&at);
            }
            Some(Pick::Down) => {
                let pos = shown.iter().position(|i| *i == at).unwrap_or(0);
                at = *shown.get((pos + 1) % shown.len().max(1)).unwrap_or(&at);
            }
            Some(Pick::Top) => at = shown.first().copied().unwrap_or(at),
            Some(Pick::Bottom) => at = shown.last().copied().unwrap_or(at),
            Some(Pick::Back) => {
                filter.pop();
            }
            Some(Pick::Clear) => filter.clear(),
            Some(Pick::Type(c)) => filter.push(c),
            None => {}
        }
    };
    print!("\x1b[?25h\r\n");
    let _ = std::io::stdout().flush();
    if raw {
        let _ = crossterm::terminal::disable_raw_mode();
    }
    Ok(result)
}

/// Запасной путь: номер строкой. Нужен, когда стрелки не доедут.
fn choose_by_number(
    caps: &Caps,
    title: &str,
    items: &[Choice],
    default: usize,
) -> Result<usize, String> {
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
            theme: crate::ui::style::Theme::Dark,
            truecolor: false,
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

    #[test]
    fn control_keys_do_not_leak_letters_into_the_filter() {
        // Конец ввода приходит как Ctrl+D; раньше он добавлял «d» в отбор и
        // список схлопывался в «ничего не подходит».
        assert_eq!(
            pick_act(KeyCode::Char('d'), KeyModifiers::CONTROL),
            Some(Pick::Cancel)
        );
        assert_eq!(
            pick_act(KeyCode::Char('j'), KeyModifiers::CONTROL),
            Some(Pick::Take)
        );
        assert_eq!(pick_act(KeyCode::Char('z'), KeyModifiers::CONTROL), None);
        // Обычная буква — по-прежнему отбор.
        assert_eq!(
            pick_act(KeyCode::Char('d'), KeyModifiers::NONE),
            Some(Pick::Type('d'))
        );
    }

    #[test]
    fn the_list_walks_by_arrows_and_by_emacs_keys() {
        assert_eq!(
            pick_act(KeyCode::Down, KeyModifiers::NONE),
            Some(Pick::Down)
        );
        assert_eq!(
            pick_act(KeyCode::Char('n'), KeyModifiers::CONTROL),
            Some(Pick::Down)
        );
        assert_eq!(
            pick_act(KeyCode::Char('p'), KeyModifiers::CONTROL),
            Some(Pick::Up)
        );
        assert_eq!(
            pick_act(KeyCode::PageUp, KeyModifiers::NONE),
            Some(Pick::Top)
        );
        assert_eq!(
            pick_act(KeyCode::Esc, KeyModifiers::NONE),
            Some(Pick::Cancel)
        );
    }
    /// Окно списка обязано держать выбранное на виду и не свисать за края.
    #[test]
    fn the_window_keeps_the_choice_in_sight() {
        // Список короче окна — показываем с начала.
        assert_eq!(window_start(3, 5, 12), 0);
        // В середине длинного списка выбранное стоит посередине окна.
        assert_eq!(window_start(20, 40, 10), 15);
        // У краёв окно упирается, а не свисает.
        assert_eq!(window_start(0, 40, 10), 0);
        assert_eq!(window_start(39, 40, 10), 30);
    }
}
