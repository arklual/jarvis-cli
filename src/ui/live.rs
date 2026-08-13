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
use crate::ui::render::{self, Window};
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

#[derive(Debug, Clone, PartialEq)]
enum View {
    List,
    Chat,
    Screen,
    Loops,
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
    None,
}

/// Клавиша в намерение. Отдельной функцией — чтобы раскладку проверяли тесты,
/// а не пальцы: в чате «j» это буква, в списке — «вниз», и перепутать их
/// значит писать сообщения вместо навигации.
pub fn map_key(view_is_text: bool, k: KeyEvent) -> Act {
    // Alt+клавиша терминал шлёт как ESC перед этой клавишей — то же, что
    // «нажали Esc, потом её». Считаем это Escape: иначе быстрый Esc перед
    // следующим нажатием слипается в Alt и молча превращается в букву.
    if k.modifiers.contains(KeyModifiers::ALT) {
        return Act::Escape;
    }
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    if ctrl {
        return match k.code {
            KeyCode::Char('c') => Act::Quit,
            KeyCode::Char('u') => Act::KillLine,
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
        KeyCode::PageUp => Act::PageUp,
        KeyCode::PageDown => Act::PageDown,
        KeyCode::Backspace => Act::Backspace,
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
    input: String,
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
    /// Выбранная связка и выбранная рука в её пульте.
    bsel: usize,
    hsel: usize,
    open_bundle: String,
    /// Набираем задачу новой руки: ввод занят не ответом агенту.
    composing: bool,
    /// Долгое действие уже идёт. Второе нажатие «влить» подряд — это два
    /// слияния, а очередь такого не прощает.
    busy: Option<String>,
}

impl Default for Ui {
    fn default() -> Self {
        Self {
            view: View::List,
            prev: View::List,
            sel: 0,
            input: String::new(),
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
            bsel: 0,
            hsel: 0,
            open_bundle: String::new(),
            composing: false,
            busy: None,
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
        present(&frame(&caps, &m.name, &ui, &list, rows), &mut shown);

        // 1. Клавиши — первым делом: отклик важнее свежести данных.
        while poll(TICK).map_err(|e| e.to_string())? {
            let Event::Key(k) = read().map_err(|e| e.to_string())? else {
                continue;
            };
            let act = map_key(ui.view == View::Chat || ui.composing, k);
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
                Msg::Done(res) => {
                    // Долгое действие закончилось: снимаем занятость и
                    // показываем итог там же, где человек его ждёт.
                    ui.busy = None;
                    ui.status = match res {
                        Ok(report) => report.join(" · "),
                        Err(e) => format!("не вышло: {e}"),
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
        Act::Escape if ui.composing => {
            ui.composing = false;
            ui.input.clear();
            ui.status.clear();
        }
        Act::Escape => match ui.view {
            View::List => return false,
            View::Chat if !ui.input.is_empty() => ui.input.clear(),
            View::Bundle => {
                ui.view = View::Bundles;
                ui.open_bundle.clear();
            }
            _ => {
                ui.view = View::List;
                ui.feed = None;
                ui.items.clear();
                ui.scroll = 0;
            }
        },
        Act::Up => match ui.view {
            View::List => ui.sel = ui.sel.saturating_sub(1),
            View::Chat => ui.scroll += 1,
            View::Bundles => ui.bsel = ui.bsel.saturating_sub(1),
            View::Bundle => ui.hsel = ui.hsel.saturating_sub(1),
            _ => {}
        },
        Act::Down => match ui.view {
            View::List => ui.sel = (ui.sel + 1).min(list.len().saturating_sub(1)),
            View::Chat => ui.scroll = ui.scroll.saturating_sub(1),
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
                        ui.status.clear();
                    }
                    Err(e) => ui.status = e,
                },
                _ => ui.status = "у сессии ещё нет транскрипта".into(),
            }
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
        Act::Send if ui.composing => {
            let task = ui.input.trim().to_string();
            if task.is_empty() {
                ui.status = "рука без задачи не поднимется".into();
                return true;
            }
            let Some(b) = current_bundle(ui).cloned() else {
                return true;
            };
            ui.composing = false;
            ui.input.clear();
            ui.busy = Some("поднимаю руку".into());
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
            let text = ui.input.trim().to_string();
            if text.is_empty() {
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
                    ui.input.clear();
                    ui.scroll = 0;
                    // Своё сообщение не дорисовываем: оно приедет из
                    // транскрипта, и подделка рядом с настоящей строкой
                    // выглядела бы двойной отправкой.
                    ui.status = format!("→ {}", truncate(&text, (caps.width as usize).min(60)));
                }
                Err(e) => ui.status = e,
            }
        }
        Act::Type(c) => ui.input.push(c),
        Act::Backspace => {
            ui.input.pop();
        }
        Act::KillLine => ui.input.clear(),
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
            ui.busy = Some(format!("вливаю {head}"));
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
        Act::NewHand => {
            if ui.view != View::Bundle {
                return true;
            }
            if ui.busy.is_some() {
                ui.status = "погоди, прошлое действие ещё идёт".into();
                return true;
            }
            // Рука без задачи бессмысленна, поэтому сначала спрашиваем её.
            ui.composing = true;
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
    let (left, right) = match ui.view {
        View::List => (
            format!("Jarvis · {machine}"),
            crate::ui::style::strip(&render::tally_line(caps, &session::tally(list))),
        ),
        View::Chat => (format!("{} · чат", ui.open_title), machine.to_string()),
        View::Screen => (format!("{} · экран", ui.open_title), machine.to_string()),
        View::Loops => ("Циклы".into(), machine.to_string()),
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
    push(&mut out, &header(caps, &left, &right, total));
    push(&mut out, "");

    // Полосы снизу: воздух, состояние, ввод (в чате) и клавиши.
    let body = (rows as usize).saturating_sub(if ui.view == View::Chat || ui.composing {
        6
    } else {
        5
    });
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
        View::Bundles => draw_bundles(&mut out, caps, ui, body),
        View::Bundle => draw_bundle(&mut out, caps, ui, body),
        View::Help => draw_help(&mut out, caps),
    }

    push(&mut out, "");
    if ui.view == View::List {
        push(
            &mut out,
            &format!(" {}", render::limits_line(caps, &ui.limits)),
        );
    } else if !ui.status.is_empty() {
        push(
            &mut out,
            &format!(" {}", paint(caps, Role::Muted, &ui.status)),
        );
    } else {
        push(&mut out, "");
    }
    if ui.view == View::Chat || ui.composing {
        // Строка ввода — на подложке: видно, где кончается лента и начинается
        // то, что ты печатаешь. Каретка — обратным цветом, как в pi.
        let room = total.saturating_sub(6);
        let typed = truncate(&ui.input, room);
        let caret = if caps.color {
            "\x1b[7m \x1b[27m".to_string()
        } else {
            "_".to_string()
        };
        push(
            &mut out,
            &band(
                caps,
                Bg::Sel,
                &format!("{} {typed}{caret}", paint(caps, Role::Accent, "›")),
                total,
            ),
        );
    }
    push(&mut out, &format!(" {}", keys_hint(caps, ui)));
    // Хвостовой перевод строки убираем: он-то и прокручивает полный экран.
    while out.ends_with("\r\n") {
        out.truncate(out.len() - 2);
    }
    out
}

/// Строка в конце — единственная подсказка, которую человек читает. Поэтому в
/// ней ровно то, что работает ЗДЕСЬ, а не весь список возможностей: клавиша
/// выделена, объяснение приглушено, пары разделены точкой.
fn keys_hint(caps: &Caps, ui: &Ui) -> String {
    let pairs: Vec<(&str, &str)> = match ui.view {
        View::List => vec![
            ("↑↓", "выбор"),
            ("↵", "чат"),
            ("s", "экран"),
            ("x", "прервать"),
            ("1-9", "ответ"),
            ("l", "циклы"),
            ("b", "связки"),
            ("?", "клавиши"),
            ("q", "выход"),
        ],
        View::Chat => vec![("↵", "отправить"), ("↑↓", "прокрутка"), ("esc", "назад")],
        View::Bundles => vec![("↑↓", "выбор"), ("↵", "пульт"), ("esc", "назад")],
        View::Bundle if ui.composing => vec![("↵", "поднять руку"), ("esc", "отмена")],
        View::Bundle => vec![
            ("↑↓", "выбор"),
            ("↵", "чат руки"),
            ("m", "влить голову"),
            ("n", "рука"),
            ("p", "пауза"),
            ("esc", "назад"),
        ],
        View::Screen | View::Loops | View::Help => {
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
            &paint(caps, Role::Dim, "Циклов пока нет. Завести: jarvis loop new"),
        );
        return;
    }
    let col = all
        .iter()
        .map(|l| width(&l.name))
        .max()
        .unwrap_or(10)
        .clamp(8, 24);
    for l in all.iter().take(rows) {
        let run = state::load_run(&l.id);
        push(out, &render::loop_row(caps, l, run.as_ref(), col));
    }
}

fn draw_bundles(out: &mut String, caps: &Caps, ui: &Ui, rows: usize) {
    if ui.bundles.is_empty() {
        push(
            out,
            &paint(
                caps,
                Role::Muted,
                " Связок пока нет. Завести: jarvis bundle new",
            ),
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
        let _ = std::io::stdout().flush();
        Self(crossterm::terminal::enable_raw_mode().is_ok())
    }
}

impl Drop for RawGuard {
    fn drop(&mut self) {
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
        ui.busy = Some("вливаю доки".into());
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
        assert!(ui.composing, "не спросил задачу");
        handle(&mut ui, Act::Type('д'), &client, &[], &caps, &tx).await;
        assert_eq!(ui.input, "д");
        handle(&mut ui, Act::Escape, &client, &[], &caps, &tx).await;
        assert!(!ui.composing && ui.input.is_empty(), "Esc не отменил набор");
        assert_eq!(ui.view, View::Bundle, "Esc из набора вышвырнул из пульта");
    }

    /// Пустая задача руку не поднимает: это молчаливая сессия без цели.
    #[tokio::test]
    async fn empty_task_does_not_launch_a_hand() {
        let mut ui = bundle_ui();
        ui.composing = true;
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
