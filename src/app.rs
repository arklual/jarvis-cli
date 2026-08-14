//! Исполнение команд: связать разбор строки, узел и отрисовку.

use crate::cli::{BundleAction, Cmd, LoopAction, Parsed};
use crate::core::machine::{self, Machine};
use crate::core::node::NodeClient;
use crate::core::session::{self, Session};
use crate::core::state;
use crate::core::util::{clock, ellipsize, now_ms, one_line};
use crate::engine::{self, builder, presets};
use crate::ui::prompt;
use crate::ui::render::{self, Window};
use crate::ui::style::{paint, rule, width, Caps, Role};
use std::collections::HashMap;

pub struct App {
    pub caps: Caps,
    pub json: bool,
}

impl App {
    pub fn new(json: bool) -> Self {
        Self {
            caps: Caps::detect(),
            json,
        }
    }

    pub fn say(&self, line: impl AsRef<str>) {
        println!("{}", line.as_ref());
    }

    pub fn dim(&self, line: &str) {
        // Переносим по ширине окна: подсказки бывают длиннее строки, а
        // терминал рвёт их посреди слова.
        let room = (self.caps.width as usize).saturating_sub(1).max(20);
        for l in crate::ui::style::wrap(line, room) {
            println!("{}", paint(&self.caps, Role::Dim, &l));
        }
    }
}

/// Найти машину по имени: понятная ошибка вместо молчаливого «локально».
fn pick_machine(name: &str) -> Result<Machine, String> {
    let all = machine::list();
    all.iter().find(|m| m.name == name).cloned().ok_or_else(|| {
        let names: Vec<&str> = all.iter().map(|m| m.name.as_str()).collect();
        format!("нет машины «{name}». Есть: {}", names.join(", "))
    })
}

/// Собрать реестр сессий машины: берём то, что узел ещё помнит.
///
/// Просим ровно тот отрезок, что у него есть (`cursor - buffered`): `since=0`
/// на живой машине всегда упирался бы в `gap` с пустым списком — урок,
/// оплаченный мобильным клиентом.
pub async fn registry(client: &NodeClient) -> Result<HashMap<String, Session>, String> {
    let hello = client.hello().await?;
    if hello.buffered == 0 {
        return Ok(HashMap::new());
    }
    let since = hello.cursor.saturating_sub(hello.buffered);
    let page = client.events(since).await?;
    if page.gap {
        return Err("узел потерял начало ленты — перезапусти его".into());
    }
    Ok(session::apply(&HashMap::new(), &page.events))
}

/// Найти сессию по началу id или по имени проекта.
///
/// Однозначность обязательна: молча взять первую подходящую значит однажды
/// отправить ответ не тому агенту.
pub fn resolve<'a>(list: &'a [Session], needle: &str) -> Result<&'a Session, String> {
    let n = needle.trim().to_lowercase();
    if n.is_empty() {
        return Err("не назвал сессию".into());
    }
    let hits: Vec<&Session> = list
        .iter()
        .filter(|s| {
            s.id.to_lowercase().starts_with(&n)
                || s.project
                    .as_deref()
                    .map(|p| p.to_lowercase() == n)
                    .unwrap_or(false)
        })
        .collect();
    match hits.len() {
        1 => Ok(hits[0]),
        0 => Err(format!("не нашёл сессию «{needle}»")),
        _ => {
            let names: Vec<String> = hits
                .iter()
                .map(|s| format!("{} ({})", s.title(), &s.id[..s.id.len().min(8)]))
                .collect();
            Err(format!(
                "«{needle}» подходит нескольким: {}. Уточни идентификатором",
                names.join(", ")
            ))
        }
    }
}

/// Пана сессии — без неё ни ответить, ни нажать клавишу.
fn pane_of(s: &Session) -> Result<&str, String> {
    s.pane
        .as_deref()
        .filter(|p| !p.is_empty())
        .ok_or_else(|| format!("{} не в tmux — отвечать некуда", s.title()))
}

pub async fn run(parsed: Parsed) -> Result<(), String> {
    let app = App::new(parsed.json);
    match parsed.cmd {
        Cmd::Help => {
            print!("{}", crate::cli::help(&app.caps));
            Ok(())
        }
        Cmd::Version => {
            println!("jarvis {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Cmd::Ls => cmd_ls(&app, &parsed.machine).await,
        Cmd::Watch => crate::ui::live::run(&app, &parsed.machine).await,
        Cmd::Reply { session, text } => cmd_reply(&app, &parsed.machine, &session, &text).await,
        Cmd::Answer { session, option } => {
            cmd_answer(&app, &parsed.machine, &session, option).await
        }
        Cmd::Stop { session } => {
            cmd_key(&app, &parsed.machine, &session, "Escape", "прервал").await
        }
        Cmd::Screen { session } => cmd_screen(&app, &parsed.machine, &session).await,
        Cmd::Chat { session, follow } => cmd_chat(&app, &parsed.machine, &session, follow).await,
        Cmd::Projects => cmd_projects(&app, &parsed.machine).await,
        Cmd::Machines => cmd_machines(&app).await,
        Cmd::MachineAdd { name, host, dir } => cmd_machine_add(&app, &name, &host, &dir).await,
        Cmd::MachineRm { name } => cmd_machine_rm(&app, &name),
        Cmd::Control { session, cmd } => cmd_control(&app, &parsed.machine, &session, &cmd).await,
        Cmd::Run { dir, agent } => cmd_run(&app, &parsed.machine, &dir, &agent).await,
        Cmd::Limits { fresh } => cmd_limits(&app, &parsed.machine, fresh).await,
        Cmd::Loop { action } => cmd_loop(&app, &parsed.machine, action).await,
        Cmd::Bundle { action } => cmd_bundle(&app, &parsed.machine, action).await,
        Cmd::Notify { once } => crate::ui::watch::notify(&app, &parsed.machine, once).await,
    }
}

async fn connect(name: &str) -> Result<(NodeClient, Option<machine::Tunnel>), String> {
    let m = pick_machine(name)?;
    machine::connect(&m).await
}

async fn cmd_ls(app: &App, machine_name: &str) -> Result<(), String> {
    let (client, _t) = connect(machine_name).await?;
    let reg = registry(&client).await?;
    let list = session::sorted(&reg);
    if app.json {
        let out: Vec<_> = list
            .iter()
            .map(|s| {
                serde_json::json!({
                    "id": s.id, "title": s.title(), "status": s.status.word(),
                    "detail": s.detail, "question": s.question, "updatedAt": s.updated_at,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
        return Ok(());
    }
    if list.is_empty() {
        app.dim("Ни одной сессии — запусти агента, и она появится здесь сама.");
        return Ok(());
    }
    let col = render::name_column(&list);
    app.say(rule(&app.caps, "Сессии"));
    for s in &list {
        app.say(render::session_row(&app.caps, s, col));
    }
    println!();
    app.say(render::tally_line(&app.caps, &session::tally(&list)));
    Ok(())
}

async fn cmd_reply(app: &App, machine_name: &str, needle: &str, text: &str) -> Result<(), String> {
    let (client, _t) = connect(machine_name).await?;
    let reg = registry(&client).await?;
    let list = session::sorted(&reg);
    let s = resolve(&list, needle)?;
    let pane = pane_of(s)?;
    // Пана из реестра может быть мертва: агента закрыли, а событие о конце не
    // дошло. Отправка «в никуда» выглядела бы как успех — проверяем живых.
    if let Ok(reply) = client.panes().await {
        if !reply.panes.is_empty() && !reply.panes.iter().any(|p| p.pane == pane) {
            return Err(format!(
                "{} уже не живёт в tmux — ответ ушёл бы в никуда",
                s.title()
            ));
        }
    }
    client.reply(pane, text).await?;
    app.say(format!(
        "{} {}",
        paint(&app.caps, Role::Accent, "→"),
        paint(
            &app.caps,
            Role::Dim,
            &format!("{}: {}", s.title(), ellipsize(text, 60))
        )
    ));
    Ok(())
}

async fn cmd_answer(app: &App, machine_name: &str, needle: &str, option: u8) -> Result<(), String> {
    let (client, _t) = connect(machine_name).await?;
    let reg = registry(&client).await?;
    let list = session::sorted(&reg);
    let s = resolve(&list, needle)?;
    client
        .keys(
            pane_of(s)?,
            crate::core::node::key_plan(&option.to_string()),
        )
        .await?;
    app.say(format!(
        "{} вариант {option}",
        paint(&app.caps, Role::Accent, "→")
    ));
    Ok(())
}

async fn cmd_key(
    app: &App,
    machine_name: &str,
    needle: &str,
    key: &str,
    word: &str,
) -> Result<(), String> {
    let (client, _t) = connect(machine_name).await?;
    let reg = registry(&client).await?;
    let list = session::sorted(&reg);
    let s = resolve(&list, needle)?;
    client
        .keys(pane_of(s)?, crate::core::node::key_plan(key))
        .await?;
    app.dim(&format!("{word} — {}", s.title()));
    Ok(())
}

async fn cmd_screen(app: &App, machine_name: &str, needle: &str) -> Result<(), String> {
    let (client, _t) = connect(machine_name).await?;
    let reg = registry(&client).await?;
    let list = session::sorted(&reg);
    let s = resolve(&list, needle)?;
    let screen = client.screen(pane_of(s)?).await?;
    app.say(rule(&app.caps, &s.title()));
    println!("{}", screen.trim_end());
    Ok(())
}

/// Машины и связь с ними: спрашиваем каждую и говорим, что ответила.
async fn cmd_machines(app: &App) -> Result<(), String> {
    let all = machine::list();
    let col = all
        .iter()
        .map(|m| crate::ui::style::width(&m.name))
        .max()
        .unwrap_or(8)
        .clamp(6, 20);
    app.say(rule(&app.caps, "Машины"));
    for m in &all {
        // Проверяем связью, а не наличием строки в файле: «настроена» и
        // «отвечает» — разные вещи, и человеку важна вторая.
        let (word, role) = match machine::connect(m).await {
            Ok((client, _t)) => match client.hello().await {
                Ok(h) => {
                    // Считаем сессии по реестру: в hello есть только длина
                    // буфера событий, а «2000 сессий» — красивая неправда.
                    let n = registry(&client).await.map(|r| r.len()).unwrap_or(0);
                    (
                        format!(
                            "узел {} · {}",
                            h.version,
                            crate::core::util::plural(n as u64, "сессия", "сессии", "сессий")
                        ),
                        Role::Dim,
                    )
                }
                Err(e) => (format!("сокет есть, узел молчит: {e}"), Role::Bad),
            },
            Err(e) => (e, Role::Bad),
        };
        // Причина отказа бывает длинной и с советом внутри; в строке таблицы
        // ей место только до края экрана — целиком её скажет сама команда,
        // ради которой человек сюда и пришёл.
        // У локальной машины ssh-адреса нет, и пустая колонка выглядит как
        // недосказанность — говорим словами, что это за машина.
        let addr = if m.ssh_host.trim().is_empty() {
            "этот компьютер"
        } else {
            m.ssh_host.trim()
        };
        let host = crate::ui::style::pad(&crate::ui::style::truncate(addr, 22), 22);
        let room = (app.caps.width as usize).saturating_sub(col + 1 + width(&host) + 2);
        app.say(format!(
            "{} {}  {}",
            crate::ui::style::pad(&m.name, col),
            paint(&app.caps, Role::Dim, &host),
            paint(
                &app.caps,
                role,
                &crate::ui::style::truncate(&ellipsize(&one_line(&word), 400), room.max(20))
            )
        ));
    }
    Ok(())
}

async fn cmd_machine_add(app: &App, name: &str, host: &str, dir: &str) -> Result<(), String> {
    if name == "local" {
        return Err("«local» — это машина, на которой ты сейчас; переименовывать её некуда".into());
    }
    let m = Machine {
        name: name.to_string(),
        ssh_host: host.to_string(),
        dir: dir.trim_end_matches('/').to_string(),
    };
    let next = machine::upsert_remote(&machine::read_settings(), &m);
    machine::write_settings(&next)?;
    // Проверяем сразу: «записана» без проверки связи — обещание, о котором
    // человек узнаёт следующей командой, уже забыв про эту.
    let (word, extra) = match machine::connect(&m).await {
        Ok((client, _t)) => match client.hello().await {
            Ok(h) => (format!("записана и на связи · узел {}", h.version), None),
            Err(e) => (
                "записана".into(),
                Some(format!(
                    "ssh пускает, но узел молчит: {e}. На той стороне должен работать \
                     jarvis-node и слушать {}/node.sock",
                    m.dir
                )),
            ),
        },
        Err(e) => ("записана".into(), Some(e)),
    };
    app.say(paint(
        &app.caps,
        Role::Accent,
        &format!("машина «{name}» {word}"),
    ));
    if let Some(why) = extra {
        app.say(paint(&app.caps, Role::Bad, &why));
        app.dim("запись оставил: починишь связь — заработает без повторного add");
    }
    Ok(())
}

fn cmd_machine_rm(app: &App, name: &str) -> Result<(), String> {
    let next = machine::remove_remote(&machine::read_settings(), name)
        .ok_or_else(|| format!("машины «{name}» в настройках и не было"))?;
    machine::write_settings(&next)?;
    app.dim(&format!("машина «{name}» убрана"));
    Ok(())
}

async fn cmd_control(app: &App, machine_name: &str, needle: &str, cmd: &str) -> Result<(), String> {
    let (client, _t) = connect(machine_name).await?;
    let reg = registry(&client).await?;
    let list = session::sorted(&reg);
    let s = resolve(&list, needle)?;
    client.control(pane_of(s)?, cmd).await?;
    // Команда почти всегда что-то РИСУЕТ: пикер модели, сводку расхода. Дать
    // ей мгновение и показать экран — иначе человек гадает, сработало ли.
    tokio::time::sleep(std::time::Duration::from_millis(700)).await;
    let screen = client.screen(pane_of(s)?).await.unwrap_or_default();
    app.say(rule(&app.caps, &format!("{} · {cmd}", s.title())));
    println!("{}", screen.trim_end());
    Ok(())
}

async fn cmd_chat(app: &App, machine_name: &str, needle: &str, follow: bool) -> Result<(), String> {
    let (client, _t) = connect(machine_name).await?;
    let reg = registry(&client).await?;
    let list = session::sorted(&reg);
    let s = resolve(&list, needle)?;
    let path = s
        .transcript
        .as_deref()
        .filter(|p| !p.is_empty())
        .ok_or_else(|| format!("{} ещё не завела транскрипт", s.title()))?;
    crate::ui::chat::tail(app, &client, path, &s.title(), follow).await
}

async fn cmd_projects(app: &App, machine_name: &str) -> Result<(), String> {
    let (client, _t) = connect(machine_name).await?;
    let list = client.projects().await?;
    if app.json {
        let out: Vec<_> = list
            .iter()
            .map(|p| serde_json::json!({ "cwd": p.cwd, "count": p.count, "lastAt": p.last_at }))
            .collect();
        println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
        return Ok(());
    }
    if list.is_empty() {
        app.dim("Проектов не нашлось — здесь ещё не работали.");
        return Ok(());
    }
    app.say(rule(&app.caps, "Проекты"));
    for p in &list {
        app.say(crate::ui::chat::project_line(
            &app.caps, &p.cwd, p.count, p.last_at,
        ));
    }
    Ok(())
}

async fn cmd_run(app: &App, machine_name: &str, dir: &str, agent: &str) -> Result<(), String> {
    let (client, _t) = connect(machine_name).await?;
    // Команду собирает вызывающий, узел только исполняет — та же граница, что
    // у панели: флаги агента это наши настройки, не его.
    let cmd = if agent == "codex" {
        "codex --dangerously-bypass-approvals-and-sandbox".to_string()
    } else {
        "claude --dangerously-skip-permissions".to_string()
    };
    let name = dir
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("project");
    let pane = client.launch(dir, &cmd, name).await?;
    app.say(format!(
        "{} {}",
        paint(&app.caps, Role::Accent, "▶"),
        paint(
            &app.caps,
            Role::Dim,
            &format!("{name} — {agent} поднят, пана {pane}")
        )
    ));
    Ok(())
}

/// Разбор `/usage`: те же правила, что в панели — формат внутренний и дрейфует,
/// поэтому и пояс, и имя модели берутся из самого текста, а не зашиты.
pub fn parse_usage(text: &str) -> Vec<Window> {
    let mut out = Vec::new();
    let grab = |pat: &str| -> Option<(u8, i64)> {
        let re = regex::RegexBuilder::new(pat)
            .case_insensitive(true)
            .build()
            .ok()?;
        let c = re.captures(text)?;
        let pct: u8 = c.get(1)?.as_str().parse().ok()?;
        let reset = c.get(2).map(|m| parse_reset(m.as_str())).unwrap_or(0);
        Some((pct.min(100), reset))
    };
    if let Some((pct, at)) = grab(r"Current session:\s*(\d+)%\s*used\s*·\s*resets\s+([^\n]+)") {
        out.push(Window {
            label: "5ч".into(),
            pct,
            reset_at: at,
        });
    }
    if let Some((pct, at)) =
        grab(r"Current week \(all models\):\s*(\d+)%\s*used\s*·\s*resets\s+([^\n]+)")
    {
        out.push(Window {
            label: "нед".into(),
            pct,
            reset_at: at,
        });
    }
    // Модель в скобках — любая: Fable, Opus, «Sonnet only»… Зашитое имя
    // протухает с каждой сменой модельного ряда.
    if let Ok(re) = regex::RegexBuilder::new(
        r"Current week \(([^)]+)\):\s*(\d+)%\s*used(?:\s*·\s*resets\s+([^\n]+))?",
    )
    .case_insensitive(true)
    .build()
    {
        if let Some(c) = re
            .captures_iter(text)
            .find(|c| !c[1].eq_ignore_ascii_case("all models"))
        {
            out.push(Window {
                label: c[1].trim().to_string(),
                pct: c[2].parse::<u8>().unwrap_or(0).min(100),
                reset_at: c.get(3).map(|m| parse_reset(m.as_str())).unwrap_or(0),
            });
        }
    }
    out
}

/// «Aug 10, 6:59pm (UTC)» → миллисекунды. Пояс — из хвоста строки.
fn parse_reset(s: &str) -> i64 {
    let re = regex::RegexBuilder::new(
        r"([A-Z][a-z]{2})[a-z]*\s+(\d{1,2})(?:,|\s+at)?\s+(\d{1,2})(?::(\d{2}))?\s*(am|pm)?",
    )
    .case_insensitive(true)
    .build()
    .ok();
    let Some(c) = re.and_then(|r| {
        r.captures(s).map(|c| {
            (
                c[1].to_string(),
                c[2].to_string(),
                c[3].to_string(),
                c.get(4).map(|m| m.as_str().to_string()),
                c.get(5).map(|m| m.as_str().to_lowercase()),
            )
        })
    }) else {
        return 0;
    };
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let Some(month) = MONTHS.iter().position(|m| m.eq_ignore_ascii_case(&c.0)) else {
        return 0;
    };
    let day: u32 = c.1.parse().unwrap_or(1);
    let mut hh: u32 = c.2.parse().unwrap_or(0);
    match c.4.as_deref() {
        Some("pm") if hh < 12 => hh += 12,
        Some("am") if hh == 12 => hh = 0,
        _ => {}
    }
    let min: u32 = c.3.and_then(|m| m.parse().ok()).unwrap_or(0);
    let up = s.to_uppercase();
    let fixed: Option<i64> = if up.contains("UTC") {
        Some(0)
    } else if up.contains("EUROPE/MOSCOW") {
        Some(3 * 3_600_000)
    } else {
        None
    };
    let now = now_ms();
    let year = chrono::DateTime::from_timestamp_millis(now)
        .map(|d| chrono::Datelike::year(&d))
        .unwrap_or(2026);
    let make = |y: i32| -> i64 {
        let Some(naive) = chrono::NaiveDate::from_ymd_opt(y, month as u32 + 1, day)
            .and_then(|d| d.and_hms_opt(hh, min, 0))
        else {
            return 0;
        };
        match fixed {
            Some(off) => naive.and_utc().timestamp_millis() - off,
            None => {
                use chrono::TimeZone;
                chrono::Local
                    .from_local_datetime(&naive)
                    .single()
                    .map(|d| d.timestamp_millis())
                    .unwrap_or(0)
            }
        }
    };
    let mut ts = make(year);
    if ts != 0 && ts < now - 12 * 3_600_000 {
        ts = make(year + 1);
    }
    ts
}

async fn cmd_limits(app: &App, machine_name: &str, fresh: bool) -> Result<(), String> {
    let (client, _t) = connect(machine_name).await?;
    let text = client.usage_text(fresh).await?;
    let windows = parse_usage(&text);
    if app.json {
        let out: Vec<_> = windows
            .iter()
            .map(|w| serde_json::json!({ "label": w.label, "pct": w.pct, "resetAt": w.reset_at }))
            .collect();
        println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
        return Ok(());
    }
    app.say(rule(&app.caps, "Лимиты"));
    if windows.is_empty() {
        app.dim("формат /usage не разобрался — вот что ответил агент:");
        println!("{}", text.trim());
        return Ok(());
    }
    let col = windows
        .iter()
        .map(|w| crate::ui::style::width(&w.label))
        .max()
        .unwrap_or(6)
        .clamp(4, 14);
    for w in &windows {
        app.say(render::limit_row(&app.caps, w, col));
    }
    Ok(())
}

/* ---------- циклы ---------- */

fn find_loop(id: &str) -> Result<state::Loop, String> {
    let all = state::load_loops();
    all.iter()
        .find(|l| l.id == id || l.name == id || l.id.starts_with(id))
        .cloned()
        .ok_or_else(|| {
            if all.is_empty() {
                "циклов пока нет — заведи их в панели или командой loop".to_string()
            } else {
                format!(
                    "нет цикла «{id}». Есть: {}",
                    all.iter()
                        .map(|l| l.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        })
}

/// Каталог по умолчанию для конструктора: здесь — откуда запущено, на узле —
/// его домашний. Путь этого компьютера на чужой машине бессмыслен.
async fn default_dir(machine_name: &str) -> String {
    let Ok(m) = machine::find(machine_name) else {
        return ".".into();
    };
    if m.is_local() {
        return std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| ".".into());
    }
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

async fn cmd_loop(app: &App, machine_name: &str, action: LoopAction) -> Result<(), String> {
    match action {
        LoopAction::New => {
            if !prompt::interactive() {
                return Err("конструктор спрашивает — запусти его в терминале, не в пайпе".into());
            }
            let cwd = default_dir(machine_name).await;
            let l = builder::new_loop(&app.caps, &cwd, machine_name)?;
            println!();
            for line in builder::summary(&app.caps, &l) {
                app.say(line);
            }
            // Дыры показываем ДО согласия: «завёл и не работает» — худший
            // исход конструктора, ради которого его и делали.
            let problems = l.problems();
            if !problems.is_empty() {
                println!();
                for p in &problems {
                    app.say(paint(&app.caps, Role::Bad, &format!("× {p}")));
                }
            }
            println!();
            if !prompt::yes(&app.caps, "Завести", problems.is_empty())? {
                app.dim("не завожу");
                return Ok(());
            }
            let mut all = state::load_loops();
            all.push(l.clone());
            state::save_loops(&all).map_err(|e| format!("не записал циклы: {e}"))?;
            app.say(paint(
                &app.caps,
                Role::Accent,
                &format!("цикл «{}» заведён", l.name),
            ));
            app.dim(&format!(
                "запустить: jarvis loop start {} · он же виден в панели",
                l.name
            ));
            Ok(())
        }
        LoopAction::Presets => {
            let src = builder::catalog(presets::Slot::Source);
            let gates = builder::catalog(presets::Slot::Gate);
            if app.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&presets::all()).unwrap_or_default()
                );
                return Ok(());
            }
            prompt::list(&app.caps, "Источники задач", &builder::choices(&src));
            println!();
            prompt::list(&app.caps, "Гейты", &builder::choices(&gates));
            println!();
            app.dim("выбираются в конструкторе: jarvis loop new");
            Ok(())
        }
        LoopAction::Say { id, text } => {
            let l = find_loop(&id)?;
            let mut run =
                state::load_run(&l.id).ok_or("цикл ещё не запускался — отвечать не на что")?;
            // Реплика едет в следующую итерацию целиком: это ответ на вопрос,
            // а вопрос был задан не для галочки.
            run.interventions.push(text.clone());
            let asked = run.ask.take().map(|a| a.question);
            if run.state == state::RunState::Asking {
                run.state = state::RunState::Idle;
            }
            state::save_run(&run).map_err(|e| format!("не записал запуск: {e}"))?;
            match asked {
                Some(q) => app.dim(&format!("ответ принят на «{}»", ellipsize(&q, 60))),
                None => app.dim("цикл ни о чём не спрашивал — скажу это на следующей итерации"),
            }
            app.dim(&format!("продолжить: jarvis loop start {}", l.name));
            Ok(())
        }
        LoopAction::Rm { id } => {
            let l = find_loop(&id)?;
            let mut all = state::load_loops();
            all.retain(|x| x.id != l.id);
            state::save_loops(&all).map_err(|e| format!("не записал циклы: {e}"))?;
            app.dim(&format!("цикл «{}» убран; журнал запуска остался", l.name));
            Ok(())
        }
        LoopAction::Ls => {
            let all = state::load_loops();
            if all.is_empty() {
                app.dim("Циклов пока нет. Цикл — рутина, которую агент крутит сам: у него есть конец (условие выхода) и стены (ограничители).");
                return Ok(());
            }
            let col = all
                .iter()
                .map(|l| crate::ui::style::width(&l.name))
                .max()
                .unwrap_or(10)
                .clamp(8, 24);
            app.say(rule(&app.caps, "Циклы"));
            for l in &all {
                let run = state::load_run(&l.id);
                app.say(render::loop_row(&app.caps, l, run.as_ref(), col));
            }
            Ok(())
        }
        LoopAction::Show { id } => {
            let l = find_loop(&id)?;
            let run = state::load_run(&l.id);
            app.say(rule(&app.caps, &l.name));
            app.dim(&format!(
                "{}{} · {} · выход: {} подряд",
                if l.machine.is_empty() || l.machine == "local" {
                    String::new()
                } else {
                    format!("{}:", l.machine)
                },
                l.sandbox.repo,
                l.wake_label(),
                l.exit.streak
            ));
            let Some(run) = run else {
                app.dim("ещё не запускался");
                return Ok(());
            };
            println!();
            app.dim(&format!(
                "запуск {} · {} · {} токенов · {}",
                run.n,
                run.branch,
                render::fmt_tokens(run.tokens),
                run.stop.word()
            ));
            for it in run.iterations.iter().rev().take(20) {
                app.say(render::iteration_row(&app.caps, it));
            }
            if let Some(ask) = &run.ask {
                println!();
                app.say(paint(
                    &app.caps,
                    Role::Accent,
                    &format!("спрашивает: {}", ask.question),
                ));
            }
            Ok(())
        }
        LoopAction::Start { id } => {
            let l = find_loop(&id)?;
            let problems = l.problems();
            if !problems.is_empty() {
                return Err(format!("цикл не заполнен: {}", problems.join("; ")));
            }
            // Печать — дело команды: движок теперь только рассказывает, что
            // происходит, и тем же рассказом пользуется живое окно.
            crate::engine::loops::start(&l, &mut |n| say_note(app, n)).await
        }
        LoopAction::Stop { id } => {
            let l = find_loop(&id)?;
            let Some(mut run) = state::load_run(&l.id) else {
                return Err("этот цикл не запускался".into());
            };
            run.state = state::RunState::Stopped;
            run.stop = state::StopReason::Stopped;
            run.stop_note = "остановлен из терминала".into();
            run.ended_at = now_ms();
            state::save_run(&run).map_err(|e| format!("не записал журнал: {e}"))?;
            app.dim(&format!("{} остановлен — ветка цела", l.name));
            Ok(())
        }
    }
}

/* ---------- связка ---------- */

fn find_bundle(id: &str) -> Result<(Vec<state::Bundle>, usize), String> {
    let all = state::load_bundles();
    let idx = all
        .iter()
        .position(|b| b.id == id || b.name == id || b.id.starts_with(id))
        .ok_or_else(|| {
            if all.is_empty() {
                "связок пока нет — заведи их в панели".to_string()
            } else {
                format!(
                    "нет связки «{id}». Есть: {}",
                    all.iter()
                        .map(|b| b.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        })?;
    Ok((all, idx))
}

/// Ход цикла в поток — так, как он выглядел, когда печатал сам движок.
fn say_note(app: &App, n: crate::engine::loops::Note) {
    use crate::engine::loops::Note;
    match n {
        Note::Head {
            name,
            branch,
            dir,
            streak,
        } => {
            app.say(rule(&app.caps, &name));
            app.dim(&format!("{branch} · {dir} · выход: {streak} подряд"));
        }
        Note::Started(n) => app.dim(&format!("итерация {n} — пошла")),
        Note::Done(it) => app.say(render::iteration_row(&app.caps, &it)),
        Note::Ask { name, question } => {
            app.say(paint(
                &app.caps,
                Role::Warn,
                &format!("цикл спрашивает: {question}"),
            ));
            app.dim(&format!(
                "ответить: jarvis loop say {name} <текст>, дальше loop start"
            ));
        }
        Note::Finished {
            reason,
            iterations,
            tokens,
            pending,
        } => {
            println!();
            let role = if reason == crate::core::state::StopReason::Exit {
                Role::Ok
            } else {
                Role::Muted
            };
            app.say(paint(
                &app.caps,
                role,
                &format!(
                    "{} · {} · {} токенов",
                    reason.word(),
                    crate::core::util::plural(
                        iterations as u64,
                        "итерация",
                        "итерации",
                        "итераций"
                    ),
                    render::fmt_tokens(tokens)
                ),
            ));
            if pending > 0 {
                app.say(paint(
                    &app.caps,
                    Role::Accent,
                    &format!(
                        "{} ждут твоего взгляда — jarvis loop show",
                        crate::core::util::plural(
                            pending as u64,
                            "итерация",
                            "итерации",
                            "итераций"
                        )
                    ),
                ));
            }
        }
    }
}

/// Отчёт движка: первая строка — итог, остальные — подробности.
fn say_report(app: &App, report: &[String]) {
    let mut it = report.iter();
    if let Some(head) = it.next() {
        app.say(paint(&app.caps, Role::Accent, head));
    }
    for rest in it {
        app.dim(rest);
    }
}

async fn cmd_bundle(app: &App, machine: &str, action: BundleAction) -> Result<(), String> {
    match action {
        BundleAction::New => {
            if !prompt::interactive() {
                return Err("конструктор спрашивает — запусти его в терминале, не в пайпе".into());
            }
            let cwd = default_dir(machine).await;
            let b = builder::new_bundle(&app.caps, &cwd, machine)?;
            let mut all = state::load_bundles();
            all.push(b.clone());
            state::save_bundles(&all).map_err(|e| format!("не записал связки: {e}"))?;
            app.say(paint(
                &app.caps,
                Role::Accent,
                &format!("связка «{}» заведена", b.name),
            ));
            app.dim(&format!(
                "добавь руки: jarvis bundle hand {} <задача>",
                b.name
            ));
            Ok(())
        }
        BundleAction::Hand { id, task } => {
            let (all, i) = find_bundle(&id)?;
            let report = engine::bundle::add_hand(all, i, &task).await?;
            say_report(app, &report);
            Ok(())
        }
        BundleAction::Ls => {
            let all = state::load_bundles();
            if all.is_empty() {
                app.dim("Связок пока нет. Связка — несколько агентов «в 10 рук» над одним проектом, с очередью слияний.");
                return Ok(());
            }
            app.say(rule(&app.caps, "Связки"));
            for b in &all {
                let q = b.queue().len();
                app.say(format!(
                    "{}  {}  {}",
                    crate::ui::style::pad(&b.name, 18),
                    paint(
                        &app.caps,
                        Role::Dim,
                        &crate::core::util::plural(b.hands.len() as u64, "рука", "руки", "рук")
                    ),
                    if q > 0 {
                        paint(&app.caps, Role::Accent, &format!("{q} в очереди"))
                    } else {
                        paint(&app.caps, Role::Dim, "очередь пуста")
                    }
                ));
            }
            Ok(())
        }
        BundleAction::Show { id } => {
            let (all, i) = find_bundle(&id)?;
            let b = &all[i];
            app.say(rule(&app.caps, &b.name));
            app.dim(&format!(
                "{} · {} → {}",
                if b.machine.is_empty() {
                    "local"
                } else {
                    &b.machine
                },
                b.dir,
                if b.base.is_empty() { "main" } else { &b.base }
            ));
            let col = b
                .hands
                .iter()
                .map(|h| crate::ui::style::width(&h.name))
                .max()
                .unwrap_or(10)
                .clamp(8, 20);
            for h in &b.hands {
                app.say(render::hand_row(&app.caps, b, &h.id, col));
            }
            let queue = b.queue();
            if !queue.is_empty() {
                println!();
                app.say(paint(
                    &app.caps,
                    Role::Accent,
                    &format!("влить: jarvis bundle merge {} {}", b.name, queue[0].name),
                ));
            }
            if !b.events.is_empty() {
                println!();
                app.say(rule(&app.caps, "лента"));
                for e in b.events.iter().rev().take(8) {
                    app.dim(&format!("{}  {}", clock(e.at), e.text));
                }
            }
            Ok(())
        }
        BundleAction::Rm { id, force, clean } => {
            let (all, i) = find_bundle(&id)?;
            let report = engine::bundle::remove(all, i, force, clean).await?;
            say_report(app, &report);
            Ok(())
        }
        BundleAction::Merge { id, hand } => {
            let (all, i) = find_bundle(&id)?;
            let report = crate::engine::bundle::merge(all, i, &hand).await?;
            say_report(app, &report);
            Ok(())
        }
        BundleAction::Pause { id, on } => {
            let (mut all, i) = find_bundle(&id)?;
            all[i].paused = on;
            let word = if on {
                "пауза"
            } else {
                "продолжает"
            };
            all[i].event(format!("{word} — из терминала"));
            state::save_bundles(&all).map_err(|e| format!("не записал: {e}"))?;
            app.dim(&format!("{}: {word}", all[i].name));
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sess(id: &str, project: &str) -> Session {
        Session {
            id: id.into(),
            project: Some(project.into()),
            ..Default::default()
        }
    }

    #[test]
    fn resolve_finds_by_prefix_and_project() {
        let list = vec![sess("abc123", "jarvis"), sess("def456", "lct")];
        assert_eq!(resolve(&list, "abc").unwrap().id, "abc123");
        assert_eq!(resolve(&list, "lct").unwrap().id, "def456");
        assert_eq!(
            resolve(&list, "ABC").unwrap().id,
            "abc123",
            "регистр не важен"
        );
    }

    /// Молча взять первую подходящую — однажды отправить ответ не тому агенту.
    #[test]
    fn ambiguous_needle_is_refused_with_the_candidates() {
        let list = vec![sess("abc1", "one"), sess("abc2", "two")];
        let err = resolve(&list, "abc").unwrap_err();
        assert!(err.contains("нескольким"), "{err}");
        assert!(err.contains("one") && err.contains("two"), "{err}");
    }

    #[test]
    fn missing_session_says_so() {
        let list = vec![sess("abc1", "one")];
        assert!(resolve(&list, "zzz").unwrap_err().contains("не нашёл"));
        assert!(resolve(&list, "  ").unwrap_err().contains("не назвал"));
    }

    #[test]
    fn pane_absence_is_explained() {
        let s = sess("a", "проект");
        let err = pane_of(&s).unwrap_err();
        assert!(err.contains("не в tmux"), "{err}");
    }

    /// Живой вывод `claude /usage` от 2026-08-10 — дословно.
    #[test]
    fn usage_parses_the_real_output() {
        let text = "Current session: 62% used · resets Aug 10, 6:59pm (UTC)\n\
                    Current week (all models): 94% used · resets Aug 10, 10:59pm (UTC)\n\
                    Current week (Fable): 54% used · resets Aug 10, 11pm (UTC)\n";
        let w = parse_usage(text);
        assert_eq!(w.len(), 3);
        assert_eq!((w[0].label.as_str(), w[0].pct), ("5ч", 62));
        assert_eq!((w[1].label.as_str(), w[1].pct), ("нед", 94));
        assert_eq!((w[2].label.as_str(), w[2].pct), ("Fable", 54));
        assert!(w[0].reset_at > 0, "время сброса обязано разбираться");
    }

    #[test]
    fn usage_survives_the_older_format() {
        let text = "Current session: 10% used · resets Aug 10 at 6:59pm\n\
                    Current week (Sonnet only): 30% used\n";
        let w = parse_usage(text);
        assert_eq!(w[0].pct, 10);
        assert_eq!(w[1].label, "Sonnet only");
        assert!(parse_usage("совсем не тот текст").is_empty());
    }

    #[test]
    fn reset_honours_the_utc_marker() {
        use chrono::{Datelike, TimeZone, Timelike};
        let ts = parse_reset("Aug 10, 6:59pm (UTC)");
        let dt = chrono::Utc.timestamp_millis_opt(ts).single().unwrap();
        assert_eq!(
            (dt.month(), dt.day(), dt.hour(), dt.minute()),
            (8, 10, 18, 59)
        );
        assert_eq!(parse_reset("не дата"), 0);
    }
}
