//! Живое окно: одно приложение, в котором живут, а не набор команд.
//!
//! Команды хороши, пока действие одно. Как только человек начинает СЛЕДИТЬ —
//! кто спросил, что ответил, что на экране, — набирать `jarvis chat …`,
//! `jarvis reply …` заново каждый раз дороже, чем сама работа. Здесь тот же
//! пульт, но с клавишами: стрелки выбирают, Enter открывает чат, набранное
//! уходит агенту.
//!
//! Два решения, определяющие всё остальное:
//!
//! 1. Долгий опрос событий живёт в СВОЁЙ задаче. Узел держит `/events` до 25
//!    секунд, и если ждать его в том же цикле, что и клавиши, окно перестаёт
//!    слушаться на эти 25 секунд — нажатие «q» уходило бы в никуда.
//! 2. Экран собирается целиком в строку и печатается одним куском. Частичная
//!    перерисовка быстрее, но именно она даёт мигание и рваные строки, ради
//!    которых люди и не любят терминальные интерфейсы.

use crate::app::{registry, App};
use crate::core::machine;
use crate::core::node::NodeClient;
use crate::core::session::{self, Session};
use crate::core::state;
use crate::core::util::now_ms;
use crate::ui::chat::{self, Feed, Item};
use crate::ui::editor::Editor;
use crate::ui::form::{Form, Kind};
use crate::ui::render::{self, Window};
use crate::ui::slash;
use crate::ui::style::{band, header, key, pad, paint, truncate, width, Bg, Caps, Role};
use crossterm::event::{poll, read, Event, KeyCode, KeyEvent, KeyModifiers};
use std::io::Write;
use std::time::Duration;

/// Как часто спрашиваем клавиши. Меньше — жжём процессор впустую, больше —
/// набор текста начинает «залипать».
const TICK: Duration = Duration::from_millis(80);
/// Лента чата дочитывается не на каждый тик: файл читается по сети.
const FEED_EVERY: i64 = 500;
const SCREEN_EVERY: i64 = 1200;
const LIMITS_EVERY: i64 = 5 * 60_000;
/// Файлы состояния читаются с диска — не чаще, чем нужно глазу.
const DISK_EVERY: i64 = 1000;
/// Потолок ленты в памяти: чат живёт часами, и без предела окно однажды
/// съедает гигабайт — урок настольной версии.
const MAX_ITEMS: usize = 2000;

/// Что набирают в строке ввода. Пустой ввод в чате — тоже набор: там строка
/// всегда наготове, и это единственный вид, где буквы принадлежат тексту.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Typing {
    None,
    /// Ответ агенту в чате.
    Message,
    /// Задача новой руки связки.
    Task,
    /// Команда окна: строка начинается со слэша.
    Command,
    /// Ответ на вопрос формы (заведение связки).
    Field,
}

#[derive(Debug, Clone, PartialEq)]
enum View {
    List,
    Chat,
    Screen,
    Loops,
    /// Журнал одного цикла: итерации и чем кончилось.
    Loop,
    Bundles,
    /// Пульт одной связки: руки, очередь, действия.
    Bundle,
    Help,
}

/// Что человек хотел сказать нажатием.
#[derive(Debug, Clone, PartialEq)]
pub enum Act {
    Quit,
    Up,
    Down,
    Open,
    Screen,
    Interrupt,
    Answer(u8),
    Loops,
    Bundles,
    Help,
    Escape,
    Send,
    Type(char),
    Backspace,
    KillLine,
    PageUp,
    PageDown,
    /// Влить голову очереди связки.
    Merge,
    /// Пауза связке и обратно.
    Pause,
    /// Новая рука: спросить задачу.
    NewHand,
    /// Убрать выбранное (связку) — со вторым нажатием как подтверждением.
    Remove,
    /// Правка строки ввода.
    WordLeft,
    WordRight,
    KillWordLeft,
    KillWordRight,
    KillToEnd,
    Yank,
    YankPop,
    Undo,
    /// Вставка из буфера обмена — приходит куском, а не по буквам.
    Paste(String),
    Left,
    Right,
    Home,
    End,
    Delete,
    /// Перевод строки внутри сообщения.
    Newline,
    /// Дополнить команду.
    Tab,
    /// Начать команду с «/».
    Slash,
    None,
}

/// Клавиша в намерение. Отдельной функцией — чтобы раскладку проверяли тесты,
/// а не пальцы: в чате «j» это буква, в списке — «вниз», и перепутать их
/// значит писать сообщения вместо навигации.
pub fn map_key(view_is_text: bool, k: KeyEvent) -> Act {
    // Alt+клавиша терминал шлёт как ESC перед этой клавишей — то же, что
    // «нажали Esc, потом её». Считаем это Escape: иначе быстрый Esc перед
    // следующим нажатием слипается в Alt и молча превращается в букву.
    // Alt-сочетания разбираем ДО общего правила про Alt: иначе они
    // превращаются в Escape и стирают набранное.
    if k.modifiers.contains(KeyModifiers::ALT) {
        return match k.code {
            KeyCode::Enter => Act::Newline,
            // Слова — как в readline: Alt+B/F ходят, Alt+D стирает вперёд,
            // Alt+Y листает кольцо убитого.
            KeyCode::Left | KeyCode::Char('b') => Act::WordLeft,
            KeyCode::Right | KeyCode::Char('f') => Act::WordRight,
            KeyCode::Char('d') => Act::KillWordRight,
            KeyCode::Char('y') => Act::YankPop,
            _ => Act::Escape,
        };
    }
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    if ctrl {
        return match k.code {
            KeyCode::Char('c') => Act::Quit,
            KeyCode::Char('u') => Act::KillLine,
            // Терминалы, где Alt+Enter не доходит, оставляют Ctrl+O — это
            // общий для readline способ сказать «перевод строки».
            KeyCode::Char('o') => Act::Newline,
            KeyCode::Char('a') => Act::Home,
            KeyCode::Char('e') => Act::End,
            KeyCode::Char('w') => Act::KillWordLeft,
            KeyCode::Char('k') => Act::KillToEnd,
            KeyCode::Char('y') => Act::Yank,
            // Ctrl+Z и Ctrl+_ — две привычные отмены; в терминале до нас
            // доезжает то одна, то другая.
            KeyCode::Char('z') | KeyCode::Char('_') => Act::Undo,
            // Ctrl+D намеренно НЕ «удалить символ»: этим кодом терминал
            // сообщает о конце ввода, и он прилетает сам, когда стдин
            // закрывается. Поймано следом нажатий: символ под курсором
            // исчезал без единого нажатия человека.
            // В сыром режиме перевод строки приходит как Ctrl+J, а не Enter:
            // так его отдают часть терминалов и всё, что печатает в пану
            // программно. Для человека это тот же Enter.
            KeyCode::Char('j') => {
                if view_is_text {
                    Act::Send
                } else {
                    Act::Open
                }
            }
            _ => Act::None,
        };
    }
    match k.code {
        KeyCode::Esc => Act::Escape,
        KeyCode::Enter => {
            if view_is_text {
                Act::Send
            } else {
                Act::Open
            }
        }
        KeyCode::Up => Act::Up,
        KeyCode::Down => Act::Down,
        KeyCode::Left => Act::Left,
        KeyCode::Right => Act::Right,
        KeyCode::Home => Act::Home,
        KeyCode::End => Act::End,
        KeyCode::Delete => Act::Delete,
        KeyCode::Tab => Act::Tab,
        KeyCode::PageUp => Act::PageUp,
        KeyCode::PageDown => Act::PageDown,
        KeyCode::Backspace => Act::Backspace,
        // Слэш открывает командную строку из любого вида: команды `jarvis`
        // человек уже знает, и заново учить клавиши окна незачем.
        KeyCode::Char('/') if !view_is_text => Act::Slash,
        KeyCode::Char(c) if view_is_text => Act::Type(c),
        KeyCode::Char('q') => Act::Quit,
        KeyCode::Char('j') => Act::Down,
        KeyCode::Char('k') => Act::Up,
        KeyCode::Char('s') => Act::Screen,
        KeyCode::Char('x') => Act::Interrupt,
        KeyCode::Char('l') => Act::Loops,
        KeyCode::Char('b') => Act::Bundles,
        KeyCode::Char('m') => Act::Merge,
        KeyCode::Char('p') => Act::Pause,
        KeyCode::Char('n') => Act::NewHand,
        KeyCode::Char('d') => Act::Remove,
        KeyCode::Char('?') | KeyCode::Char('h') => Act::Help,
        KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => Act::Answer(c as u8 - b'0'),
        _ => Act::None,
    }
}

/// Какие записи ленты видны при высоте `rows` и прокрутке `scroll` снизу.
/// Чистая функция: край ленты — место, где легко ошибиться на единицу и
/// показать человеку чужой конец разговора.
pub fn visible<T>(items: &[T], rows: usize, scroll: usize) -> &[T] {
    if items.is_empty() || rows == 0 {
        return &[];
    }
    let scroll = scroll.min(items.len().saturating_sub(1));
    let end = items.len() - scroll;
    let start = end.saturating_sub(rows);
    &items[start..end]
}

struct Ui {
    view: View,
    prev: View,
    sel: usize,
    input: Editor,
    /// Что сейчас набирают: ответ агенту, задачу руке или команду.
    typing: Typing,
    status: String,
    scroll: usize,
    feed: Option<Feed>,
    items: Vec<Item>,
    open_id: String,
    open_title: String,
    open_pane: String,
    screen: String,
    limits: Vec<Window>,
    limits_at: i64,
    feed_at: i64,
    screen_at: i64,
    /// Связки и циклы читаются с диска — не на каждый кадр, а раз в секунду.
    bundles: Vec<state::Bundle>,
    loops: Vec<state::Loop>,
    disk_at: i64,
    /// Выбранные цикл, связка и рука.
    lsel: usize,
    open_loop: String,
    bsel: usize,
    hsel: usize,
    open_bundle: String,
    /// Где мы работаем: связка заводится на этой же машине, руки поднимутся
    /// там же, где за ними смотрят.
    machine: String,
    /// Счётчик кадров — им крутится спиннер.
    tick: u64,
    /// Что уже просили убрать: второе нажатие подтверждает.
    ///
    /// Подтверждение клавишей, а не окном с кнопками: промах по «d» не должен
    /// стирать связку, но и городить модальное окно ради одного вопроса
    /// незачем.
    confirm_rm: Option<String>,
    /// Заводим связку: форма спрашивает по одному вопросу.
    form: Option<Form>,
    /// Долгое действие уже идёт. Второе нажатие «влить» подряд — это два
    /// слияния, а очередь такого не прощает. Время начала — чтобы человек
    /// видел не только «идёт», но и «сколько уже».
    busy: Option<(String, i64)>,
}

impl Default for Ui {
    fn default() -> Self {
        Self {
            view: View::List,
            prev: View::List,
            sel: 0,
            input: Editor::default(),
            typing: Typing::None,
            status: String::new(),
            scroll: 0,
            feed: None,
            items: Vec::new(),
            open_id: String::new(),
            open_title: String::new(),
            open_pane: String::new(),
            screen: String::new(),
            limits: Vec::new(),
            limits_at: 0,
            feed_at: 0,
            screen_at: 0,
            bundles: Vec::new(),
            loops: Vec::new(),
            disk_at: 0,
            lsel: 0,
            open_loop: String::new(),
            bsel: 0,
            hsel: 0,
            open_bundle: String::new(),
            machine: "local".into(),
            confirm_rm: None,
            form: None,
            busy: None,
            tick: 0,
        }
    }
}

/// Запустить окно. Возвращается, когда человек вышел.
pub async fn run(app: &App, machine_name: &str) -> Result<(), String> {
    let m = machine::list()
        .into_iter()
        .find(|m| m.name == machine_name)
        .ok_or_else(|| format!("нет машины «{machine_name}»"))?;
    let (client, _tunnel) = machine::connect(&m).await?;

    let mut reg = registry(&client).await.unwrap_or_default();
    let cursor = client.hello().await.map(|h| h.cursor).unwrap_or(0);

    // Долгий опрос — в свою задачу: иначе окно глохнет на время ожидания.
    let (tx, mut rx) = tokio::sync::mpsc::channel(32);
    let poller = tokio::spawn(events_task(client.clone(), cursor, tx.clone()));

    let raw = RawGuard::enable();
    // Файлы состояния читаем ДО первого кадра: иначе первые доли секунды окно
    // честно показывает «связок пока нет» при полном файле связок, а нажатая в
    // этот момент клавиша разговаривает с пустотой.
    let mut ui = Ui {
        machine: m.name.clone(),
        bundles: state::load_bundles(),
        loops: state::load_loops(),
        disk_at: now_ms(),
        ..Default::default()
    };
    let mut list = session::sorted(&reg);
    let mut shown = String::new();
    let mut was_size = (0u16, 0u16);

    loop {
        let (cols, rows) = size();
        let caps = Caps {
            width: cols,
            ..app.caps
        };
        if (cols, rows) != was_size {
            // Окно изменили: прошлый кадр больше ни о чём не говорит, а на
            // экране остались куски старой раскладки — рисуем начисто.
            was_size = (cols, rows);
            shown.clear();
            print!("\x1b[2J");
        }
        // Пока идёт долгая работа, кадр обязан меняться: спиннер и есть
        // единственный признак жизни. В покое тик не трогаем — иначе экран
        // перерисовывался бы вхолостую двенадцать раз в секунду.
        if ui.busy.is_some() {
            ui.tick = ui.tick.wrapping_add(1);
        }
        present(&frame(&caps, &m.name, &ui, &list, rows), &mut shown);

        // 1. Клавиши — первым делом: отклик важнее свежести данных.
        while poll(TICK).map_err(|e| e.to_string())? {
            let act = match read().map_err(|e| e.to_string())? {
                Event::Key(k) => map_key(typing_now(&ui), k),
                // Вставка приходит целым куском — так её и кладём в поле.
                Event::Paste(text) => Act::Paste(text),
                _ => continue,
            };
            keylog_act(&act, &ui);
            if !handle(&mut ui, act, &client, &list, &caps, &tx).await {
                drop(raw);
                poller.abort();
                return Ok(());
            }
        }

        // 2. События узла, накопившиеся в канале.
        let mut changed = false;
        while let Ok(msg) = rx.try_recv() {
            match msg {
                Msg::Events(evs) => {
                    reg = session::apply(&reg, &evs);
                    changed = true;
                }
                Msg::Gap => {
                    reg = registry(&client).await.unwrap_or_default();
                    changed = true;
                }
                Msg::Loop(n) => {
                    ui.status = loop_line(n);
                    // Журнал цикла лежит в файле — перечитаем его к кадру.
                    ui.disk_at = 0;
                }
                Msg::Done(res) => {
                    // Долгое действие закончилось: снимаем занятость и
                    // показываем итог там же, где человек его ждёт.
                    let took = ui
                        .busy
                        .take()
                        .map(|(_, since)| crate::ui::style::elapsed(now_ms() - since))
                        .unwrap_or_default();
                    ui.status = match res {
                        // Сколько это заняло — часть итога: «влито» без времени
                        // не даёт понять, дорого ли обошлось.
                        Ok(report) => format!("{} · {took}", report.join(" · ")),
                        Err(e) => format!("не вышло: {e} · {took}"),
                    };
                    ui.disk_at = 0; // связки на диске изменились — перечитать
                }
            }
        }
        if changed {
            // Выделение держим за сессией, а не за номером строки: список
            // пересортировывается сам, и «курсор уехал на чужой чат» —
            // готовый способ ответить не тому.
            let keep = list.get(ui.sel).map(|s| s.id.clone());
            list = session::sorted(&reg);
            if let Some(id) = keep {
                if let Some(i) = list.iter().position(|s| s.id == id) {
                    ui.sel = i;
                }
            }
            ui.sel = ui.sel.min(list.len().saturating_sub(1));
        }

        // 3. То, что подгружается по виду.
        refresh(&mut ui, &client).await;
    }
}

enum Msg {
    Events(Vec<crate::core::node::Recorded>),
    Gap,
    /// Долгое действие закончилось: что сказать человеку.
    Done(Result<Vec<String>, String>),
    /// Цикл рассказывает, что с ним происходит.
    Loop(crate::engine::loops::Note),
}

/// Отдельная задача: сидит на долгом опросе и складывает события в канал.
async fn events_task(client: NodeClient, mut cursor: u64, tx: tokio::sync::mpsc::Sender<Msg>) {
    loop {
        match client.events(cursor).await {
            Ok(page) => {
                cursor = page.cursor;
                let msg = if page.gap {
                    Msg::Gap
                } else if page.events.is_empty() {
                    continue;
                } else {
                    Msg::Events(page.events)
                };
                if tx.send(msg).await.is_err() {
                    return; // окно закрылось
                }
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(900)).await,
        }
    }
}

/// Перечитать связки с диска и найти нужную.
///
/// Именно с диска, а не из памяти окна: пока человек смотрел на экран, связку
/// мог поменять и второй терминал, и панель — состояние у них общее.
fn load_bundle(id: &str) -> Option<(Vec<state::Bundle>, usize)> {
    let all = state::load_bundles();
    let i = all.iter().position(|b| b.id == id)?;
    Some((all, i))
}

fn toggle_pause(id: &str, on: bool) -> Result<(), String> {
    let (mut all, i) = load_bundle(id).ok_or("связка пропала из файла")?;
    all[i].paused = on;
    all[i].event(if on {
        "пауза всем".to_string()
    } else {
        "связка продолжает".to_string()
    });
    state::save_bundles(&all).map_err(|e| format!("не записал состояние: {e}"))
}

/// Завести связку: спрашиваем по одному вопросу в той же строке ввода.
async fn start_form(ui: &mut Ui, kind: Kind) {
    // Каталог по умолчанию спрашиваем у той машины, где будем работать: на
    // сервере путь этого компьютера бессмыслен, а «./» бессмыслен вдвойне.
    let cwd = match machine::find(&ui.machine) {
        Ok(m) if m.is_local() => std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| ".".into()),
        Ok(m) => {
            let (code, home) = machine::run(
                &m,
                "/",
                "printf %s \"$HOME\"",
                std::time::Duration::from_secs(15),
            )
            .await;
            if code == 0 && !home.trim().is_empty() {
                home.trim().to_string()
            } else {
                "~".into()
            }
        }
        Err(e) => {
            ui.status = e;
            return;
        }
    };
    ui.form = Some(match kind {
        Kind::Bundle => Form::new(&cwd),
        Kind::Loop => Form::new_loop(&cwd),
    });
    ui.typing = Typing::Field;
    ui.input.clear();
    ui.status.clear();
}

/// След нажатий в файл — по просьбе `JARVIS_KEYLOG`.
///
/// «Клавиша не работает» — жалоба, которую без записи не проверить: терминалы
/// шлют одну и ту же клавишу по-разному, а часть кодов приходит сама (Ctrl+D
/// при закрытии ввода). Один такой след уже нашёл здесь съеденную букву.
fn keylog_act(act: &Act, ui: &Ui) {
    let Ok(path) = std::env::var("JARVIS_KEYLOG") else {
        return;
    };
    use std::io::Write as _;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "{act:?} | {:?}", ui.input.text());
    }
}

/// Строка ввода сейчас принимает буквы: в чате она всегда наготове, в
/// остальных видах — только когда что-то набирают.
fn typing_now(ui: &Ui) -> bool {
    ui.typing != Typing::None
}

/// Куда вернуться после команды: в чате строка ввода остаётся наготове, в
/// остальных видах буквы снова принадлежат навигации.
fn idle_typing(ui: &Ui) -> Typing {
    if ui.view == View::Chat {
        Typing::Message
    } else {
        Typing::None
    }
}

/// Подходящие значения аргумента для показа под строкой ввода.
fn arg_palette(ui: &Ui, list: &[Session]) -> Vec<String> {
    let line = ui.input.text();
    if !line.starts_with('/') || line.starts_with("//") {
        return Vec::new();
    }
    let head = line.split_whitespace().next().unwrap_or("/");
    if !line[head.len()..].starts_with(' ') {
        return Vec::new();
    }
    let name = head.trim_start_matches('/').to_lowercase();
    let part = line[head.len()..].trim_start();
    let cands = arg_candidates(ui, list, &name, part);
    slash::rank(part, &cands)
}

/// Что подставлять вторым словом команды: живые сессии, каталоги, слова.
///
/// Кандидаты берём из того, что человек видит прямо сейчас: список сессий
/// перед ним, каталоги — под ним. Придуманные подсказки хуже отсутствующих.
fn arg_candidates(ui: &Ui, list: &[Session], name: &str, part: &str) -> Vec<String> {
    match slash::arg_of(name) {
        slash::Arg::Session => {
            let mut out: Vec<String> = list.iter().map(|s| s.title()).collect();
            out.sort();
            out.dedup();
            out
        }
        slash::Arg::Word(words) => words.iter().map(|w| w.to_string()).collect(),
        slash::Arg::Dir => dir_candidates(part),
        _ => {
            let _ = ui;
            Vec::new()
        }
    }
}

/// Каталоги по началу пути. Только каталоги: `/run` запускает агента в
/// каталоге, и подсовывать файлы значило бы предлагать заведомо неверное.
fn dir_candidates(part: &str) -> Vec<String> {
    let raw = crate::core::util::expand_tilde(part);
    let (dir, prefix) = if part.ends_with('/') {
        (raw.clone(), String::new())
    } else {
        (
            raw.parent().map(|p| p.to_path_buf()).unwrap_or_default(),
            raw.file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default(),
        )
    };
    let dir = if dir.as_os_str().is_empty() {
        std::path::PathBuf::from(".")
    } else {
        dir
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            // Скрытые каталоги показываем, только когда их спросили точкой:
            // иначе список — это .git, .cache и прочее, чего никто не искал.
            if name.starts_with('.') && !prefix.starts_with('.') {
                return None;
            }
            if !prefix.is_empty() && !name.to_lowercase().starts_with(&prefix.to_lowercase()) {
                return None;
            }
            Some(format!(
                "{}/{name}",
                dir.to_string_lossy().trim_end_matches('/')
            ))
        })
        .collect();
    out.sort();
    out.truncate(30);
    out
}

/// Палитра показывается, пока строка похожа на команду.
fn palette(ui: &Ui) -> Vec<&'static slash::Command> {
    let line = ui.input.text();
    if !line.starts_with('/') || line.starts_with("//") {
        return Vec::new();
    }
    let head = line.split_whitespace().next().unwrap_or("/");
    // Как только команда набрана целиком и пошли аргументы, палитра лишняя.
    if line.contains(char::is_whitespace) && slash::matching(head).len() == 1 {
        return Vec::new();
    }
    slash::matching(head)
}

/// Выбранный цикл: в списке — по курсору, в журнале — открытый.
fn current_loop(ui: &Ui) -> Option<&state::Loop> {
    if ui.view == View::Loop {
        ui.loops.iter().find(|l| l.id == ui.open_loop)
    } else {
        ui.loops.get(ui.lsel)
    }
}

/// Пометить прогон остановленным — то же, что делает команда.
fn stop_loop(id: &str) -> Result<(), String> {
    let Some(mut run) = state::load_run(id) else {
        return Err("этот цикл не запускался".into());
    };
    run.state = state::RunState::Stopped;
    run.stop = state::StopReason::Stopped;
    run.stop_note = "остановлен из окна".into();
    run.ended_at = now_ms();
    state::save_run(&run).map_err(|e| format!("не записал журнал: {e}"))
}

/// Ход цикла одной строкой для строки состояния.
fn loop_line(n: crate::engine::loops::Note) -> String {
    use crate::engine::loops::Note;
    match n {
        Note::Head { branch, .. } => format!("прогон в {branch}"),
        Note::Started(i) => format!("итерация {i} — пошла"),
        Note::Done(it) => format!(
            "итерация {} — {}",
            it.n,
            match it.verdict {
                state::Verdict::Passed => "прошла",
                state::Verdict::Returned => "возврат критика",
                state::Verdict::GateFailed => "красный гейт",
                state::Verdict::Failed => "сорвалась",
                state::Verdict::Running => "идёт",
            }
        ),
        Note::Ask { question, .. } => format!("цикл спрашивает: {question}"),
        Note::Finished {
            reason,
            iterations,
            tokens,
            ..
        } => format!(
            "{} · {} · {} токенов",
            reason.word(),
            crate::core::util::plural(iterations as u64, "итерация", "итерации", "итераций"),
            render::fmt_tokens(tokens)
        ),
    }
}

/// Открытая сейчас связка.
fn current_bundle(ui: &Ui) -> Option<&state::Bundle> {
    ui.bundles.iter().find(|b| b.id == ui.open_bundle)
}

/// Кого вливаем: голову очереди. Решение принимает человек, но выбирать ему
/// не из чего — порядок задан готовностью, и вливается только первый.
fn queue_head(b: &state::Bundle) -> Option<String> {
    b.queue().first().map(|h| h.name.clone())
}

/// Ответ `false` означает «человек вышел».
async fn handle(
    ui: &mut Ui,
    act: Act,
    client: &NodeClient,
    list: &[Session],
    caps: &Caps,
    tx: &tokio::sync::mpsc::Sender<Msg>,
) -> bool {
    let cur = list.get(ui.sel).cloned();
    match act {
        Act::Quit => return false,
        // Отмена набора — отдельный шаг: Esc в наборе не должен ещё и
        // выбрасывать из вида, где человек стоит.
        Act::Escape if ui.confirm_rm.is_some() => {
            ui.confirm_rm = None;
            ui.status = "не убираю".into();
        }
        Act::Escape if ui.typing == Typing::Field => {
            ui.form = None;
            ui.input.clear();
            ui.typing = Typing::None;
            ui.status = "не завожу".into();
        }
        Act::Escape if matches!(ui.typing, Typing::Command | Typing::Task) => {
            ui.input.clear();
            ui.typing = idle_typing(ui);
            ui.status.clear();
        }
        Act::Escape if ui.view == View::Chat && !ui.input.is_empty() => ui.input.clear(),
        Act::Escape => match ui.view {
            View::List => return false,
            View::Bundle => {
                ui.view = View::Bundles;
                ui.open_bundle.clear();
                ui.typing = Typing::None;
            }
            _ => {
                ui.view = View::List;
                ui.feed = None;
                ui.items.clear();
                ui.scroll = 0;
                ui.typing = Typing::None;
            }
        },
        // В многострочном сообщении стрелки принадлежат курсору: иначе
        // вторую строку не поправить, не стерев её целиком.
        Act::Up if typing_now(ui) && ui.input.is_multiline() => ui.input.up(),
        Act::Down if typing_now(ui) && ui.input.is_multiline() => ui.input.down(),
        // Однострочный ввод с набранным текстом или уже гуляющий по истории —
        // это шелл: вверх поднимает прошлое отправленное.
        Act::Up if ui.view == View::Chat && (!ui.input.is_empty() || ui.input.in_history()) => {
            ui.input.history_prev();
        }
        Act::Down if ui.view == View::Chat && ui.input.in_history() => {
            ui.input.history_next();
        }
        Act::Up => match ui.view {
            View::List => ui.sel = ui.sel.saturating_sub(1),
            View::Chat => ui.scroll += 1,
            View::Loops => ui.lsel = ui.lsel.saturating_sub(1),
            View::Bundles => ui.bsel = ui.bsel.saturating_sub(1),
            View::Bundle => ui.hsel = ui.hsel.saturating_sub(1),
            _ => {}
        },
        Act::Down => match ui.view {
            View::List => ui.sel = (ui.sel + 1).min(list.len().saturating_sub(1)),
            View::Chat => ui.scroll = ui.scroll.saturating_sub(1),
            View::Loops => ui.lsel = (ui.lsel + 1).min(ui.loops.len().saturating_sub(1)),
            View::Bundles => {
                ui.bsel = (ui.bsel + 1).min(ui.bundles.len().saturating_sub(1));
            }
            View::Bundle => {
                let hands = current_bundle(ui).map(|b| b.hands.len()).unwrap_or(0);
                ui.hsel = (ui.hsel + 1).min(hands.saturating_sub(1));
            }
            _ => {}
        },
        Act::PageUp => ui.scroll += 10,
        Act::PageDown => ui.scroll = ui.scroll.saturating_sub(10),
        Act::Open if ui.view == View::Loops => {
            let Some(l) = ui.loops.get(ui.lsel).cloned() else {
                ui.status = "циклов пока нет — n заведёт".into();
                return true;
            };
            ui.open_loop = l.id;
            ui.view = View::Loop;
            ui.status.clear();
        }
        Act::Open if ui.view == View::Bundles => {
            let Some(b) = ui.bundles.get(ui.bsel).cloned() else {
                ui.status = "связок пока нет — заведи: jarvis bundle new".into();
                return true;
            };
            ui.open_bundle = b.id;
            ui.hsel = 0;
            ui.view = View::Bundle;
            ui.status.clear();
        }
        Act::Open if ui.view == View::Bundle => {
            // Рука — это обычный чат в своём worktree; открываем его тем же
            // путём, что и любую сессию: по совпадению каталога.
            let Some(b) = current_bundle(ui).cloned() else {
                return true;
            };
            let Some(h) = b.hands.get(ui.hsel).cloned() else {
                ui.status = "рук пока нет — заведи клавишей n".into();
                return true;
            };
            let found = list
                .iter()
                .find(|s| {
                    s.cwd.as_deref() == Some(h.worktree.as_str()) || s.pane == Some(h.pane.clone())
                })
                .cloned();
            match found.and_then(|s| s.transcript.clone().map(|p| (s, p))) {
                Some((s, path)) => match Feed::open(client, &path).await {
                    Ok(f) => {
                        ui.feed = Some(f);
                        ui.items.clear();
                        ui.scroll = 0;
                        ui.feed_at = 0;
                        ui.open_id = s.id.clone();
                        ui.open_title = format!("{} · {}", b.name, h.name);
                        ui.view = View::Chat;
                        ui.typing = Typing::Message;
                        ui.status.clear();
                    }
                    Err(e) => ui.status = e,
                },
                None => ui.status = format!("{}: сессия ещё не отозвалась", h.name),
            }
        }
        Act::Open => {
            let Some(s) = cur else {
                ui.status = "некого открывать".into();
                return true;
            };
            match s.transcript.clone() {
                Some(path) if !path.is_empty() => match Feed::open(client, &path).await {
                    Ok(f) => {
                        ui.feed = Some(f);
                        ui.items.clear();
                        ui.scroll = 0;
                        ui.feed_at = 0;
                        ui.open_id = s.id.clone();
                        ui.open_title = s.title();
                        ui.view = View::Chat;
                        ui.typing = Typing::Message;
                        ui.status.clear();
                    }
                    Err(e) => ui.status = e,
                },
                _ => ui.status = "у сессии ещё нет транскрипта".into(),
            }
        }
        Act::Screen if matches!(ui.view, View::Loops | View::Loop) => {
            let Some(l) = current_loop(ui).cloned() else {
                ui.status = "циклов пока нет — n заведёт".into();
                return true;
            };
            if ui.busy.is_some() {
                ui.status = "погоди, прошлое действие ещё идёт".into();
                return true;
            }
            let problems = l.problems();
            if !problems.is_empty() {
                // Цикл без цели или без стен не запускаем: он будет крутиться
                // непонятно за чем и остановится непонятно когда.
                ui.status = problems.join(" · ");
                return true;
            }
            ui.busy = Some((format!("цикл {}", l.name), now_ms()));
            ui.status = format!("{}: прогон пошёл", l.name);
            ui.open_loop = l.id.clone();
            ui.view = View::Loop;
            let tx2 = tx.clone();
            tokio::spawn(async move {
                let tx3 = tx2.clone();
                let mut note = move |n| {
                    // Отчёт не должен ждать: очередь полна — переживём, ход
                    // всё равно лежит в журнале.
                    let _ = tx3.try_send(Msg::Loop(n));
                };
                let res = crate::engine::loops::start(&l, &mut note)
                    .await
                    .map(|_| vec![format!("цикл «{}» отработал", l.name)]);
                let _ = tx2.send(Msg::Done(res)).await;
            });
        }
        Act::Interrupt if matches!(ui.view, View::Loops | View::Loop) => {
            let Some(l) = current_loop(ui).cloned() else {
                return true;
            };
            // Останавливаем через журнал: движок спрашивает его перед каждой
            // итерацией, и так же это делает команда `jarvis loop stop`.
            ui.status = match stop_loop(&l.id) {
                Ok(_) => format!("{}: остановится после текущей итерации", l.name),
                Err(e) => e,
            };
            ui.disk_at = 0;
        }
        Act::Screen => {
            let Some(s) = cur else { return true };
            match s.pane.clone() {
                Some(p) if !p.is_empty() => {
                    ui.open_id = s.id.clone();
                    ui.open_title = s.title();
                    ui.open_pane = p.clone();
                    ui.screen = client.screen(&p).await.unwrap_or_default();
                    ui.screen_at = now_ms();
                    ui.view = View::Screen;
                }
                _ => ui.status = format!("{} не в tmux — экрана нет", s.title()),
            }
        }
        Act::Interrupt => {
            let Some(s) = cur else { return true };
            match s.pane.clone() {
                Some(p) if !p.is_empty() => {
                    ui.status = match client.keys(&p, crate::core::node::key_plan("Escape")).await {
                        Ok(_) => format!("прервал {}", s.title()),
                        Err(e) => e,
                    }
                }
                _ => ui.status = "нечего прерывать: сессия не в tmux".into(),
            }
        }
        Act::Answer(n) => {
            let Some(s) = cur else { return true };
            // Цифра отвечает на вопрос с меню — ровно как `jarvis answer`.
            match s.pane.clone() {
                Some(p) if !p.is_empty() => {
                    let keys = crate::core::node::key_plan(&n.to_string());
                    ui.status = match client.keys(&p, keys).await {
                        Ok(_) => format!("{}: выбран вариант {n}", s.title()),
                        Err(e) => e,
                    }
                }
                _ => ui.status = "сессия не в tmux — вариант не отправить".into(),
            }
        }
        Act::Send if ui.typing == Typing::Field => {
            let answer = ui.input.text().to_string();
            let Some(form) = ui.form.as_mut() else {
                ui.typing = Typing::None;
                return true;
            };
            if !form.accept(&answer) {
                // Либо следующий вопрос, либо тот же — когда ответ не понят.
                ui.input.clear();
                ui.status.clear();
                return true;
            }
            let form = ui.form.take().unwrap();
            ui.input.clear();
            ui.typing = Typing::None;
            let problems = form.problems();
            if !problems.is_empty() {
                ui.status = problems.join(" · ");
                return true;
            }
            match form.kind {
                Kind::Bundle => {
                    let b = form.build(&ui.machine);
                    let mut all = state::load_bundles();
                    all.push(b.clone());
                    match state::save_bundles(&all) {
                        Ok(_) => {
                            ui.bundles = all;
                            ui.disk_at = now_ms();
                            // Сразу в пульт новой связки: следующий шаг —
                            // завести руку, и он должен быть под рукой, а не
                            // через два экрана.
                            ui.open_bundle = b.id.clone();
                            ui.bsel = ui.bundles.len().saturating_sub(1);
                            ui.hsel = 0;
                            ui.view = View::Bundle;
                            ui.status = format!("связка «{}» заведена · n — первая рука", b.name);
                        }
                        Err(e) => ui.status = format!("не записал связки: {e}"),
                    }
                }
                Kind::Loop => {
                    let l = form.build_loop(&ui.machine);
                    let mut all = state::load_loops();
                    all.push(l.clone());
                    match state::save_loops(&all) {
                        Ok(_) => {
                            ui.loops = all;
                            ui.disk_at = now_ms();
                            ui.view = View::Loops;
                            // Про запуск говорим честно: цикл идёт часами и
                            // печатает ход, и это работа командной строки, а
                            // не окна.
                            ui.status = format!(
                                "цикл «{}» заведён · запуск: jarvis loop start {}",
                                l.name, l.name
                            );
                        }
                        Err(e) => ui.status = format!("не записал циклы: {e}"),
                    }
                }
            }
        }
        Act::Send if ui.typing == Typing::Task => {
            let task = ui.input.text().trim().to_string();
            if task.is_empty() {
                ui.status = "рука без задачи не поднимется".into();
                return true;
            }
            let Some(b) = current_bundle(ui).cloned() else {
                return true;
            };
            ui.typing = idle_typing(ui);
            ui.input.clear();
            ui.busy = Some(("поднимаю руку".to_string(), now_ms()));
            ui.status = "поднимаю руку: worktree, ветка, агент…".into();
            let (tx, id) = (tx.clone(), b.id.clone());
            tokio::spawn(async move {
                let res = match load_bundle(&id) {
                    Some((all, i)) => crate::engine::bundle::add_hand(all, i, &task).await,
                    None => Err("связка пропала из файла".into()),
                };
                let _ = tx.send(Msg::Done(res)).await;
            });
        }
        Act::Send => {
            let line = ui.input.text().to_string();
            if line.trim().is_empty() {
                return true;
            }
            match slash::parse(&line) {
                slash::Line::Cmd { name, rest } => {
                    ui.input.clear();
                    let alive = run_slash(ui, &name, &rest, client, list, caps, tx).await;
                    ui.typing = idle_typing(ui);
                    return alive;
                }
                slash::Line::Text(text) => {
                    if ui.view != View::Chat {
                        ui.status = "это не команда — начни со слэша".into();
                        return true;
                    }
                    let pane = list
                        .iter()
                        .find(|s| s.id == ui.open_id)
                        .and_then(|s| s.pane.clone())
                        .unwrap_or_default();
                    if pane.is_empty() {
                        ui.status = "сессия не в tmux — отвечать некуда".into();
                        return true;
                    }
                    match client.reply(&pane, &text).await {
                        Ok(_) => {
                            ui.input.remember(&text);
                            ui.input.clear();
                            ui.scroll = 0;
                            // Своё сообщение не дорисовываем: оно приедет из
                            // транскрипта, и подделка рядом с настоящей строкой
                            // выглядела бы двойной отправкой.
                            ui.status =
                                format!("→ {}", truncate(&text, (caps.width as usize).min(60)));
                        }
                        Err(e) => ui.status = e,
                    }
                }
            }
        }
        Act::Type(c) => ui.input.insert(c),
        Act::Paste(text) => {
            // Вставка целиком: перевод строки внутри неё — текст, а не Enter.
            // Иначе многострочная вставка отправляла бы первую строку.
            ui.input.paste(&text);
            if ui.typing == Typing::None {
                ui.typing = idle_typing(ui);
            }
        }
        Act::WordLeft => ui.input.word_left(),
        Act::WordRight => ui.input.word_right(),
        Act::KillWordLeft => ui.input.kill_word_left(),
        Act::KillWordRight => ui.input.kill_word_right(),
        Act::KillToEnd => ui.input.kill_to_end(),
        Act::Yank => {
            if !ui.input.yank() {
                ui.status = "нечего возвращать: кольцо пусто".into();
            }
        }
        Act::YankPop => {
            if !ui.input.yank_pop() {
                ui.status = "в кольце одно значение".into();
            }
        }
        Act::Undo => {
            if !ui.input.undo() {
                ui.status = "отменять нечего".into();
            }
        }
        Act::Left => ui.input.left(),
        Act::Right => ui.input.right(),
        Act::Home => ui.input.home(),
        Act::End => ui.input.end(),
        Act::Delete => ui.input.delete(),
        Act::Newline => ui.input.newline(),
        Act::Tab => {
            // Дополняем только команду: в тексте сообщения Tab — это Tab.
            let line = ui.input.text().to_string();
            if !line.starts_with('/') {
                return true;
            }
            let head = line.split_whitespace().next().unwrap_or("/").to_string();
            let rest = line[head.len()..].to_string();
            if rest.trim().is_empty() && !line.ends_with(' ') {
                // Дополняем имя команды.
                if let Some(full) = slash::complete(&head) {
                    ui.input.set(format!("{full}{}", rest.trim_start()));
                }
                return true;
            }
            // Дальше — аргумент: подставляем из того, что человек видит.
            let name = head.trim_start_matches('/').to_lowercase();
            let part = rest.trim_start().to_string();
            let cands = arg_candidates(ui, list, &name, &part);
            if let Some(done) = slash::complete_arg(&part, &cands) {
                ui.input.set(format!("{head} {done}"));
            } else if cands.is_empty() {
                ui.status = "дополнять нечем".into();
            }
        }
        Act::Slash => {
            ui.typing = Typing::Command;
            ui.input.set("/");
            ui.status.clear();
        }
        Act::Backspace => {
            ui.input.backspace();
            // Стёрли слэш — команды больше нет, и палитра уходит сама.
            if ui.typing == Typing::Command && ui.input.text().is_empty() {
                ui.typing = idle_typing(ui);
            }
        }
        Act::KillLine => ui.input.kill_line(),
        Act::Merge => {
            if ui.view != View::Bundle {
                return true;
            }
            let Some(b) = current_bundle(ui).cloned() else {
                return true;
            };
            if ui.busy.is_some() {
                ui.status = "погоди, прошлое действие ещё идёт".into();
                return true;
            }
            let Some(head) = queue_head(&b) else {
                ui.status = "очередь пуста — руки встанут в неё сами".into();
                return true;
            };
            // Долгое (git, ssh, гейты) уходит в фон: окно обязано слушаться
            // клавиш всё это время, иначе оно выглядит зависшим.
            ui.busy = Some((format!("вливаю {head}"), now_ms()));
            ui.status = format!("вливаю {head}…");
            let (tx, id) = (tx.clone(), b.id.clone());
            tokio::spawn(async move {
                let res = match load_bundle(&id) {
                    Some((all, i)) => crate::engine::bundle::merge(all, i, &head).await,
                    None => Err("связка пропала из файла".into()),
                };
                let _ = tx.send(Msg::Done(res)).await;
            });
        }
        Act::Pause => {
            if ui.view != View::Bundle {
                return true;
            }
            let Some(b) = current_bundle(ui).cloned() else {
                return true;
            };
            let on = !b.paused;
            match toggle_pause(&b.id, on) {
                Ok(_) => {
                    ui.disk_at = 0;
                    ui.status = if on {
                        "пауза: новые руки не поднимаются, слияния стоят".into()
                    } else {
                        "связка продолжает".to_string()
                    };
                }
                Err(e) => ui.status = e,
            }
        }
        Act::Remove if matches!(ui.view, View::Bundles | View::Bundle) => {
            let b = if ui.view == View::Bundle {
                current_bundle(ui).cloned()
            } else {
                ui.bundles.get(ui.bsel).cloned()
            };
            let Some(b) = b else {
                ui.status = "нечего убирать".into();
                return true;
            };
            if ui.confirm_rm.as_deref() != Some(b.id.as_str()) {
                let live = crate::engine::bundle::alive(&b).len();
                ui.confirm_rm = Some(b.id.clone());
                ui.status = if live > 0 {
                    format!(
                        "«{}»: {} ещё в работе. Убрать вместе с ними? d — да, esc — нет",
                        b.name,
                        crate::core::util::plural(live as u64, "рука", "руки", "рук")
                    )
                } else {
                    format!("убрать «{}»? d — да, esc — нет", b.name)
                };
                return true;
            }
            ui.confirm_rm = None;
            // Worktree и ветки не трогаем: в окне у человека нет ни флага
            // `--clean`, ни возможности прочитать отказ git по каждой руке.
            let all = state::load_bundles();
            let Some(i) = all.iter().position(|x| x.id == b.id) else {
                ui.status = "связка уже убрана".into();
                return true;
            };
            match crate::engine::bundle::remove(all, i, true, false).await {
                Ok(report) => {
                    ui.bundles = state::load_bundles();
                    ui.bsel = ui.bsel.min(ui.bundles.len().saturating_sub(1));
                    ui.open_bundle.clear();
                    ui.view = View::Bundles;
                    ui.status = report.join(" · ");
                }
                Err(e) => ui.status = e,
            }
        }
        Act::Remove if matches!(ui.view, View::Bundles | View::Bundle) => {
            let b = if ui.view == View::Bundle {
                current_bundle(ui).cloned()
            } else {
                ui.bundles.get(ui.bsel).cloned()
            };
            let Some(b) = b else {
                ui.status = "нечего убирать".into();
                return true;
            };
            if ui.confirm_rm.as_deref() != Some(b.id.as_str()) {
                // Первое нажатие спрашивает, второе делает: промах по клавише
                // не должен стирать связку, а модальное окно ради одного
                // вопроса — лишнее.
                let live = crate::engine::bundle::alive(&b).len();
                ui.confirm_rm = Some(b.id.clone());
                ui.status = if live > 0 {
                    format!(
                        "«{}»: {} ещё в работе. Убрать вместе с ними? d — да, esc — нет",
                        b.name,
                        crate::core::util::plural(live as u64, "рука", "руки", "рук")
                    )
                } else {
                    format!("убрать «{}»? d — да, esc — нет", b.name)
                };
                return true;
            }
            ui.confirm_rm = None;
            // Worktree и ветки не трогаем: в окне нет ни флага `--clean`, ни
            // места прочитать отказ git по каждой руке.
            let all = state::load_bundles();
            let Some(i) = all.iter().position(|x| x.id == b.id) else {
                ui.status = "связка уже убрана".into();
                return true;
            };
            match crate::engine::bundle::remove(all, i, true, false).await {
                Ok(report) => {
                    ui.bundles = state::load_bundles();
                    ui.bsel = ui.bsel.min(ui.bundles.len().saturating_sub(1));
                    ui.open_bundle.clear();
                    ui.view = View::Bundles;
                    ui.status = report.join(" · ");
                }
                Err(e) => ui.status = e,
            }
        }
        Act::Remove => {
            ui.status = "убирать можно связку — в её списке (b)".into();
        }
        Act::NewHand if ui.view == View::Bundles => start_form(ui, Kind::Bundle).await,
        Act::NewHand if ui.view == View::Loops => start_form(ui, Kind::Loop).await,
        Act::NewHand => {
            if ui.view != View::Bundle {
                ui.status = "новая связка — в списке связок (b), новая рука — в пульте".into();
                return true;
            }
            if ui.busy.is_some() {
                ui.status = "погоди, прошлое действие ещё идёт".into();
                return true;
            }
            // Рука без задачи бессмысленна, поэтому сначала спрашиваем её.
            ui.typing = Typing::Task;
            ui.input.clear();
            ui.status = "чем займётся рука? Enter — поднять, Esc — отмена".into();
        }
        Act::Loops => ui.view = View::Loops,
        Act::Bundles => ui.view = View::Bundles,
        Act::Help => {
            if ui.view == View::Help {
                ui.view = ui.prev.clone();
            } else {
                ui.prev = ui.view.clone();
                ui.view = View::Help;
            }
        }
        Act::None => {}
    }
    true
}

/// Та же клавиша, но вызванная командой. Через `Box::pin`, потому что команда
/// умеет делать то же, что клавиша, а клавиша — вызывать команду: без бокса
/// такое будущее не имеет конечного размера.
async fn as_key(
    ui: &mut Ui,
    act: Act,
    client: &NodeClient,
    list: &[Session],
    caps: &Caps,
    tx: &tokio::sync::mpsc::Sender<Msg>,
) -> bool {
    Box::pin(handle(ui, act, client, list, caps, tx)).await
}

/// Выполнить команду окна. `false` — человек вышел.
///
/// Неизвестное имя — не ошибка: у агента свои слэш-команды, и человек,
/// набравший `/compact`, хочет попасть к нему, а не получить нотацию.
async fn run_slash(
    ui: &mut Ui,
    name: &str,
    rest: &str,
    client: &NodeClient,
    list: &[Session],
    caps: &Caps,
    tx: &tokio::sync::mpsc::Sender<Msg>,
) -> bool {
    // Кому адресована команда: открытому чату, а если его нет — выбранному в
    // списке. Иначе `/stop` из списка молчал бы, хотя сессия перед глазами.
    let target = list
        .iter()
        .find(|s| s.id == ui.open_id)
        .or_else(|| list.get(ui.sel))
        .cloned();
    match name {
        "quit" | "q" | "exit" => return false,
        "help" => {
            ui.prev = ui.view.clone();
            ui.view = View::Help;
        }
        "list" | "ls" => {
            ui.view = View::List;
            ui.feed = None;
            ui.items.clear();
        }
        "loops" | "loop" => ui.view = View::Loops,
        "bundles" | "bundle" => ui.view = View::Bundles,
        "new" => {
            // `/new` человек наберёт там, где вспомнил, поэтому уточнить можно
            // словом, а без слова — по тому виду, где он стоит.
            let kind = match rest.trim().to_lowercase().as_str() {
                "loop" | "цикл" => Kind::Loop,
                "bundle" | "связка" | "связку" => Kind::Bundle,
                _ if ui.view == View::Loops => Kind::Loop,
                _ => Kind::Bundle,
            };
            ui.view = if kind == Kind::Loop {
                View::Loops
            } else {
                View::Bundles
            };
            start_form(ui, kind).await;
        }
        "limits" => {
            ui.limits_at = 0; // следующий круг перечитает
            ui.status = "обновляю лимиты…".into();
        }
        "chat" => {
            let needle = if rest.is_empty() {
                target.as_ref().map(|s| s.id.clone()).unwrap_or_default()
            } else {
                rest.to_string()
            };
            match crate::app::resolve(list, &needle) {
                Ok(s) => {
                    let (id, title, path) = (s.id.clone(), s.title(), s.transcript.clone());
                    match path.filter(|p| !p.is_empty()) {
                        Some(path) => match Feed::open(client, &path).await {
                            Ok(f) => {
                                ui.feed = Some(f);
                                ui.items.clear();
                                ui.scroll = 0;
                                ui.feed_at = 0;
                                ui.open_id = id;
                                ui.open_title = title;
                                ui.view = View::Chat;
                                ui.typing = Typing::Message;
                                ui.status.clear();
                            }
                            Err(e) => ui.status = e,
                        },
                        None => ui.status = format!("{title}: транскрипта ещё нет"),
                    }
                }
                Err(e) => ui.status = e,
            }
        }
        "screen" => return as_key(ui, Act::Screen, client, list, caps, tx).await,
        "stop" => return as_key(ui, Act::Interrupt, client, list, caps, tx).await,
        "merge" => return as_key(ui, Act::Merge, client, list, caps, tx).await,
        "pause" => return as_key(ui, Act::Pause, client, list, caps, tx).await,
        "rm" => return as_key(ui, Act::Remove, client, list, caps, tx).await,
        "hand" => {
            if ui.view != View::Bundle {
                ui.status = "рука заводится в пульте связки: /bundles, потом Enter".into();
                return true;
            }
            if rest.trim().is_empty() {
                return as_key(ui, Act::NewHand, client, list, caps, tx).await;
            }
            ui.typing = Typing::Task;
            ui.input.set(rest);
            return as_key(ui, Act::Send, client, list, caps, tx).await;
        }
        "run" => {
            if rest.trim().is_empty() {
                ui.status = "где запускать? /run <каталог>".into();
                return true;
            }
            let (dir, client2, tx2) = (rest.to_string(), client.clone(), tx.clone());
            ui.busy = Some(("поднимаю агента".to_string(), now_ms()));
            ui.status = format!("поднимаю агента в {dir}…");
            tokio::spawn(async move {
                let name = dir
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or("проект");
                let res = client2
                    .launch(&dir, "claude --dangerously-skip-permissions", name)
                    .await
                    .map(|pane| vec![format!("агент поднят, пана {pane}")]);
                let _ = tx2.send(Msg::Done(res)).await;
            });
        }
        // Всё остальное — слэш-команда самого агента: /model, /effort,
        // /compact, /clear. Отправляем как есть той сессии, что перед глазами.
        _ => {
            let Some(s) = target else {
                ui.status = format!("некому передать /{name}");
                return true;
            };
            let Some(pane) = s.pane.clone().filter(|p| !p.is_empty()) else {
                ui.status = format!("{} не в tmux — команду не передать", s.title());
                return true;
            };
            let cmd = if rest.is_empty() {
                format!("/{name}")
            } else {
                format!("/{name} {rest}")
            };
            ui.status = match client.control(&pane, &cmd).await {
                Ok(_) => format!("{} → {cmd}", s.title()),
                Err(e) => e,
            };
        }
    }
    true
}

/// Подтянуть то, что нужно текущему виду, и не чаще, чем нужно.
async fn refresh(ui: &mut Ui, client: &NodeClient) {
    let now = now_ms();
    if ui.view == View::Chat && now - ui.feed_at > FEED_EVERY {
        ui.feed_at = now;
        if let Some(feed) = ui.feed.as_mut() {
            match feed.poll(client).await {
                Ok(items) if !items.is_empty() => {
                    ui.items.extend(items);
                    let extra = ui.items.len().saturating_sub(MAX_ITEMS);
                    if extra > 0 {
                        ui.items.drain(..extra);
                    }
                }
                Ok(_) => {}
                Err(e) => ui.status = e,
            }
        }
    }
    if ui.view == View::Screen && now - ui.screen_at > SCREEN_EVERY {
        ui.screen_at = now;
        if !ui.open_pane.is_empty() {
            if let Ok(s) = client.screen(&ui.open_pane).await {
                ui.screen = s;
            }
        }
    }
    // Циклы и связки лежат в файлах, общих с панелью: перечитываем раз в
    // секунду, а не на каждый кадр — кадров двенадцать в секунду.
    if now - ui.disk_at > DISK_EVERY {
        ui.disk_at = now;
        ui.bundles = state::load_bundles();
        ui.loops = state::load_loops();
        ui.bsel = ui.bsel.min(ui.bundles.len().saturating_sub(1));
        if let Some(b) = ui.bundles.iter().find(|b| b.id == ui.open_bundle) {
            ui.hsel = ui.hsel.min(b.hands.len().saturating_sub(1));
        }
    }
    if now - ui.limits_at > LIMITS_EVERY {
        ui.limits_at = now;
        if let Ok(text) = client.usage_text(false).await {
            ui.limits = crate::app::parse_usage(&text);
        }
    }
}

/* ---------- отрисовка ---------- */

fn size() -> (u16, u16) {
    match crossterm::terminal::size() {
        Ok((0, _)) | Err(_) => (80, 24),
        Ok((w, h)) => (w.max(20), h.max(8)),
    }
}

/// Показать кадр. Здесь и только здесь решается, мигает окно или нет.
///
/// Мигание берётся из двух привычек, и обе выглядят безобидно:
///
/// 1. «Очистить экран и нарисовать заново» (`\x1b[H\x1b[J`). Между очисткой и
///    печатью терминал успевает показать пустоту — это и есть вспышка. Вместо
///    очистки затираем КАЖДУЮ строку по мере печати (`\x1b[K`), а `\x1b[J`
///    делаем один раз в конце: пустого кадра не существует ни мгновения.
/// 2. Рисовать на каждом обороте цикла. Клавиши опрашиваются двенадцать раз в
///    секунду, и столько же раз перерисовывался неизменившийся экран. Сверяем
///    кадр с предыдущим и молчим, если он тот же.
///
/// Последняя строка печатается БЕЗ перевода: кадр ростом в целый экран,
/// закончившийся переводом строки, прокручивает терминал на строку — и весь
/// экран дёргается вверх на каждом кадре.
fn present(frame: &str, last: &mut String) {
    let Some(out) = repaint(frame, last) else {
        return;
    };
    let mut so = std::io::stdout().lock();
    let _ = so.write_all(out.as_bytes());
    let _ = so.flush();
    last.clear();
    last.push_str(frame);
}

/// Что именно отправить в терминал ради нового кадра. `None` — отправлять
/// нечего: кадр тот же, и любая запись была бы чистой рябью.
fn repaint(frame: &str, last: &str) -> Option<String> {
    if frame == last {
        return None;
    }
    let mut out = String::with_capacity(frame.len() + 64);
    out.push_str("\x1b[H");
    for (i, line) in frame.split("\r\n").enumerate() {
        if i > 0 {
            out.push_str("\r\n");
        }
        out.push_str(line);
        out.push_str("\x1b[K");
    }
    out.push_str("\x1b[J");
    Some(out)
}

/// Собрать кадр целиком. Чистая функция: ни печати, ни управляющих
/// последовательностей очистки — только текст, который увидит человек.
fn frame(caps: &Caps, machine: &str, ui: &Ui, list: &[Session], rows: u16) -> String {
    let total = caps.width as usize;
    let mut out = String::new();

    // Шапка-полоса: слева — где мы, справа — сводка. Полосу видно боковым
    // зрением, и экран читается как приложение, а не как вывод команды.
    if let Some(f) = ui.form.as_ref() {
        push(&mut out, &header(caps, f.title(), "Esc — отменить", total));
        push(&mut out, "");
    }
    let (left, right) = match ui.view {
        View::List => (
            format!("Jarvis · {machine}"),
            crate::ui::style::strip(&render::tally_line(caps, &session::tally(list))),
        ),
        View::Chat => (format!("{} · чат", ui.open_title), machine.to_string()),
        View::Screen => (format!("{} · экран", ui.open_title), machine.to_string()),
        View::Loops => ("Циклы".into(), machine.to_string()),
        View::Loop => {
            let l = current_loop(ui);
            let name = l.map(|l| l.name.clone()).unwrap_or_default();
            let right = l
                .and_then(|l| state::load_run(&l.id))
                .map(|r| {
                    format!(
                        "{} · {}",
                        r.stop.word(),
                        crate::core::util::plural(
                            r.iterations.len() as u64,
                            "итерация",
                            "итерации",
                            "итераций"
                        )
                    )
                })
                .unwrap_or_else(|| "не запускался".into());
            (format!("цикл · {name}"), right)
        }
        View::Bundles => ("Связки".into(), machine.to_string()),
        View::Bundle => {
            let b = current_bundle(ui);
            let name = b.map(|b| b.name.clone()).unwrap_or_default();
            let right = match b {
                Some(b) if b.paused => "на паузе".to_string(),
                Some(b) => {
                    let q = b.queue().len();
                    if q > 0 {
                        format!("{q} в очереди")
                    } else {
                        crate::core::util::plural(b.hands.len() as u64, "рука", "руки", "рук")
                    }
                }
                None => String::new(),
            };
            (format!("связка · {name}"), right)
        }
        View::Help => ("Клавиши".into(), String::new()),
    };
    if ui.form.is_none() {
        push(&mut out, &header(caps, &left, &right, total));
        push(&mut out, "");
    }

    // Полосы снизу: воздух, состояние, ввод (в чате) и клавиши.
    let body =
        (rows as usize).saturating_sub(if ui.view == View::Chat || (ui.typing != Typing::None) {
            6
        } else {
            5
        });
    if ui.form.is_some() {
        draw_form(&mut out, caps, ui, body);
    } else {
        match ui.view {
            View::List => draw_list(&mut out, caps, ui, list, body),
            View::Chat => draw_chat(&mut out, caps, ui, body),
            View::Screen => {
                for line in ui.screen.lines().take(body) {
                    push(
                        &mut out,
                        &format!(" {}", truncate(line, total.saturating_sub(1))),
                    );
                }
            }
            View::Loops => draw_loops(&mut out, caps, ui, body),
            View::Loop => draw_loop(&mut out, caps, ui, body),
            View::Bundles => draw_bundles(&mut out, caps, ui, body),
            View::Bundle => draw_bundle(&mut out, caps, ui, body),
            View::Help => draw_help(&mut out, caps),
        }
    }

    push(&mut out, "");
    if ui.view == View::List && ui.busy.is_none() {
        push(
            &mut out,
            &format!(" {}", render::limits_line(caps, &ui.limits)),
        );
    } else if let Some((what, since)) = ui.busy.as_ref() {
        // Пока идёт чужая долгая работа, строка состояния отвечает сразу на два
        // вопроса: живое ли оно и сколько уже тянется.
        push(
            &mut out,
            &format!(
                " {} {} {}",
                crate::ui::style::spinner(caps, ui.tick),
                paint(caps, Role::Text, what),
                paint(
                    caps,
                    Role::Dim,
                    &crate::ui::style::elapsed(now_ms() - since)
                )
            ),
        );
    } else if !ui.status.is_empty() {
        push(
            &mut out,
            &format!(" {}", paint(caps, Role::Muted, &ui.status)),
        );
    } else {
        push(&mut out, "");
    }
    // Палитра команд — над строкой ввода, как только строка похожа на команду.
    // Аргумент набирают — показываем подходящие значения, а не список команд:
    // «какие вообще бывают» человек спрашивает один раз, а «что подставить
    // сюда» — каждый раз.
    let arg_hits = arg_palette(ui, list);
    for c in arg_hits.iter().take(6) {
        push(&mut out, &format!("  {}", paint(caps, Role::Muted, c)));
    }
    let hits = if arg_hits.is_empty() {
        palette(ui)
    } else {
        Vec::new()
    };
    let col = hits
        .iter()
        .map(|c| {
            1 + c.name.len()
                + if c.args.is_empty() {
                    0
                } else {
                    1 + width(c.args)
                }
        })
        .max()
        .unwrap_or(10)
        .clamp(10, 28);
    for c in hits.iter().take(6) {
        let head = if c.args.is_empty() {
            paint(caps, Role::Accent, &format!("/{}", c.name))
        } else {
            format!(
                "{} {}",
                paint(caps, Role::Accent, &format!("/{}", c.name)),
                paint(caps, Role::Dim, c.args)
            )
        };
        push(
            &mut out,
            &format!(" {}  {}", pad(&head, col), paint(caps, Role::Muted, c.what)),
        );
    }
    if typing_now(ui) {
        // Строка ввода — на подложке: видно, где кончается лента и начинается
        // то, что ты печатаешь. Каретка — обратным цветом, как в pi.
        let room = total.saturating_sub(6);
        let (row, col) = ui.input.row_col();
        for (i, line) in ui.input.lines().iter().enumerate() {
            let head = if i == 0 {
                paint(caps, Role::Accent, "›")
            } else {
                // Продолжение — с отступом, чтобы многострочное сообщение
                // читалось как одно, а не как три реплики.
                paint(caps, Role::Dim, "│")
            };
            let body = if i == row {
                with_caret(caps, line, col, room)
            } else {
                truncate(line, room)
            };
            push(
                &mut out,
                &band(caps, Bg::Sel, &format!("{head} {body}"), total),
            );
        }
    }
    push(&mut out, &format!(" {}", keys_hint(caps, ui)));
    // Хвостовой перевод строки убираем: он-то и прокручивает полный экран.
    while out.ends_with("\r\n") {
        out.truncate(out.len() - 2);
    }
    out
}

/// Строка с кареткой: символ под курсором — обратным цветом.
///
/// Рисуем свою каретку, а не двигаем настоящую: кадр печатается одним куском с
/// произвольного места, и позиция аппаратного курсора после него — вопрос
/// удачи. Нарисованная каретка всегда там, где текст.
fn with_caret(caps: &Caps, line: &str, col: usize, room: usize) -> String {
    let chars: Vec<char> = line.chars().collect();
    let before: String = chars.iter().take(col).collect();
    let at: String = chars
        .get(col)
        .copied()
        .map(String::from)
        .unwrap_or_else(|| " ".into());
    let after: String = chars.iter().skip(col + 1).collect();
    if !caps.color {
        // Без краски каретку показать нечем — покажем место словом-знаком.
        return truncate(&format!("{before}_{at}{after}"), room);
    }
    let shown = format!("{before}\x1b[7m{at}\x1b[27m{after}");
    // Обрезаем по видимой ширине: управляющие последовательности места не
    // занимают, и truncate это уже знает.
    truncate(&shown, room)
}

/// Строка в конце — единственная подсказка, которую человек читает. Поэтому в
/// ней ровно то, что работает ЗДЕСЬ, а не весь список возможностей: клавиша
/// выделена, объяснение приглушено, пары разделены точкой.
fn keys_hint(caps: &Caps, ui: &Ui) -> String {
    let pairs: Vec<(&str, &str)> = match ui.view {
        // Набор идёт поверх любого вида, поэтому его подсказки — первыми.
        _ if ui.typing == Typing::Field => vec![("↵", "дальше"), ("esc", "отменить")],
        _ if ui.typing == Typing::Command => {
            vec![("↵", "выполнить"), ("tab", "дополнить"), ("esc", "отмена")]
        }
        View::Bundle if ui.typing == Typing::Task => {
            vec![("↵", "поднять руку"), ("esc", "отмена")]
        }
        View::List => vec![
            ("↑↓", "выбор"),
            ("↵", "чат"),
            ("/", "команда"),
            ("s", "экран"),
            ("x", "прервать"),
            ("1-9", "ответ"),
            ("l", "циклы"),
            ("b", "связки"),
            ("?", "клавиши"),
            ("q", "выход"),
        ],
        View::Chat => vec![
            ("↵", "отправить"),
            ("alt+↵", "новая строка"),
            ("↑↓", "прошлые"),
            ("^w", "стереть слово"),
            ("^y", "вернуть"),
            ("^z", "отменить"),
            ("/", "команда"),
            ("esc", "назад"),
        ],
        View::Bundles => vec![
            ("↑↓", "выбор"),
            ("↵", "пульт"),
            ("n", "новая"),
            ("d", "убрать"),
            ("esc", "назад"),
        ],
        View::Bundle => vec![
            ("↑↓", "выбор"),
            ("↵", "чат руки"),
            ("m", "влить"),
            ("n", "рука"),
            ("p", "пауза"),
            ("d", "убрать"),
            ("esc", "назад"),
        ],
        View::Loops => vec![
            ("↑↓", "выбор"),
            ("↵", "журнал"),
            ("s", "запустить"),
            ("x", "остановить"),
            ("n", "новый"),
            ("esc", "назад"),
        ],
        View::Loop => vec![
            ("s", "запустить"),
            ("x", "остановить"),
            ("esc", "назад"),
            ("q", "выход"),
        ],
        View::Screen | View::Help => {
            vec![("esc", "назад"), ("q", "выход")]
        }
    };
    let sep = paint(caps, Role::Border, " · ");
    let room = (caps.width as usize).saturating_sub(2);
    // В узком окне выбрасываем ПАРЫ с конца, а не режем строку: обрезанная
    // строка оставляет висеть половину подсказки и открытую краску.
    let mut n = pairs.len();
    loop {
        let line = pairs[..n]
            .iter()
            .map(|(k, w)| key(caps, k, w))
            .collect::<Vec<_>>()
            .join(&sep);
        if n <= 1 || width(&line) <= room {
            return line;
        }
        n -= 1;
    }
}

fn draw_list(out: &mut String, caps: &Caps, ui: &Ui, list: &[Session], rows: usize) {
    if list.is_empty() {
        push(out, &paint(caps, Role::Muted, " Ни одной сессии."));
        return;
    }
    let total = caps.width as usize;
    // Строку строим на ширину без полей: выделенная ляжет на подложку, у
    // которой свои поля, и обе обязаны совпасть по краю.
    let inner = Caps {
        width: total.saturating_sub(2) as u16,
        ..*caps
    };
    let col = render::name_column(list);
    let from = ui.sel.saturating_sub(rows.saturating_sub(1));
    for (i, s) in list.iter().enumerate().skip(from).take(rows) {
        let row = render::session_row(&inner, s, col);
        if i == ui.sel {
            push(out, &band(caps, Bg::Sel, &row, total));
        } else {
            push(out, &format!(" {row}"));
        }
    }
}

fn draw_chat(out: &mut String, caps: &Caps, ui: &Ui, rows: usize) {
    if ui.items.is_empty() {
        push(
            out,
            &paint(caps, Role::Muted, " Пока пусто — ждём первых строк."),
        );
        return;
    }
    // Лента собирается блоками, а потом берётся её хвост: у блока переменная
    // высота, и считать видимое по числу записей — верный способ показать
    // половину сообщения.
    let total = caps.width as usize;
    let tail = ui.items.len().saturating_sub(60);
    let lines = chat::feed_lines(caps, &ui.items[tail..], total);
    for l in visible(&lines, rows, ui.scroll) {
        push(out, l);
    }
    if ui.scroll > 0 {
        push(
            out,
            &paint(caps, Role::Dim, &format!(" ↑ прокручено на {}", ui.scroll)),
        );
    }
}

fn draw_loops(out: &mut String, caps: &Caps, ui: &Ui, rows: usize) {
    let all = &ui.loops;
    if all.is_empty() {
        push(
            out,
            &paint(caps, Role::Muted, " Циклов пока нет. n — завести цикл."),
        );
        return;
    }
    let col = all
        .iter()
        .map(|l| width(&l.name))
        .max()
        .unwrap_or(10)
        .clamp(8, 24);
    let total = caps.width as usize;
    let inner = Caps {
        width: total.saturating_sub(2) as u16,
        ..*caps
    };
    for (i, l) in all.iter().enumerate().take(rows) {
        let run = state::load_run(&l.id);
        let row = render::loop_row(&inner, l, run.as_ref(), col);
        if i == ui.lsel {
            push(out, &band(caps, Bg::Sel, &row, total));
        } else {
            push(out, &format!(" {row}"));
        }
    }
}

/// Журнал цикла: итерации сверху вниз, свежие внизу — как в разговоре.
fn draw_loop(out: &mut String, caps: &Caps, ui: &Ui, rows: usize) {
    let Some(l) = current_loop(ui) else {
        push(out, &paint(caps, Role::Muted, " цикл пропал"));
        return;
    };
    push(
        out,
        &paint(
            caps,
            Role::Dim,
            &format!(
                " {}{} · {} · выход: {} подряд",
                if l.machine.is_empty() || l.machine == "local" {
                    String::new()
                } else {
                    format!("{}:", l.machine)
                },
                l.sandbox.repo,
                l.wake_label(),
                l.exit.streak
            ),
        ),
    );
    push(
        out,
        &paint(caps, Role::Muted, &format!(" цель: {}", l.source.goal)),
    );
    push(out, "");
    let Some(run) = state::load_run(&l.id) else {
        push(
            out,
            &paint(caps, Role::Muted, " Ещё не запускался. s — запустить."),
        );
        return;
    };
    if run.iterations.is_empty() {
        push(out, &paint(caps, Role::Muted, " Итераций пока нет."));
    }
    let inner = Caps {
        width: (caps.width as usize).saturating_sub(2) as u16,
        ..*caps
    };
    // Свежие итерации внизу: журнал читается как разговор, а не как архив.
    for it in visible(&run.iterations, rows.saturating_sub(5), 0) {
        push(out, &render::iteration_row(&inner, it));
    }
    if let Some(ask) = run.ask.as_ref() {
        push(out, "");
        push(
            out,
            &paint(caps, Role::Warn, &format!(" спрашивает: {}", ask.question)),
        );
        push(
            out,
            &paint(
                caps,
                Role::Dim,
                &format!(" ответить: jarvis loop say {} <текст>", l.name),
            ),
        );
    }
}

fn draw_bundles(out: &mut String, caps: &Caps, ui: &Ui, rows: usize) {
    if ui.bundles.is_empty() {
        push(
            out,
            &paint(caps, Role::Muted, " Связок пока нет. n — завести связку."),
        );
        return;
    }
    let total = caps.width as usize;
    for (i, b) in ui.bundles.iter().enumerate().take(rows) {
        let q = b.queue().len();
        let row = format!(
            "{}  {}  {}",
            pad(&truncate(&b.name, 18), 18),
            paint(
                caps,
                Role::Dim,
                &crate::core::util::plural(b.hands.len() as u64, "рука", "руки", "рук")
            ),
            if b.paused {
                paint(caps, Role::Warn, "на паузе")
            } else if q > 0 {
                paint(caps, Role::Accent, &format!("{q} в очереди"))
            } else {
                paint(caps, Role::Dim, "очередь пуста")
            }
        );
        if i == ui.bsel {
            push(out, &band(caps, Bg::Sel, &row, total));
        } else {
            push(out, &format!(" {row}"));
        }
    }
}

/// Форма связки: что уже отвечено, что спрашивают сейчас.
///
/// Отвеченное остаётся на экране: человек должен видеть, что он набрал, а не
/// помнить. На шаге гейтов показываем каталог заготовок — те же, что в
/// конструкторе команды, потому что команды проверок из головы не вводят.
fn draw_form(out: &mut String, caps: &Caps, ui: &Ui, rows: usize) {
    let Some(form) = ui.form.as_ref() else { return };
    let total = caps.width as usize;
    push(
        out,
        &paint(
            caps,
            Role::Muted,
            " Несколько вопросов. Enter — согласиться с тем, что в скобках.",
        ),
    );
    push(out, "");
    for (label, value) in form.filled() {
        push(
            out,
            &format!(
                " {}  {}",
                paint(caps, Role::Dim, &pad(label, 9)),
                paint(
                    caps,
                    Role::Text,
                    &truncate(&value, total.saturating_sub(12))
                )
            ),
        );
    }

    let (question, default) = form.question();
    push(out, "");
    push(
        out,
        &format!(
            " {} {}",
            paint(caps, Role::Accent, &format!("{question}?")),
            paint(caps, Role::Dim, &format!("[{}]", truncate(&default, 40)))
        ),
    );

    let slot = match form.step() {
        crate::ui::form::Step::Gates => Some(crate::engine::presets::Slot::Gate),
        crate::ui::form::Step::Source => Some(crate::engine::presets::Slot::Source),
        _ => None,
    };
    if let Some(slot) = slot {
        push(out, "");
        let cat = crate::engine::builder::catalog(slot);
        let col = cat
            .iter()
            .map(|p| width(p.name))
            .max()
            .unwrap_or(10)
            .clamp(8, 24);
        let mut group = "";
        // Влезет столько, сколько осталось места: остальное человек и так
        // наберёт номерами, а обрезанный список честнее вранья про «это всё».
        let room = rows.saturating_sub(form.filled().len() + 6).max(3);
        for (i, p) in cat.iter().enumerate().take(room) {
            if p.category != group {
                group = p.category;
                push(out, &format!("  {}", paint(caps, Role::Dim, group)));
            }
            push(
                out,
                &format!(
                    "  {}  {}  {}",
                    paint(caps, Role::Accent, &format!("{:>2}", i + 1)),
                    pad(&truncate(p.name, col), col),
                    paint(
                        caps,
                        Role::Muted,
                        &truncate(p.hint, total.saturating_sub(col + 12))
                    )
                ),
            );
        }
        if cat.len() > room {
            push(
                out,
                &paint(caps, Role::Dim, &format!("  … и ещё {}", cat.len() - room)),
            );
        }
    }
}

/// Пульт связки: руки сверху, события снизу.
///
/// События показываем прямо здесь, а не прячем в отдельный вид: слияние и
/// конфликт — это то, ради чего в пульт заходят, и узнавать о них человек
/// должен там же, где нажимает.
fn draw_bundle(out: &mut String, caps: &Caps, ui: &Ui, rows: usize) {
    let Some(b) = current_bundle(ui) else {
        push(out, &paint(caps, Role::Muted, " связка пропала"));
        return;
    };
    let total = caps.width as usize;
    push(
        out,
        &paint(
            caps,
            Role::Dim,
            &format!(
                " {} → {}{}",
                b.dir,
                if b.base.is_empty() { "main" } else { &b.base },
                if b.machine.is_empty() || b.machine == "local" {
                    String::new()
                } else {
                    format!(" · {}", b.machine)
                }
            ),
        ),
    );
    push(out, "");

    if b.hands.is_empty() {
        push(
            out,
            &paint(caps, Role::Muted, " Рук пока нет. Завести — клавиша n."),
        );
    } else {
        let col = b
            .hands
            .iter()
            .map(|h| width(&h.name))
            .max()
            .unwrap_or(10)
            .clamp(8, 24);
        let inner = Caps {
            width: total.saturating_sub(2) as u16,
            ..*caps
        };
        // Половину места отдаём рукам, остальное — событиям: и то и другое
        // должно быть видно сразу, иначе пульт превращается в два экрана.
        let room = rows.saturating_sub(4).max(1);
        let hands_room = room.saturating_sub(3).max(1).min(b.hands.len());
        let from = ui.hsel.saturating_sub(hands_room.saturating_sub(1));
        for (i, h) in b.hands.iter().enumerate().skip(from).take(hands_room) {
            let row = render::hand_row(&inner, b, &h.id, col);
            if i == ui.hsel {
                push(out, &band(caps, Bg::Sel, &row, total));
            } else {
                push(out, &format!(" {row}"));
            }
        }
    }

    if !b.events.is_empty() {
        push(out, "");
        push(out, &paint(caps, Role::Border, " события"));
        let room = rows.saturating_sub(6 + b.hands.len().min(rows)).max(1);
        for e in b.events.iter().rev().take(room).rev() {
            push(
                out,
                &paint(
                    caps,
                    Role::Dim,
                    &format!(
                        " {} {}",
                        crate::core::util::clock(e.at),
                        truncate(&e.text, total.saturating_sub(9))
                    ),
                ),
            );
        }
    }
}

fn draw_help(out: &mut String, caps: &Caps) {
    for (k, what) in [
        ("↑ ↓ / k j", "выбрать сессию"),
        ("Enter", "открыть чат; в чате — отправить набранное"),
        ("s", "экран сессии как есть"),
        ("x", "прервать агента (Escape ему в пану)"),
        ("1…9", "ответить вариантом на вопрос с меню"),
        ("l / b", "циклы / связки"),
        ("Esc", "назад; в чате с набранным — стереть набранное"),
        ("Ctrl-U", "стереть строку ввода"),
        ("q / Ctrl-C", "выход"),
    ] {
        push(
            out,
            &format!("  {}  {}", pad(k, 12), paint(caps, Role::Dim, what)),
        );
    }
}

/// В сыром режиме перевод строки не возвращает каретку сам — пишем оба знака.
fn push(out: &mut String, line: &str) {
    out.push_str(line);
    out.push_str("\r\n");
}

/// Сырой режим с гарантированным восстановлением: брошенный терминал без эха —
/// худшее, что программа может оставить после себя.
struct RawGuard(bool);

impl RawGuard {
    fn enable() -> Self {
        // Курсор прячем на всё время окна: рисуя кадр, он пробегает по экрану
        // и оставляет за собой мерцающий след — самая заметная часть «ряби».
        print!("\x1b[?25l");
        // Скобочная вставка: терминал обрамляет вставленный кусок метками, и
        // мы получаем его ОДНИМ событием. Без неё вставка приезжает как град
        // нажатий, и первый же перевод строки уходит отправкой недописанного.
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste);
        let _ = std::io::stdout().flush();
        Self(crossterm::terminal::enable_raw_mode().is_ok())
    }
}

impl Drop for RawGuard {
    fn drop(&mut self) {
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableBracketedPaste);
        if self.0 {
            let _ = crossterm::terminal::disable_raw_mode();
        }
        // Курсор вернуть обязаны, даже если дальше паника: терминал без
        // курсора человек чинит вслепую, командой reset.
        print!("\x1b[?25h");
        let _ = std::io::stdout().flush();
        println!();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame_caps() -> Caps {
        Caps {
            color: false,
            truecolor: false,
            unicode: true,
            width: 80,
        }
    }

    fn sessions(n: usize) -> Vec<Session> {
        (0..n)
            .map(|i| Session {
                id: format!("s{i}"),
                project: Some(format!("проект-{i}")),
                detail: "выполняет Bash".into(),
                ..Default::default()
            })
            .collect()
    }

    /// Кадр — это текст, а не команды терминалу. Очистка внутри кадра означала
    /// бы, что между нею и печатью экран пуст: ровно это и мигает.
    #[test]
    fn frame_carries_no_screen_clearing() {
        let f = frame(&frame_caps(), "local", &Ui::default(), &sessions(3), 24);
        assert!(!f.contains("\x1b[J"), "очистка внутри кадра");
        assert!(!f.contains("\x1b[H"), "перевод курсора внутри кадра");
    }

    /// Кадр ростом в целый экран, оканчивающийся переводом строки, прокручивает
    /// терминал — и весь экран прыгает на каждом обновлении.
    #[test]
    fn frame_does_not_end_with_a_line_break() {
        for rows in [8u16, 24, 60] {
            let f = frame(&frame_caps(), "local", &Ui::default(), &sessions(40), rows);
            assert!(!f.ends_with("\r\n"), "{rows}: хвостовой перевод строки");
            assert!(
                f.split("\r\n").count() <= rows as usize,
                "{rows}: кадр выше экрана — нижние строки уедут"
            );
        }
    }

    #[test]
    fn unchanged_frame_is_not_repainted() {
        let f = frame(&frame_caps(), "local", &Ui::default(), &sessions(2), 24);
        assert!(
            repaint(&f, &f).is_none(),
            "перерисовка того же кадра — это и есть мерцание"
        );
        let other = frame(&frame_caps(), "vps", &Ui::default(), &sessions(2), 24);
        let out = repaint(&other, &f).expect("кадр изменился — рисуем");
        assert!(out.starts_with("\x1b[H"));
        assert!(out.ends_with("\x1b[J"), "хвост прошлого кадра надо стереть");
        assert!(
            !out.contains("\x1b[2J"),
            "полная очистка вернула бы вспышку"
        );
        // Каждая строка затирается по мере печати — иначе от длинной строки
        // прошлого кадра остаётся хвост.
        assert_eq!(out.matches("\x1b[K").count(), other.split("\r\n").count());
    }

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn ui_in(view: View, typing: Typing) -> Ui {
        Ui {
            view,
            typing,
            ..Default::default()
        }
    }

    async fn press(ui: &mut Ui, act: Act) -> bool {
        let (tx, _rx) = tokio::sync::mpsc::channel(4);
        let client = NodeClient::unix("/nonexistent.sock");
        handle(ui, act, &client, &[], &Caps::default(), &tx).await
    }

    /// Ctrl+D — это конец ввода, а не «удалить символ»: терминал шлёт его сам,
    /// когда закрывается стдин, и привязка к правке стирала букву без единого
    /// нажатия человека.
    #[test]
    fn ctrl_d_does_not_edit_the_line() {
        let cd = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL);
        assert_eq!(map_key(true, cd), Act::None);
        // Delete на своём месте работает как обычно.
        assert_eq!(
            map_key(true, KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE)),
            Act::Delete
        );
    }

    /// Каретка рисуется НА символе под курсором, ничего не съедая: ошибка на
    /// единицу здесь означает пропавшую букву прямо под пальцами.
    #[test]
    fn caret_marks_the_character_without_eating_it() {
        let c = Caps {
            color: true,
            truecolor: true,
            unicode: true,
            width: 40,
        };
        let mut e = Editor::default();
        e.set("раз два");
        for _ in 0..3 {
            e.left();
        }
        e.insert('!');
        assert_eq!(e.text(), "раз !два");
        let (_, col) = e.row_col();
        let line = with_caret(&c, e.text(), col, 40);
        let plain = crate::ui::style::strip(&line);
        assert_eq!(plain, "раз !два", "каретка съела букву: {plain:?}");
        // Обратным цветом помечен ровно один символ — тот, что под курсором.
        assert!(line.contains("\x1b[7mд\x1b[27m"), "{line:?}");
    }

    /// Ход цикла виден в строке состояния: без этого запуск из окна выглядит
    /// как «нажал и ничего не произошло».
    #[test]
    fn loop_progress_reads_like_a_sentence() {
        use crate::engine::loops::Note;
        assert_eq!(loop_line(Note::Started(3)), "итерация 3 — пошла");
        let it = state::Iteration {
            n: 2,
            verdict: state::Verdict::GateFailed,
            ..Default::default()
        };
        assert_eq!(loop_line(Note::Done(it)), "итерация 2 — красный гейт");
        let done = loop_line(Note::Finished {
            reason: state::StopReason::Exit,
            iterations: 1,
            tokens: 90_000,
            pending: 0,
        });
        assert!(
            done.contains("1 итерация"),
            "числительное не согласовано: {done}"
        );
        assert!(done.contains("90k"));
    }

    /// Связка должна заводиться НЕ выходя из окна: раньше пустой список
    /// советовал команду снаружи — это и значило «в окне нельзя».
    #[tokio::test]
    async fn n_in_the_bundle_list_starts_the_form() {
        let mut ui = ui_in(View::Bundles, Typing::None);
        press(&mut ui, Act::NewHand).await;
        assert!(ui.form.is_some(), "форма не открылась");
        assert_eq!(ui.typing, Typing::Field);

        // Ответы идут по одному: каталог, имя, база — и только потом гейты.
        for _ in 0..3 {
            press(&mut ui, Act::Send).await;
        }
        assert_eq!(
            ui.form.as_ref().map(|f| f.step()),
            Some(crate::ui::form::Step::Gates)
        );

        // Esc отменяет всё разом и не оставляет полуформы.
        press(&mut ui, Act::Escape).await;
        assert!(ui.form.is_none() && ui.typing == Typing::None);
    }

    /// То же самое командой — из любого вида.
    #[tokio::test]
    async fn slash_new_starts_the_form_too() {
        let mut ui = ui_in(View::Chat, Typing::Message);
        let (tx, _rx) = tokio::sync::mpsc::channel(4);
        let client = NodeClient::unix("/nonexistent.sock");
        run_slash(&mut ui, "new", "", &client, &[], &Caps::default(), &tx).await;
        assert!(ui.form.is_some());
        assert_eq!(ui.view, View::Bundles);
    }

    /// Слэш открывает командную строку из любого вида — команды `jarvis`
    /// человек уже знает, и заново учить клавиши окна незачем.
    #[tokio::test]
    async fn slash_opens_the_command_line() {
        let mut ui = ui_in(View::List, Typing::None);
        assert_eq!(map_key(false, key('/')), Act::Slash);
        press(&mut ui, Act::Slash).await;
        assert_eq!(ui.typing, Typing::Command);
        assert_eq!(ui.input.text(), "/");
        assert_eq!(
            palette(&ui).len(),
            slash::all().len(),
            "палитра показывает всё"
        );
    }

    /// В чате «/» — обычный знак: путь `/etc/hosts` пишут агенту постоянно.
    #[test]
    fn slash_is_a_character_while_typing() {
        assert_eq!(map_key(true, key('/')), Act::Type('/'));
    }

    #[tokio::test]
    async fn tab_completes_commands_and_leaves_text_alone() {
        let mut ui = ui_in(View::List, Typing::Command);
        ui.input.set("/lim");
        press(&mut ui, Act::Tab).await;
        assert_eq!(ui.input.text(), "/limits");

        let mut ui = ui_in(View::Chat, Typing::Message);
        ui.input.set("просто текст");
        press(&mut ui, Act::Tab).await;
        assert_eq!(ui.input.text(), "просто текст", "Tab в сообщении — это Tab");
    }

    /// Палитра уходит, как только команда набрана и пошли аргументы: подсказка
    /// поверх собственного ответа только мешает.
    #[test]
    fn palette_disappears_once_arguments_start() {
        let mut ui = ui_in(View::Chat, Typing::Command);
        ui.input.set("/li");
        let names: Vec<&str> = palette(&ui).iter().map(|c| c.name).collect();
        assert!(
            names.contains(&"limits") && names.contains(&"list"),
            "{names:?}"
        );
        ui.input.set("/chat lct");
        assert!(palette(&ui).is_empty());
        ui.input.set("//etc/hosts");
        assert!(palette(&ui).is_empty(), "экранированный слэш — не команда");
    }

    /// Esc из команды возвращает в чат, а не выбрасывает из него.
    #[tokio::test]
    async fn escape_from_a_command_keeps_the_chat_open() {
        let mut ui = ui_in(View::Chat, Typing::Command);
        ui.input.set("/lim");
        press(&mut ui, Act::Escape).await;
        assert_eq!(ui.view, View::Chat);
        assert_eq!(ui.typing, Typing::Message, "строка ввода осталась наготове");
        assert!(ui.input.text().is_empty());
    }

    /// В многострочном сообщении стрелки принадлежат курсору: иначе вторую
    /// строку не поправить, не стерев её целиком.
    #[tokio::test]
    async fn arrows_move_the_cursor_in_a_multiline_message() {
        let mut ui = ui_in(View::Chat, Typing::Message);
        ui.input.set("раз\nдва");
        assert_eq!(ui.input.row_col().0, 1);
        press(&mut ui, Act::Up).await;
        assert_eq!(ui.input.row_col().0, 0, "курсор не поднялся");
        assert_eq!(ui.scroll, 0, "вместо курсора прокрутилась лента");
    }

    /// Незнакомая команда — это команда агента (/compact, /clear): её нужно
    /// передать ему, а не отчитать человека.
    #[tokio::test]
    async fn unknown_command_is_addressed_to_the_agent() {
        let mut ui = ui_in(View::Chat, Typing::Message);
        let (tx, _rx) = tokio::sync::mpsc::channel(4);
        let client = NodeClient::unix("/nonexistent.sock");
        run_slash(&mut ui, "compact", "", &client, &[], &Caps::default(), &tx).await;
        assert!(
            ui.status.contains("/compact"),
            "команду не адресовали агенту: {}",
            ui.status
        );
    }

    /// Аргумент дополняется из того, что человек видит: живые сессии, а не
    /// придуманные имена.
    #[tokio::test]
    async fn tab_completes_an_argument_from_live_sessions() {
        let mut ui = ui_in(View::List, Typing::Command);
        ui.input.set("/chat ja");
        let sessions = vec![
            Session {
                id: "s1".into(),
                project: Some("jarvis".into()),
                ..Default::default()
            },
            Session {
                id: "s2".into(),
                project: Some("lct".into()),
                ..Default::default()
            },
        ];
        let (tx, _rx) = tokio::sync::mpsc::channel(4);
        let client = NodeClient::unix("/nonexistent.sock");
        handle(&mut ui, Act::Tab, &client, &sessions, &Caps::default(), &tx).await;
        assert_eq!(ui.input.text(), "/chat jarvis");
    }

    /// Слова аргумента у команд с готовым набором.
    #[tokio::test]
    async fn tab_completes_a_word_argument() {
        let mut ui = ui_in(View::Chat, Typing::Command);
        ui.input.set("/model op");
        let (tx, _rx) = tokio::sync::mpsc::channel(4);
        let client = NodeClient::unix("/nonexistent.sock");
        handle(&mut ui, Act::Tab, &client, &[], &Caps::default(), &tx).await;
        assert_eq!(ui.input.text(), "/model opus");
    }

    /// Пока набирают аргумент, палитра показывает значения, а не список команд:
    /// «какие бывают» спрашивают один раз, «что подставить сюда» — каждый раз.
    #[test]
    fn the_palette_switches_to_argument_values() {
        let mut ui = ui_in(View::List, Typing::Command);
        let sessions = vec![Session {
            id: "s1".into(),
            project: Some("jarvis".into()),
            ..Default::default()
        }];
        ui.input.set("/chat ");
        assert_eq!(arg_palette(&ui, &sessions), vec!["jarvis".to_string()]);
        ui.input.set("/chat");
        assert!(
            arg_palette(&ui, &sessions).is_empty(),
            "имя команды — ещё не аргумент"
        );
    }

    /// Клавиши пульта не должны отбирать буквы у чата: там «m» — это буква.
    #[test]
    fn bundle_keys_are_letters_while_typing() {
        assert_eq!(map_key(false, key('m')), Act::Merge);
        assert_eq!(map_key(false, key('n')), Act::NewHand);
        assert_eq!(map_key(false, key('p')), Act::Pause);
        assert_eq!(map_key(true, key('m')), Act::Type('m'));
        assert_eq!(map_key(true, key('n')), Act::Type('n'));
    }

    fn bundle_ui() -> Ui {
        let b = state::Bundle {
            id: "b1".into(),
            name: "api".into(),
            hands: vec![
                state::Hand {
                    id: "h1".into(),
                    name: "очередь".into(),
                    state: state::HandState::Ready,
                    ready_at: 200,
                    gates_ok: true,
                    ..Default::default()
                },
                state::Hand {
                    id: "h2".into(),
                    name: "доки".into(),
                    state: state::HandState::Ready,
                    ready_at: 100,
                    gates_ok: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        Ui {
            view: View::Bundle,
            open_bundle: "b1".into(),
            bundles: vec![b],
            ..Default::default()
        }
    }

    /// Вливается голова очереди — та, что встала в неё первой, а не выбранная
    /// строка: порядок задаёт готовность, и это единственное честное правило.
    #[test]
    fn head_of_the_queue_is_the_earliest_ready() {
        let ui = bundle_ui();
        assert_eq!(
            queue_head(current_bundle(&ui).unwrap()).as_deref(),
            Some("доки")
        );
    }

    /// Второе «влить» подряд, пока идёт первое, — это два слияния в очередь,
    /// которая такого не прощает.
    #[tokio::test]
    async fn merge_while_busy_is_refused() {
        let mut ui = bundle_ui();
        ui.busy = Some(("вливаю доки".to_string(), now_ms()));
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        let client = NodeClient::unix("/nonexistent.sock");
        let caps = Caps::default();
        assert!(handle(&mut ui, Act::Merge, &client, &[], &caps, &tx).await);
        assert!(ui.status.contains("погоди"), "{}", ui.status);
        assert!(rx.try_recv().is_err(), "второе слияние всё-таки ушло");
    }

    /// Рука заводится с задачей: сначала спрашиваем её, Esc отменяет набор.
    #[tokio::test]
    async fn new_hand_asks_for_the_task_first() {
        let mut ui = bundle_ui();
        let (tx, _rx) = tokio::sync::mpsc::channel(4);
        let client = NodeClient::unix("/nonexistent.sock");
        let caps = Caps::default();
        handle(&mut ui, Act::NewHand, &client, &[], &caps, &tx).await;
        assert!((ui.typing != Typing::None), "не спросил задачу");
        handle(&mut ui, Act::Type('д'), &client, &[], &caps, &tx).await;
        assert_eq!(ui.input.text(), "д");
        handle(&mut ui, Act::Escape, &client, &[], &caps, &tx).await;
        assert!(
            !(ui.typing != Typing::None) && ui.input.is_empty(),
            "Esc не отменил набор"
        );
        assert_eq!(ui.view, View::Bundle, "Esc из набора вышвырнул из пульта");
    }

    /// Пустая задача руку не поднимает: это молчаливая сессия без цели.
    #[tokio::test]
    async fn empty_task_does_not_launch_a_hand() {
        let mut ui = bundle_ui();
        ui.typing = Typing::Task;
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        let client = NodeClient::unix("/nonexistent.sock");
        let caps = Caps::default();
        handle(&mut ui, Act::Send, &client, &[], &caps, &tx).await;
        assert!(ui.status.contains("без задачи"), "{}", ui.status);
        assert!(rx.try_recv().is_err());
    }

    /// В сыром режиме `\n` приходит как Ctrl+J. Для человека это Enter — и
    /// окно обязано понимать его так же, иначе часть терминалов «не нажимает».
    #[test]
    fn ctrl_j_is_the_same_enter() {
        let cj = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL);
        assert_eq!(map_key(false, cj), Act::Open);
        assert_eq!(map_key(true, cj), Act::Send);
        // Без Ctrl «j» остаётся навигацией по списку и буквой в чате.
        assert_eq!(map_key(false, key('j')), Act::Down);
        assert_eq!(map_key(true, key('j')), Act::Type('j'));
    }

    #[test]
    fn letters_navigate_in_the_list_and_type_in_the_chat() {
        assert_eq!(map_key(false, key('j')), Act::Down);
        assert_eq!(map_key(false, key('q')), Act::Quit);
        // В чате те же буквы — это текст, иначе ответ агенту не написать.
        assert_eq!(map_key(true, key('j')), Act::Type('j'));
        assert_eq!(map_key(true, key('q')), Act::Type('q'));
        assert_eq!(
            map_key(true, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Act::Send
        );
        assert_eq!(
            map_key(false, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Act::Open
        );
    }

    /// Ctrl-C обязан работать везде: в сыром режиме сигнала нет, и без этой
    /// ветки человек остался бы запертым в окне.
    #[test]
    fn ctrl_c_quits_from_anywhere() {
        let c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(map_key(false, c), Act::Quit);
        assert_eq!(map_key(true, c), Act::Quit);
    }

    /// ESC перед клавишей приезжает как Alt: это по-прежнему «назад», а не
    /// буква в тексте сообщения.
    #[test]
    fn alt_is_read_as_escape() {
        let alt_s = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::ALT);
        assert_eq!(map_key(true, alt_s), Act::Escape);
        assert_eq!(map_key(false, alt_s), Act::Escape);
    }

    #[test]
    fn digits_answer_the_menu_question() {
        assert_eq!(map_key(false, key('2')), Act::Answer(2));
        // Ноль вариантом не бывает — пусть лучше ничего не делает.
        assert_eq!(map_key(false, key('0')), Act::None);
    }

    #[test]
    fn visible_window_sticks_to_the_bottom() {
        let items: Vec<u32> = (1..=10).collect();
        assert_eq!(visible(&items, 3, 0), &[8, 9, 10]);
        assert_eq!(visible(&items, 3, 2), &[6, 7, 8]);
        // Прокрутка дальше начала не должна выходить за край ленты.
        assert_eq!(visible(&items, 3, 100), &[1]);
        assert!(visible(&items, 0, 0).is_empty());
        assert!(visible::<u32>(&[], 5, 0).is_empty());
    }
}
