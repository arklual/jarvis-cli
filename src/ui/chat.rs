//! Чат сессии: разбор транскрипта и живое дочитывание.
//!
//! Дочитываем инкрементально, с последнего смещения, а не перечитываем хвост:
//! транскрипт живой сессии — это не реплики, а полные ответы инструментов, и
//! один `Read` большого файла даёт сотни килобайт. Урок мобильного клиента,
//! оплаченный там ростом памяти и подтормаживанием.

use crate::app::App;
use crate::core::node::NodeClient;
use crate::core::util::{clock, ellipsize, one_line};
use crate::ui::style::{accent, band, paint, truncate, width, wrap, Bg, Caps, Role};
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

/// Как показать строку разметки.
///
/// Раньше маркеры просто стирались, и весь ответ выглядел ровной серой стеной.
/// У pi разметка рендерится (components/markdown.ts): заголовок — весом, код —
/// своим тоном, список — значком. Здесь то же, но скупо: терминал не браузер, а
/// пять уровней заголовков в чате никому не нужны.
#[derive(Debug, Clone, PartialEq)]
pub enum Md {
    /// Обычный текст.
    Plain(String),
    /// Заголовок — весом, а не решётками. Число — уровень: h1 крупнее h3.
    Head(u8, String),
    /// Пункт списка: уровень вложенности и текст.
    Item(usize, String),
    /// Начало ```-блока: язык, если он назван.
    CodeStart(String),
    /// Строка кода внутри ```-блока.
    Code(String),
    /// Конец ```-блока.
    CodeEnd,
    /// Цитата.
    Quote(String),
    /// Горизонтальная черта.
    Rule,
}

/// Разобрать текст ответа в строки разметки.
///
/// Внутри ```-блока разметку не трогаем вовсе: там код, и «звёздочка» в нём —
/// это звёздочка, а не жирный шрифт.
pub fn markdown(text: &str) -> Vec<Md> {
    let mut out = Vec::new();
    let mut in_code = false;
    for raw in text.lines() {
        let line = raw.trim_end();
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("```") {
            if in_code {
                out.push(Md::CodeEnd);
            } else {
                out.push(Md::CodeStart(rest.trim().to_string()));
            }
            in_code = !in_code;
            continue;
        }
        if in_code {
            out.push(Md::Code(line.to_string()));
            continue;
        }
        // Черта разделяет части ответа — она и на экране черта, а не три
        // минуса посреди текста.
        if matches!(t, "---" | "***" | "___" | "- - -") {
            out.push(Md::Rule);
            continue;
        }
        // Отступ пункта — это вложенность. Считаем по пробелам до значка:
        // вложенный список без отступа читается как один плоский.
        let indent = line.len() - line.trim_start().len();
        if let Some(rest) = t.strip_prefix("> ") {
            out.push(Md::Quote(rest.to_string()));
        } else if t.starts_with('#') {
            let level = t.chars().take_while(|c| *c == '#').count().min(6) as u8;
            let title = t.trim_start_matches('#').trim();
            if title.is_empty() {
                out.push(Md::Plain(String::new()));
            } else {
                out.push(Md::Head(level, title.to_string()));
            }
        } else if let Some(rest) = t
            .strip_prefix("- ")
            .or_else(|| t.strip_prefix("* "))
            .or_else(|| t.strip_prefix("+ "))
        {
            out.push(Md::Item(indent / 2, rest.to_string()));
        } else if let Some((num, rest)) = numbered(t) {
            out.push(Md::Item(indent / 2, format!("{num}. {rest}")));
        } else {
            out.push(Md::Plain(line.to_string()));
        }
    }
    out
}

/// «1. пункт» → (1, «пункт»).
fn numbered(t: &str) -> Option<(u32, &str)> {
    let (head, rest) = t.split_once(". ")?;
    head.parse::<u32>().ok().map(|n| (n, rest))
}

/// Строчная разметка в краску: жирное — жирным, `код` — цветом, ссылка —
/// подчёркиванием.
///
/// Раньше разметку просто снимали: звёздочки мешают читать, а «рисовать нечем»
/// было неправдой — терминал умеет и вес, и курсив. Снятая разметка теряет
/// ровно то, ради чего её ставили: агент выделяет имя файла или предупреждение,
/// а человек видел ровный серый текст.
///
/// Открывающая звёздочка обязана прилегать к слову, закрывающая — тоже: иначе
/// «2 * 2 * 3» превратилось бы в курсив.
fn inline(caps: &Caps, s: &str, base: Role) -> String {
    let ch: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut plain = String::new();
    let mut i = 0usize;
    while i < ch.len() {
        let c = ch[i];
        // `код` — цветом, содержимое не разбираем: там звёздочки принадлежат
        // коду.
        if c == '`' {
            if let Some(j) = close_at(&ch, i + 1, '`', 1) {
                flush(caps, base, &mut plain, &mut out);
                out.push_str(&accent(
                    caps,
                    Role::Accent,
                    &[],
                    &ch[i + 1..j].iter().collect::<String>(),
                ));
                i = j + 1;
                continue;
            }
        }
        // ~~зачёркнутое~~
        if c == '~' && ch.get(i + 1) == Some(&'~') {
            if let Some(j) = close_at(&ch, i + 2, '~', 2) {
                flush(caps, base, &mut plain, &mut out);
                let body: String = ch[i + 2..j].iter().collect();
                out.push_str(&accent(caps, base, &[9], &inline(caps, &body, base)));
                i = j + 2;
                continue;
            }
        }
        // **жирное** и __жирное__
        if (c == '*' || c == '_') && ch.get(i + 1) == Some(&c) && word_edge(&ch, i, c) {
            if let Some(j) = close_at(&ch, i + 2, c, 2) {
                flush(caps, base, &mut plain, &mut out);
                let body: String = ch[i + 2..j].iter().collect();
                out.push_str(&accent(caps, base, &[1], &body));
                i = j + 2;
                continue;
            }
        }
        // *курсив* и _курсив_
        if (c == '*' || c == '_')
            && ch.get(i + 1).is_some_and(|n| !n.is_whitespace())
            && word_edge(&ch, i, c)
        {
            if let Some(j) = close_at(&ch, i + 1, c, 1) {
                if ch.get(j - 1).is_some_and(|p| !p.is_whitespace()) {
                    flush(caps, base, &mut plain, &mut out);
                    let body: String = ch[i + 1..j].iter().collect();
                    out.push_str(&accent(caps, base, &[3], &body));
                    i = j + 1;
                    continue;
                }
            }
        }
        // [текст](ссылка): подчёркиваем текст, а саму ссылку показываем только
        // если она говорит не то же самое.
        if c == '[' {
            if let Some((text, url, end)) = link_at(&ch, i) {
                flush(caps, base, &mut plain, &mut out);
                out.push_str(&accent(caps, Role::Accent, &[4], &text));
                if text != url {
                    out.push_str(&paint(caps, Role::Dim, &format!(" ({url})")));
                }
                i = end;
                continue;
            }
        }
        plain.push(c);
        i += 1;
    }
    flush(caps, base, &mut plain, &mut out);
    out
}

/// Сбросить накопленный обычный текст в вывод.
fn flush(caps: &Caps, base: Role, plain: &mut String, out: &mut String) {
    if !plain.is_empty() {
        out.push_str(&paint(caps, base, plain));
        plain.clear();
    }
}

/// Можно ли начинать разметку в этом месте.
///
/// Подчёркивание внутри слова — часть имени, а не курсив: `snake_case_name` не
/// должен рассыпаться на куски. Для звёздочки такого правила нет — она внутри
/// слова разметкой и считается.
fn word_edge(ch: &[char], at: usize, mark: char) -> bool {
    if mark != '_' {
        return true;
    }
    at == 0 || !ch[at - 1].is_alphanumeric()
}

/// Где закрывается разметка: `len` одинаковых знаков подряд, начиная с `from`.
/// Пустая вставка (`**` подряд) разметкой не считается.
fn close_at(ch: &[char], from: usize, mark: char, len: usize) -> Option<usize> {
    let mut i = from;
    while i + len <= ch.len() {
        if ch[i..i + len].iter().all(|c| *c == mark) && i > from {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// `[текст](ссылка)` начиная с `at`: (текст, ссылка, где кончилось).
fn link_at(ch: &[char], at: usize) -> Option<(String, String, usize)> {
    let close = ch.iter().skip(at + 1).position(|c| *c == ']')? + at + 1;
    if ch.get(close + 1) != Some(&'(') {
        return None;
    }
    let end = ch.iter().skip(close + 2).position(|c| *c == ')')? + close + 2;
    let text: String = ch[at + 1..close].iter().collect();
    let url: String = ch[close + 2..end].iter().collect();
    if text.is_empty() || url.is_empty() {
        return None;
    }
    Some((text, url, end + 1))
}

/// Разметку показываем как текст — для строк, где формат не важен.
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
        Kind::Agent => {
            let room = total.saturating_sub(4);
            let mut out = Vec::new();
            for md in markdown(&it.text) {
                match md {
                    // Заголовок — весом и краской; первый уровень ещё и
                    // подчёркнут: в длинном ответе он делит текст на части.
                    Md::Head(level, t) => {
                        let attrs: &[u8] = if level <= 1 { &[1, 4] } else { &[1] };
                        for l in wrap(&inline(caps, &t, Role::Text), room) {
                            out.push(format!("  {}", accent(caps, Role::Plain, attrs, &l)));
                        }
                    }
                    Md::Item(level, t) => {
                        // Вложенность отступом: плоский список из вложенного
                        // читается как перечисление одного уровня, а это враньё.
                        let pad = "  ".repeat(level.min(4));
                        let head = room.saturating_sub(2 + width(&pad));
                        for (i, l) in wrap(&inline(caps, &t, Role::Text), head)
                            .into_iter()
                            .enumerate()
                        {
                            let mark = if i == 0 { "·" } else { " " };
                            out.push(format!("  {pad}{} {}", paint(caps, Role::Accent, mark), l));
                        }
                    }
                    // Блок кода — с полосой слева и названием языка: так видно,
                    // где он начался и где кончился, даже посреди длинного
                    // ответа.
                    Md::CodeStart(lang) if !lang.is_empty() => out.push(format!(
                        "  {} {}",
                        paint(caps, Role::Border, "┌"),
                        paint(caps, Role::Dim, &lang)
                    )),
                    Md::CodeStart(_) => out.push(format!("  {}", paint(caps, Role::Border, "┌"))),
                    Md::CodeEnd => out.push(format!("  {}", paint(caps, Role::Border, "└"))),
                    Md::Code(t) => out.push(format!(
                        "  {} {}",
                        paint(caps, Role::Border, "│"),
                        paint(caps, Role::Muted, &truncate(&t, room.saturating_sub(2)))
                    )),
                    Md::Quote(t) => {
                        for l in wrap(&inline(caps, &t, Role::Muted), room.saturating_sub(2)) {
                            out.push(format!("  {} {}", paint(caps, Role::Border, "│"), l));
                        }
                    }
                    Md::Rule => out.push(format!(
                        "  {}",
                        paint(caps, Role::Border, &"─".repeat(room.min(60)))
                    )),
                    Md::Plain(t) if t.trim().is_empty() => out.push(String::new()),
                    Md::Plain(t) => {
                        for l in wrap(&inline(caps, &t, Role::Text), room) {
                            out.push(format!("  {l}"));
                        }
                    }
                }
            }
            out
        }
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

/// На сколько строк вырастет лента от новых записей.
///
/// Нужно ровно для одного: человек читает историю, а лента растёт снизу. Окно
/// видимого считается от конца, поэтому без поправки каждая новая строка
/// уносила бы прочитанное вверх — это и есть «прокрутка сама прыгает».
pub fn grown_lines(caps: &Caps, prev: Option<&Kind>, items: &[Item], total: usize) -> usize {
    let mut n = 0;
    let mut prev = prev.cloned();
    for it in items {
        if needs_gap(prev.as_ref(), &it.kind) {
            n += 1;
        }
        n += block(caps, it, total).len();
        prev = Some(it.kind.clone());
    }
    n
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
            theme: crate::ui::style::Theme::Dark,
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

    /// Разметка обязана становиться формой, а не исчезать: серая стена текста
    /// одинаково выглядит и для плана, и для отчёта.
    #[test]
    fn markdown_becomes_shape() {
        let md = markdown(
            "# Итог\n\nСделал:\n- первое\n- второе\n\n```\nfn main() {}\n```\n> и заметка",
        );
        assert!(matches!(&md[0], Md::Head(1, t) if t == "Итог"));
        assert!(matches!(&md[3], Md::Item(0, t) if t == "первое"));
        assert!(md
            .iter()
            .any(|m| matches!(m, Md::Code(t) if t.contains("fn main"))));
        assert!(md
            .iter()
            .any(|m| matches!(m, Md::Quote(t) if t == "и заметка")));
        // Тройные кавычки сами по себе в ленту не попадают.
        assert!(!md
            .iter()
            .any(|m| matches!(m, Md::Plain(t) if t.contains("```"))));
    }

    /// Внутри блока кода разметку не трогаем: звёздочка там — звёздочка.
    #[test]
    fn code_blocks_keep_their_asterisks() {
        let md = markdown("```rust\nlet x = a * b * c;\n```");
        assert!(matches!(&md[0], Md::CodeStart(l) if l == "rust"), "{md:?}");
        assert!(
            matches!(&md[1], Md::Code(t) if t.contains("a * b * c")),
            "{md:?}"
        );
        assert!(matches!(&md[2], Md::CodeEnd));
    }

    /// Умножение не должно превращаться в курсив.
    #[test]
    fn multiplication_survives_the_italics_rule() {
        let c = colored_caps();
        let plain = crate::ui::style::strip(&inline(&c, "площадь = 2 * 2 * 3", Role::Text));
        assert_eq!(plain, "площадь = 2 * 2 * 3");
        // А парные звёздочки вокруг слова становятся курсивом — и исчезают
        // сами, оставив на экране только слово.
        let it = inline(&c, "это *важно* сегодня", Role::Text);
        assert_eq!(crate::ui::style::strip(&it), "это важно сегодня");
        assert!(italic_on(&it), "курсив не включился: {it:?}");
    }

    /// Разметка обязана становиться начертанием, а не пропадать: агент
    /// выделяет имя файла и предупреждение, и это надо видеть.
    #[test]
    fn inline_markup_becomes_weight_and_colour() {
        let c = colored_caps();
        let bold = inline(&c, "это **важно** очень", Role::Text);
        assert_eq!(crate::ui::style::strip(&bold), "это важно очень");
        assert!(
            bold.contains("\x1b[1;") || bold.contains("\x1b[1m"),
            "жирный не включился: {bold:?}"
        );

        let code = inline(&c, "правь `src/main.rs` сегодня", Role::Text);
        assert_eq!(crate::ui::style::strip(&code), "правь src/main.rs сегодня");
        // Внутри кода разметку не разбираем: звёздочка там — звёздочка.
        let stars = inline(&c, "`a * b * c`", Role::Text);
        assert_eq!(crate::ui::style::strip(&stars), "a * b * c");

        // Ссылка: текст подчёркнут, адрес рядом — но только если он говорит не
        // то же самое.
        let link = inline(&c, "смотри [доки](https://pi.dev)", Role::Text);
        assert!(crate::ui::style::strip(&link).contains("доки (https://pi.dev)"));
        let same = inline(&c, "[https://pi.dev](https://pi.dev)", Role::Text);
        assert_eq!(crate::ui::style::strip(&same), "https://pi.dev");

        // Подчёркивание внутри имени — часть имени: `snake_case_name` не
        // рассыпается на курсив.
        let snake = inline(&c, "функция snake_case_name готова", Role::Text);
        assert_eq!(
            crate::ui::style::strip(&snake),
            "функция snake_case_name готова"
        );
        assert!(!italic_on(&snake), "{snake:?}");
    }

    /// Курсив — это ровно «3» отдельным кодом: «38;2;…» — это цвет, и
    /// проверка на «\x1b[3» ловила бы его.
    fn italic_on(s: &str) -> bool {
        s.contains("\x1b[3;") || s.contains("\x1b[3m")
    }

    fn colored_caps() -> Caps {
        Caps {
            color: true,
            theme: crate::ui::style::Theme::Dark,
            truecolor: true,
            unicode: true,
            width: 80,
        }
    }

    #[test]
    fn numbered_lists_keep_their_numbers() {
        let md = markdown("1. первое\n2. второе");
        assert!(
            matches!(&md[0], Md::Item(0, t) if t == "1. первое"),
            "{md:?}"
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
            theme: crate::ui::style::Theme::Dark,
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
            theme: crate::ui::style::Theme::Dark,
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
    /// Поправка прокрутки считается ровно по тем строкам, которые лента и
    /// нарисует: иначе окно уедет на пару строк и человек потеряет место.
    #[test]
    fn growth_is_counted_in_the_very_lines_the_feed_draws() {
        let c = colored_caps();
        let items = vec![
            Item {
                kind: Kind::Agent,
                text: "первая строка\nвторая строка".into(),
                detail: String::new(),
            },
            Item {
                kind: Kind::Tool,
                text: "Bash".into(),
                detail: "ls".into(),
            },
        ];
        let grew = grown_lines(&c, Some(&Kind::User), &items, 60);
        // Ровно столько же строк, сколько появится в самой ленте.
        let before = feed_lines(&c, &[], 60).len();
        let after = feed_lines(
            &c,
            &[
                Item {
                    kind: Kind::User,
                    text: "было".into(),
                    detail: String::new(),
                },
                items[0].clone(),
                items[1].clone(),
            ],
            60,
        )
        .len();
        let user_only = feed_lines(
            &c,
            &[Item {
                kind: Kind::User,
                text: "было".into(),
                detail: String::new(),
            }],
            60,
        )
        .len();
        assert_eq!(before, 0);
        assert_eq!(grew, after - user_only, "поправка разошлась с лентой");
    }
}
