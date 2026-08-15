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
    /// Тёмный терминал или светлый — от этого зависит вся палитра.
    pub theme: Theme,
    /// Терминал умеет 24 бита. Иначе цвет приближается кубом из 256.
    pub truecolor: bool,
    pub unicode: bool,
    pub width: u16,
}

impl Default for Caps {
    fn default() -> Self {
        Self {
            color: false,
            theme: Theme::Dark,
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
            theme: Theme::detect(),
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
/// Какая тема сейчас. Светлый терминал — не редкость, а на нём тёмная палитра
/// выглядит выцветшей: серое по белому почти не читается.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Theme {
    #[default]
    Dark,
    Light,
}

impl Theme {
    /// Определить по терминалу и настройке.
    ///
    /// `JARVIS_THEME=light|dark` — прямое слово человека, оно сильнее всего.
    /// Иначе смотрим `COLORFGBG` (его ставят xterm, konsole, iTerm): в нём
    /// фон — последнее число, и светлым считается всё, что ярче середины.
    pub fn detect() -> Theme {
        match std::env::var("JARVIS_THEME").unwrap_or_default().trim() {
            "light" | "светлая" => return Theme::Light,
            "dark" | "тёмная" | "темная" => return Theme::Dark,
            _ => {}
        }
        let Ok(fgbg) = std::env::var("COLORFGBG") else {
            // Не сказали — считаем тёмным: так выглядит большинство терминалов,
            // и ошибка в эту сторону дешевле (тёмное на светлом читается
            // хуже, чем светлое на тёмном).
            return Theme::Dark;
        };
        match fgbg
            .rsplit(';')
            .next()
            .and_then(|b| b.trim().parse::<u8>().ok())
        {
            // 0–6 и 8 — тёмные цвета палитры, 7 и 15 — светлые.
            Some(bg) if bg == 7 || bg >= 9 => Theme::Light,
            _ => Theme::Dark,
        }
    }
}

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

    /// Светлая палитра — значения из темы pi (light.json): на белом фоне
    /// нужны не «те же цвета потемнее», а свои, иначе жёлтый становится
    /// невидимым, а серый — грязным.
    pub mod light {
        pub const TEXT: (u8, u8, u8) = (0x1f, 0x23, 0x28);
        pub const MUTED: (u8, u8, u8) = (0x6c, 0x6c, 0x6c);
        pub const DIM: (u8, u8, u8) = (0x76, 0x76, 0x76);
        pub const ACCENT: (u8, u8, u8) = (0x5a, 0x80, 0x80);
        pub const OK: (u8, u8, u8) = (0x58, 0x84, 0x58);
        pub const WARN: (u8, u8, u8) = (0x9a, 0x73, 0x26);
        pub const ERR: (u8, u8, u8) = (0xaa, 0x55, 0x55);
        pub const BORDER: (u8, u8, u8) = (0xb0, 0xb0, 0xb0);

        pub const BG_HEAD: (u8, u8, u8) = (0xe4, 0xe4, 0xea);
        pub const BG_SEL: (u8, u8, u8) = (0xd0, 0xd0, 0xe0);
        pub const BG_USER: (u8, u8, u8) = (0xe8, 0xe8, 0xe8);
        pub const BG_TOOL: (u8, u8, u8) = (0xe8, 0xe8, 0xf0);
        pub const BG_ERR: (u8, u8, u8) = (0xf0, 0xe8, 0xe8);
    }
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

fn rgb(theme: Theme, role: Role) -> Option<(u8, u8, u8)> {
    if theme == Theme::Light {
        return Some(match role {
            Role::Plain => return None,
            Role::Text => ink::light::TEXT,
            Role::Muted => ink::light::MUTED,
            Role::Dim => ink::light::DIM,
            Role::Accent => ink::light::ACCENT,
            Role::Ok => ink::light::OK,
            Role::Warn => ink::light::WARN,
            Role::Bad => ink::light::ERR,
            Role::Border => ink::light::BORDER,
        });
    }
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

fn bg_rgb(theme: Theme, bg: Bg) -> (u8, u8, u8) {
    if theme == Theme::Light {
        return match bg {
            Bg::Head => ink::light::BG_HEAD,
            Bg::Sel => ink::light::BG_SEL,
            Bg::User => ink::light::BG_USER,
            Bg::Tool => ink::light::BG_TOOL,
            Bg::Bad => ink::light::BG_ERR,
        };
    }
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
        r => match rgb(caps.theme, r) {
            None => text.to_string(),
            Some(c) => format!("\x1b[{}m{text}\x1b[0m", sgr(caps, c, false)),
        },
    }
}

/// Краска вместе с начертанием: жирное, курсив, зачёркнутое.
///
/// Одной последовательностью, а не вложенными: вложенный сброс погасил бы
/// цвет снаружи, и остаток строки поехал бы не тем тоном.
pub fn accent(caps: &Caps, role: Role, attrs: &[u8], text: &str) -> String {
    if !caps.color || text.is_empty() {
        return text.to_string();
    }
    let mut codes: Vec<String> = attrs.iter().map(|a| a.to_string()).collect();
    if let Role::Plain = role {
    } else if let Some(c) = rgb(caps.theme, role) {
        codes.push(sgr(caps, c, false));
    }
    if codes.is_empty() {
        return text.to_string();
    }
    format!("\x1b[{}m{text}\x1b[0m", codes.join(";"))
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
    if !caps.color {
        // Без краски подложки нет, и добивать строку пробелами не за чем:
        // в `jarvis chat > файл` они станут хвостами из ниоткуда.
        return format!(" {}", truncate(text, inner));
    }
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
    let code = sgr(caps, bg_rgb(caps.theme, bg), true);
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
        // Краска переживает перенос: если строку разорвало посреди жирного
        // куска, продолжение обязано остаться жирным, а конец строки — закрыть
        // краску, чтобы она не потекла на подложку соседа.
        let mut open = String::new();
        let mut line = String::new();
        let close = |line: &mut String, out: &mut Vec<String>, open: &str| {
            let done = std::mem::take(line);
            if open.is_empty() {
                out.push(done);
            } else {
                out.push(format!("{done}\x1b[0m"));
            }
        };
        for word in para.split_whitespace() {
            let w = width(word);
            if !line.is_empty() && width(&line) + 1 + w <= max {
                line.push(' ');
                line.push_str(word);
                open = sgr_open(&open, word);
                continue;
            }
            if !line.is_empty() {
                close(&mut line, &mut out, &open);
            }
            line.push_str(&open);
            // Слово длиннее строки (путь, ссылка) рвём по месту: иначе оно одно
            // растянет вёрстку и всё поедет за край экрана.
            let mut rest = word.to_string();
            while width(&rest) > max {
                let head = take_width(&rest, max);
                rest = rest[head.len()..].to_string();
                open = sgr_open(&open, &head);
                line.push_str(&head);
                close(&mut line, &mut out, &open);
                line.push_str(&open);
            }
            line.push_str(&rest);
            open = sgr_open(&open, &rest);
        }
        close(&mut line, &mut out, &open);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// Осталась ли краска открытой в конце строки. Нужна проверкам кадра: цвет,
/// не закрытый на своей строке, течёт на подложку соседней.
#[cfg(test)]
pub fn open_colour(line: &str) -> String {
    sgr_open("", line)
}

/// Какая краска остаётся включённой после куска текста.
///
/// Считаем грубо, но верно для того, что печатаем сами: сброс `ESC[0m` гасит
/// всё, любая другая последовательность — накапливается. Этого хватает, чтобы
/// перенесённая строка начиналась в том же цвете, в каком оборвалась.
fn sgr_open(before: &str, chunk: &str) -> String {
    let mut open = before.to_string();
    for (w, text) in clusters(chunk) {
        if w > 0 || !text.starts_with('\x1b') {
            continue;
        }
        if text == "\x1b[0m" || text == "\x1b[m" {
            open.clear();
        } else if text.ends_with('m') {
            open.push_str(text);
        }
    }
    open
}

/// Взять с начала строки ровно `max` ячеек, не разрывая символ.
fn take_width(s: &str, max: usize) -> String {
    let mut out = String::new();
    let mut w = 0;
    for (cw, text) in clusters(s) {
        if cw == 0 {
            out.push_str(text);
            continue;
        }
        if w + cw > max {
            break;
        }
        out.push_str(text);
        w += cw;
    }
    out
}

/// Видимая ширина строки: без управляющих последовательностей и по кластерам.
///
/// Считать посимвольно нельзя: «👨‍👩‍👧» — это пять кодовых точек и одна ячейка на
/// экране, а «й» из буквы и знака — две точки и одна ячейка. Ошибка здесь не
/// косметическая: по ширине режутся строки, выравниваются колонки и рисуются
/// подложки, и один лишний столбец ломает весь кадр.
///
/// Точность как у pi (packages/tui/src/utils.ts), но без таблиц Unicode:
/// склеиваем базовый символ с тем, что не занимает места (диакритика,
/// селекторы начертания, модификаторы тона, соединители), и меряем базу.
pub fn width(s: &str) -> usize {
    clusters(s).map(|(w, _)| w).sum()
}

/// Разбор строки на кластеры: (ширина, текст кластера).
///
/// Управляющие последовательности (ANSI-цвет, OSC-ссылки) выходят кластерами
/// нулевой ширины: в подсчёт они не идут, но обрезка обязана нести их с собой,
/// иначе краска рвётся посреди последовательности.
fn clusters(s: &str) -> impl Iterator<Item = (usize, &str)> {
    let bytes = s.as_char_indices_vec();
    ClusterIter {
        s,
        at: 0,
        chars: bytes,
    }
}

/// Вспомогательное: позиции символов, чтобы ходить по строке без паник.
trait CharIndices {
    fn as_char_indices_vec(&self) -> Vec<(usize, char)>;
}

impl CharIndices for str {
    fn as_char_indices_vec(&self) -> Vec<(usize, char)> {
        self.char_indices().collect()
    }
}

struct ClusterIter<'a> {
    s: &'a str,
    at: usize,
    chars: Vec<(usize, char)>,
}

impl<'a> Iterator for ClusterIter<'a> {
    type Item = (usize, &'a str);

    fn next(&mut self) -> Option<Self::Item> {
        while self.at < self.chars.len() {
            let (start, c) = self.chars[self.at];
            // Управляющая последовательность: ESC [ … буква, ESC ] … BEL/ST.
            if c == '\x1b' {
                self.at += 1;
                if let Some((_, next)) = self.chars.get(self.at).copied() {
                    if next == ']' {
                        // OSC: до BEL или ESC \.
                        while self.at < self.chars.len() {
                            let (_, ch) = self.chars[self.at];
                            self.at += 1;
                            if ch == '\x07' {
                                break;
                            }
                            if ch == '\x1b' {
                                self.at += 1;
                                break;
                            }
                        }
                        let to = self
                            .chars
                            .get(self.at)
                            .map(|(i, _)| *i)
                            .unwrap_or(self.s.len());
                        return Some((0, &self.s[start..to]));
                    }
                }
                while self.at < self.chars.len() {
                    let (_, ch) = self.chars[self.at];
                    self.at += 1;
                    if ch.is_ascii_alphabetic() {
                        break;
                    }
                }
                let to = self
                    .chars
                    .get(self.at)
                    .map(|(i, _)| *i)
                    .unwrap_or(self.s.len());
                return Some((0, &self.s[start..to]));
            }
            if zero_width(c) {
                // Одинокий нулевой символ: места не занимает.
                self.at += 1;
                continue;
            }
            let mut w = char_width(c);
            let mut end = self.at + 1;
            // Приклеиваем всё, что не занимает места: диакритику, селекторы,
            // модификаторы тона; соединитель ZWJ тянет за собой следующий
            // символ — так семья эмодзи остаётся одной ячейкой.
            while end < self.chars.len() {
                let (_, next) = self.chars[end];
                if zero_width(next) {
                    // Селектор начертания превращает символ в эмодзи: «✅️»
                    // занимает две ячейки, хотя сам знак — из узкого блока.
                    if next == '\u{FE0F}' {
                        w = 2;
                    }
                    let joiner = next == '\u{200D}';
                    end += 1;
                    if joiner && end < self.chars.len() {
                        end += 1;
                    }
                    continue;
                }
                break;
            }
            let from = start;
            let to = self.chars.get(end).map(|(i, _)| *i).unwrap_or(self.s.len());
            self.at = end;
            return Some((w, &self.s[from..to]));
        }
        None
    }
}

/// Символы, не занимающие ячейки: диакритика, селекторы, соединители.
fn zero_width(c: char) -> bool {
    let u = c as u32;
    (0x300..=0x36F).contains(&u)          // комбинирующая диакритика
        || (0x483..=0x489).contains(&u)   // кириллические знаки
        || (0x591..=0x5BD).contains(&u)   // иврит
        || (0x610..=0x61A).contains(&u)   // арабский
        || (0x64B..=0x65F).contains(&u)
        || (0x200B..=0x200F).contains(&u) // нулевые пробелы и соединители
        || (0xFE00..=0xFE0F).contains(&u) // селекторы начертания
        || (0xFE20..=0xFE2F).contains(&u)
        || (0x1F3FB..=0x1F3FF).contains(&u) // модификаторы тона кожи
        || (0xE0100..=0xE01EF).contains(&u)
        || u == 0
}

/// Ширина базового символа в ячейках: две у иероглифов и эмодзи, одна у
/// остального. Нулевые сюда не попадают — их отсекает `zero_width`.
fn char_width(c: char) -> usize {
    let u = c as u32;
    let wide = (0x1100..=0x115F).contains(&u)
        || (0x2E80..=0xA4CF).contains(&u)   // CJK
        || (0xAC00..=0xD7A3).contains(&u)   // хангыль
        || (0xF900..=0xFAFF).contains(&u)
        || (0xFF00..=0xFF60).contains(&u)   // полноширинные формы
        || (0xFFE0..=0xFFE6).contains(&u)
        || (0x1F300..=0x1F64F).contains(&u) // эмодзи
        || (0x1F680..=0x1F6FF).contains(&u)
        || (0x1F900..=0x1F9FF).contains(&u)
        || (0x1FA70..=0x1FAFF).contains(&u)
        // Одиночные эмодзи из старых блоков: они узкие «по таблице», но
        // терминалы рисуют их в две ячейки.
        || matches!(
            u,
            0x231A..=0x231B
                | 0x23E9..=0x23EC
                | 0x25FD..=0x25FE
                | 0x2614..=0x2615
                | 0x2648..=0x2653
                | 0x267F
                | 0x2693
                | 0x26A1
                | 0x26AA..=0x26AB
                | 0x26BD..=0x26BE
                | 0x26C4..=0x26C5
                | 0x26CE
                | 0x26D4
                | 0x26EA
                | 0x26F2..=0x26F3
                | 0x26F5
                | 0x26FA
                | 0x26FD
                | 0x2705
                | 0x270A..=0x270B
                | 0x2728
                | 0x274C
                | 0x274E
                | 0x2753..=0x2755
                | 0x2757
                | 0x2795..=0x2797
                | 0x27B0
                | 0x27BF
                | 0x2B1B..=0x2B1C
                | 0x2B50
                | 0x2B55
        );
    if wide {
        2
    } else {
        1
    }
}

/// Снять краску: строка нужна как текст — например, чтобы положить её на
/// подложку, где чужие сбросы цвета всё испортят.
pub fn strip(s: &str) -> String {
    clusters(s)
        .filter(|(w, _)| *w > 0)
        .map(|(_, t)| t)
        .collect()
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
    let mut painted = false;
    for (cw, text) in clusters(s) {
        if cw == 0 {
            // Краска места не занимает — несём её с собой, иначе обрезок
            // потеряет цвет, а то и оборвётся посреди последовательности.
            out.push_str(text);
            painted = true;
            continue;
        }
        if w + cw > max - 1 {
            break;
        }
        out.push_str(text);
        w += cw;
    }
    out.push('…');
    // Обрезали до сброса — краска потечёт на весь остаток строки.
    if painted && !out.ends_with("\x1b[0m") {
        out.push_str("\x1b[0m");
    }
    out
}

/// Подсветить вхождения обратным цветом, не сломав уже наложенную краску.
///
/// Отбор в ленте показывает подходящие записи — но не говорит, ЧЕМ они
/// подошли: в длинной строке искомое слово теряется. В pi найденное
/// подсвечивается прямо в строке (alt-screen-search.ts), и здесь так же.
///
/// Ходим по кластерам: краска — кластеры нулевой ширины, их переносим как
/// есть, а сравниваем только видимое. Иначе искомое «38» находилось бы внутри
/// escape-последовательности цвета.
pub fn highlight(caps: &Caps, line: &str, needle: &str) -> String {
    if !caps.color || needle.trim().is_empty() {
        return line.to_string();
    }
    let items: Vec<(usize, &str)> = clusters(line).collect();
    // Плоский текст и карта «байт в плоском тексте → номер кластера».
    let mut plain = String::new();
    let mut at: Vec<usize> = Vec::new();
    for (i, (w, text)) in items.iter().enumerate() {
        if *w == 0 {
            continue;
        }
        let low = text.to_lowercase();
        for _ in 0..low.len() {
            at.push(i);
        }
        plain.push_str(&low);
    }
    let needle = needle.trim().to_lowercase();
    if needle.is_empty() || plain.is_empty() {
        return line.to_string();
    }
    // Границы подсветки: номера кластеров, с которых она начинается и на
    // которых заканчивается.
    let mut on: Vec<usize> = Vec::new();
    let mut off: Vec<usize> = Vec::new();
    let mut from = 0usize;
    while let Some(hit) = plain[from..].find(&needle) {
        let start = from + hit;
        let end = start + needle.len();
        if let (Some(a), Some(b)) = (at.get(start), at.get(end - 1)) {
            on.push(*a);
            off.push(*b);
        }
        from = end.max(start + 1);
        if from >= plain.len() {
            break;
        }
    }
    if on.is_empty() {
        return line.to_string();
    }
    let mut out = String::new();
    for (i, (_, text)) in items.iter().enumerate() {
        if on.contains(&i) {
            out.push_str("\x1b[7m");
        }
        out.push_str(text);
        if off.contains(&i) {
            out.push_str("\x1b[27m");
        }
    }
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

/// Кадр спиннера. Те же брайлевские точки, что у pi: они мельче букв и не
/// прыгают по ширине, а в ASCII-терминале честно вырождаются в палочку.
///
/// Крутящийся значок — не украшение: он единственный отвечает на вопрос «оно
/// живое или повисло», пока идёт чужая долгая работа.
pub fn spinner(caps: &Caps, tick: u64) -> String {
    const DOTS: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    const ASCII: [&str; 4] = ["|", "/", "-", "\\"];
    let frame = if caps.unicode {
        DOTS[(tick as usize) % DOTS.len()]
    } else {
        ASCII[(tick as usize) % ASCII.len()]
    };
    paint(caps, Role::Accent, frame)
}

/// Сколько идёт: «12 с», «3 м 05 с». Без времени спиннер говорит «живое», но
/// не говорит «слишком долго» — а это второй вопрос, который задают.
pub fn elapsed(ms: i64) -> String {
    let sec = (ms / 1000).max(0);
    if sec < 60 {
        format!("{sec} с")
    } else {
        format!("{} м {:02} с", sec / 60, sec % 60)
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

    /// Спиннер обязан крутиться и не менять ширину: прыгающая на ячейку
    /// строка состояния читается как дрожь.
    #[test]
    fn the_spinner_turns_without_changing_width() {
        let c = Caps {
            color: false,
            theme: Theme::Dark,
            truecolor: false,
            unicode: true,
            width: 40,
        };
        let frames: Vec<String> = (0..10).map(|i| spinner(&c, i)).collect();
        assert!(frames.iter().all(|f| width(f) == 1), "{frames:?}");
        assert!(
            frames.windows(2).any(|w| w[0] != w[1]),
            "спиннер не крутится"
        );
        assert_eq!(spinner(&c, 0), spinner(&c, 10), "круг замкнулся");
        // В ASCII-терминале — палочка, а не пустота.
        let a = Caps {
            unicode: false,
            ..c
        };
        assert_eq!(width(&spinner(&a, 0)), 1);
    }

    #[test]
    fn elapsed_reads_like_a_clock() {
        assert_eq!(elapsed(0), "0 с");
        assert_eq!(elapsed(12_000), "12 с");
        assert_eq!(elapsed(185_000), "3 м 05 с");
        assert_eq!(elapsed(-5), "0 с", "часы не идут назад");
    }

    fn colored() -> Caps {
        Caps {
            color: true,
            theme: Theme::Dark,
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
    fn band_without_color_is_just_text() {
        let c = Caps {
            color: false,
            theme: Theme::Dark,
            truecolor: false,
            unicode: true,
            width: 40,
        };
        // Без краски подложки нет — значит и добивать строку пробелами не за
        // чем: в `jarvis chat > файл` из них выйдут хвосты из ниоткуда.
        let line = band(&c, Bg::User, "привет", 20);
        assert_eq!(line, " привет");
        assert!(!line.contains('\x1b'));
    }

    #[test]
    fn header_puts_the_summary_on_the_right_edge() {
        let c = colored();
        let h = header(&c, "Jarvis · local", "2 в работе", 40);
        assert_eq!(width(&h), 40);
        let plain = plain_csi(&h);
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

    /// Старый помощник тестов: снимает только цветовые последовательности.
    fn plain_csi(s: &str) -> String {
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
            theme: Theme::Dark,
            truecolor: false,
            unicode: true,
            width: term_width(),
        };
        assert!(width(&rule(&narrow, "Заголовок")) <= narrow.width as usize);
    }

    fn caps(color: bool, unicode: bool) -> Caps {
        Caps {
            color,
            theme: Theme::Dark,
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

    /// На светлом терминале тёмная палитра выцветает: серое по белому почти
    /// не читается. Тема определяется до первой отрисовки.
    #[test]
    fn the_theme_is_read_from_the_terminal_and_the_word_of_the_human() {
        // Прямое слово человека сильнее всего.
        std::env::set_var("JARVIS_THEME", "light");
        assert_eq!(Theme::detect(), Theme::Light);
        std::env::set_var("JARVIS_THEME", "тёмная");
        assert_eq!(Theme::detect(), Theme::Dark);
        std::env::remove_var("JARVIS_THEME");
        // Иначе — по фону терминала: 15 и 7 светлые, 0 тёмный.
        std::env::set_var("COLORFGBG", "0;15");
        assert_eq!(Theme::detect(), Theme::Light);
        std::env::set_var("COLORFGBG", "15;0");
        assert_eq!(Theme::detect(), Theme::Dark);
        std::env::remove_var("COLORFGBG");
        // Молчание — тёмная: ошибка в эту сторону дешевле.
        assert_eq!(Theme::detect(), Theme::Dark);
    }

    /// Светлая тема — не «те же цвета потемнее»: на белом жёлтый становится
    /// невидимым, а серый грязным, поэтому значения свои.
    #[test]
    fn light_and_dark_are_different_palettes() {
        let dark = Caps {
            color: true,
            theme: Theme::Dark,
            truecolor: true,
            unicode: true,
            width: 40,
        };
        let light = Caps {
            theme: Theme::Light,
            ..dark
        };
        for role in [Role::Text, Role::Muted, Role::Accent, Role::Warn, Role::Bad] {
            assert_ne!(
                paint(&dark, role, "х"),
                paint(&light, role, "х"),
                "роль {role:?} одинакова в обеих темах"
            );
        }
        // Подложки тоже: тёмная полоса на белом листе — дыра.
        assert_ne!(
            band(&dark, Bg::Sel, "х", 10),
            band(&light, Bg::Sel, "х", 10)
        );
    }

    /// Кластеры: буква с диакритикой, эмодзи с селектором и семья через
    /// соединитель — по одной ячейке каждая. Ошибка здесь ломает всю вёрстку.
    #[test]
    fn width_counts_clusters_not_codepoints() {
        assert_eq!(width("й"), 1);
        assert_eq!(
            width("и\u{306}"),
            1,
            "буква с комбинирующей краткой — одна ячейка"
        );
        assert_eq!(width("é\u{301}"), 1);
        assert_eq!(
            width("✅\u{FE0F}"),
            2,
            "селектор начертания места не занимает"
        );
        assert_eq!(width("👨\u{200D}👩\u{200D}👧"), 2, "семья — один кластер");
        assert_eq!(
            width("👍\u{1F3FD}"),
            2,
            "модификатор тона не добавляет ячейку"
        );
        assert_eq!(width("\u{200B}"), 0, "нулевой пробел");
    }

    /// OSC-последовательности (например ссылки) места не занимают — но в
    /// наивном подсчёте они лезут первыми и сдвигают всю строку.
    #[test]
    fn width_skips_osc_sequences() {
        let link = "\x1b]8;;https://example.com\x07текст\x1b]8;;\x07";
        assert_eq!(width(link), 5, "видно только «текст»");
        assert_eq!(strip(link), "текст");
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
    /// Обрезка крашеной строки: считать надо видимые знаки, а краску нести с
    /// собой. Иначе цветная строка режется вчетверо раньше срока, а обрывок
    /// последовательности выплёскивается на экран мусором.
    #[test]
    fn truncation_counts_letters_and_carries_the_colour() {
        let c = caps(true, true);
        let painted = paint(&c, Role::Accent, "длинная строка про всё на свете");
        let cut = truncate(&painted, 10);
        assert_eq!(width(&cut), 10, "видимая ширина обрезка: {cut:?}");
        assert!(strip(&cut).starts_with("длинная"), "{:?}", strip(&cut));
        assert!(cut.ends_with("\x1b[0m"), "краска не закрыта: {cut:?}");
        // Некрашеная строка остаётся некрашеной.
        assert_eq!(truncate("абвгде", 4), "абв…");
    }
    /// Перенос не должен ронять краску: разорванный жирный кусок обязан
    /// остаться жирным на второй строке, а первая — закрыться, иначе цвет
    /// потечёт на соседнюю подложку.
    #[test]
    fn wrapping_carries_the_colour_across_the_break() {
        let c = caps(true, true);
        let painted = format!(
            "начало {} конец",
            paint(&c, Role::Accent, "очень длинный крашеный кусок")
        );
        let lines = wrap(&painted, 20);
        assert!(lines.len() > 1, "не перенеслось: {lines:?}");
        for l in &lines {
            assert!(width(l) <= 20, "строка шире положенного: {l:?}");
        }
        // Внутри крашеного куска перенос обязан продолжить краску.
        let inside = lines
            .iter()
            .find(|l| strip(l).trim() == "крашеный кусок конец")
            .unwrap_or(&lines[1]);
        assert!(
            inside.starts_with('\x1b'),
            "продолжение без краски: {inside:?}"
        );
        // Ни одна строка не оставляет краску открытой — иначе следующая
        // строка кадра приедет чужим цветом.
        for l in &lines {
            assert!(
                sgr_open("", l).is_empty(),
                "краска осталась открытой: {l:?}"
            );
        }
        // Текст без краски переносится ровно как раньше.
        assert_eq!(wrap("раз два три", 7), vec!["раз два", "три"]);
    }
    /// Подсветка обязана попадать в буквы, а не в краску: искомое «38»
    /// встречается внутри последовательности цвета, и наивный поиск подсветил
    /// бы её.
    #[test]
    fn highlighting_lands_on_letters_not_on_colour_codes() {
        let c = caps(true, true);
        let painted = paint(&c, Role::Accent, "цвет 38 внутри");
        let lit = highlight(&c, &painted, "38");
        assert_eq!(strip(&lit), "цвет 38 внутри", "текст поехал: {lit:?}");
        assert!(lit.contains("\x1b[7m38\x1b[27m"), "не подсвечено: {lit:?}");
        // Регистр не важен, и подсвечиваются все вхождения.
        let many = highlight(&c, "Раз два раз", "РАЗ");
        assert_eq!(many.matches("\x1b[7m").count(), 2, "{many:?}");
        // Пустой запрос ничего не трогает.
        assert_eq!(highlight(&c, "текст", "  "), "текст");
    }
    /// Длинное слово рвётся по ширине, где бы оно ни стояло: раньше это
    /// работало только для первого слова строки, и путь во второй половине
    /// сообщения уезжал за край экрана.
    #[test]
    fn a_long_word_breaks_anywhere_in_the_line() {
        let path = "/очень/длинный/путь/до/сокета/узла/node.sock";
        for line in wrap(&format!("сокет: {path}"), 20) {
            assert!(width(&line) <= 20, "не влезло: {line:?} ({})", width(&line));
        }
        let joined: String = wrap(&format!("сокет: {path}"), 20).join("");
        assert!(joined.contains("node.sock"), "потеряли хвост: {joined}");
    }
}
