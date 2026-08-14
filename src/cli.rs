//! Разбор командной строки.
//!
//! Своими руками, без парсер-фреймворка: набор команд небольшой и стабильный,
//! а зависимость ради него потянула бы полдюжины чужих крейтов в бинарь,
//! который должен ставиться одним файлом. Заодно справка пишется по-русски
//! ровно так, как её приятно читать, а не как её печатает генератор.

#[derive(Debug, Clone, PartialEq)]
pub enum Cmd {
    /// Живой экран: сессии, лимиты, что требует человека.
    Watch,
    /// Список сессий разово.
    Ls,
    /// Ответить сессии: `jarvis reply <сессия> <текст…>`.
    Reply {
        session: String,
        text: String,
    },
    /// Ответить на вопрос цифрой: `jarvis answer <сессия> <n>`.
    Answer {
        session: String,
        option: u8,
    },
    /// Прервать агента.
    Stop {
        session: String,
    },
    /// Экран паны сессии — что видно в её терминале.
    Screen {
        session: String,
    },
    /// Лента чата сессии; `--follow` — дочитывать вживую.
    Chat {
        session: String,
        follow: bool,
    },
    /// Проекты машины: где на ней работали.
    Projects,
    /// Машины: список, добавить, убрать.
    Machines,
    MachineAdd {
        name: String,
        host: String,
        dir: String,
    },
    MachineRm {
        name: String,
    },
    /// Слэш-команда пульта в сессию: `jarvis cmd <сессия> /model opus`.
    Control {
        session: String,
        cmd: String,
    },
    /// Поднять агента в каталоге: `jarvis run <путь> [--agent codex]`.
    Run {
        dir: String,
        agent: String,
    },
    /// Лимиты аккаунта.
    Limits {
        fresh: bool,
    },
    /// Циклы: `jarvis loop [ls|show <id>|start <id>|stop <id>]`.
    Loop {
        action: LoopAction,
    },
    /// Связка: `jarvis bundle [ls|show <id>|merge <id> <рука>]`.
    Bundle {
        action: BundleAction,
    },
    /// Уведомления: следить и печатать переходы (для интеграции со звуком).
    Notify {
        once: bool,
    },
    Help,
    Version,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LoopAction {
    Ls,
    New,
    Presets,
    Rm {
        id: String,
    },
    /// Реплика человека циклу: снимает вопрос и уходит в следующую итерацию.
    Say {
        id: String,
        text: String,
    },
    Show {
        id: String,
    },
    Start {
        id: String,
    },
    Stop {
        id: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum BundleAction {
    Ls,
    New,
    Hand {
        id: String,
        task: String,
    },
    Show {
        id: String,
    },
    Merge {
        id: String,
        hand: String,
    },
    Pause {
        id: String,
        on: bool,
    },
    Rm {
        id: String,
        force: bool,
        clean: bool,
    },
}

/// Разобранная команда вместе с общими флагами.
#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    pub cmd: Cmd,
    /// На какой машине работать: `local` или имя узла.
    pub machine: String,
    /// Печатать JSON вместо человеческого вывода — для скриптов.
    pub json: bool,
}

/// Строка справки: заголовок раздела, пара «команда — что делает» или
/// примечание.
///
/// Таблицей, а не готовым текстом: выравнивание описаний считается по самой
/// длинной команде, и добавленная команда не заставляет переставлять пробелы
/// во всём файле — раньше три строки из-за этого стояли вкось.
pub enum Help {
    Gap,
    Head(&'static str),
    Cmd(&'static str, &'static str),
    Note(&'static str),
}

pub fn help_rows() -> &'static [Help] {
    use Help::*;
    &[
        Cmd("jarvis", "живой экран: кто работает, кто спрашивает"),
        Cmd("jarvis ls", "список сессий разово"),
        Cmd("jarvis reply <id> <текст>", "ответить агенту"),
        Cmd("jarvis answer <id> <n>", "ответить на вопрос вариантом n"),
        Cmd("jarvis stop <id>", "прервать агента"),
        Cmd("jarvis screen <id>", "показать экран сессии"),
        Cmd("jarvis chat <id> [-f]", "лента чата; -f — следить вживую"),
        Cmd("jarvis projects", "проекты машины"),
        Cmd("jarvis cmd <id> /model …", "слэш-команда пульта в сессию"),
        Cmd("jarvis machines", "машины и связь с ними"),
        Cmd("jarvis machine add <имя> <user@host>", "добавить машину"),
        Cmd("jarvis machine rm <имя>", "убрать машину"),
        Cmd("jarvis run <каталог>", "поднять агента в каталоге"),
        Cmd("jarvis limits", "лимиты аккаунта"),
        Gap,
        Head("Циклы — рутина, которую агент крутит сам"),
        Cmd("jarvis loop new", "завести цикл — с каталогом заготовок"),
        Cmd("jarvis loop ls", "какие циклы есть и что с ними"),
        Cmd("jarvis loop show <id>", "журнал итераций цикла"),
        Cmd("jarvis loop start <id>", "запустить цикл"),
        Cmd("jarvis loop stop <id>", "остановить цикл"),
        Cmd(
            "jarvis loop say <id> <текст>",
            "ответить циклу на его вопрос",
        ),
        Cmd(
            "jarvis loop presets",
            "каталог заготовок: источники задач и гейты",
        ),
        Cmd("jarvis loop rm <id>", "убрать цикл"),
        Gap,
        Head("Связки — несколько агентов над одним проектом"),
        Cmd("jarvis bundle new", "завести связку"),
        Cmd("jarvis bundle ls", "какие связки есть и что с ними"),
        Cmd(
            "jarvis bundle show <id>",
            "пульт связки: руки, очередь слияний",
        ),
        Cmd(
            "jarvis bundle hand <id> <задача>",
            "добавить руку и поднять её агента",
        ),
        Cmd("jarvis bundle merge <id> <рука>", "влить голову очереди"),
        Cmd("jarvis bundle pause <id>", "пауза всем рукам"),
        Cmd(
            "jarvis bundle rm <id>",
            "убрать связку (--clean — снести и worktree рук)",
        ),
        Gap,
        Cmd("jarvis notify", "печатать события по мере появления"),
        Gap,
        Head("Общие флаги"),
        Cmd("-m, --machine <имя>", "где работать: local или имя узла"),
        Cmd("    --json", "машинный вывод вместо человеческого"),
        Cmd("-h, --help", "эта справка"),
        Cmd("-V, --version", "версия"),
        Gap,
        Note(
            "Сессию можно называть началом её идентификатора или именем проекта — \
             достаточно, чтобы совпадение было одно.",
        ),
    ]
}

/// Справка целиком — как её видит человек.
///
/// Описания выровнены по колонке, команды выделены, примечания приглушены и
/// перенесены по ширине окна. Раньше это была строка с пробелами руками.
pub fn help(caps: &crate::ui::style::Caps) -> String {
    use crate::ui::style::{pad, paint, wrap, Role};
    let total = (caps.width as usize).max(40);
    let col = help_rows()
        .iter()
        .filter_map(|r| match r {
            Help::Cmd(name, _) => Some(crate::ui::style::width(name)),
            _ => None,
        })
        .max()
        .unwrap_or(28)
        .min(total.saturating_sub(24));
    let mut out = format!(
        "{} {}\n",
        paint(caps, Role::Accent, "jarvis"),
        paint(
            caps,
            Role::Dim,
            "— агенты, циклы и связка прямо в терминале"
        )
    );
    for row in help_rows() {
        match row {
            Help::Gap => out.push('\n'),
            Help::Head(t) => out.push_str(&format!("{}\n", paint(caps, Role::Text, t))),
            Help::Cmd(name, what) => {
                let room = total.saturating_sub(col + 4);
                if room < 24 {
                    // Узкое окно: описание уходит под команду. Обрезанное до
                    // «машинный вывод вмес…» описание не описывает ничего.
                    out.push_str(&format!("  {}\n", paint(caps, Role::Accent, name)));
                    for line in wrap(what, total.saturating_sub(6)) {
                        out.push_str(&format!("      {}\n", paint(caps, Role::Dim, &line)));
                    }
                } else {
                    out.push_str(&format!(
                        "  {}  {}\n",
                        pad(&paint(caps, Role::Accent, name), col),
                        paint(caps, Role::Dim, &crate::ui::style::truncate(what, room))
                    ));
                }
            }
            Help::Note(t) => {
                for line in wrap(t, total.saturating_sub(2)) {
                    out.push_str(&format!("{}\n", paint(caps, Role::Dim, &line)));
                }
            }
        }
    }
    out
}

/// Разобрать аргументы (без имени программы).
pub fn parse(args: &[String]) -> Result<Parsed, String> {
    let mut machine = "local".to_string();
    let mut json = false;
    let mut rest: Vec<String> = Vec::new();

    let mut it = args.iter().cloned();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-m" | "--machine" => {
                machine = it.next().ok_or("после --machine нужно имя машины")?;
            }
            "--json" => json = true,
            "-h" | "--help" => {
                return Ok(Parsed {
                    cmd: Cmd::Help,
                    machine,
                    json,
                })
            }
            "-V" | "--version" => {
                return Ok(Parsed {
                    cmd: Cmd::Version,
                    machine,
                    json,
                })
            }
            // Флаг после «--» — уже часть текста ответа, а не наш флаг.
            "--" => {
                rest.extend(it.by_ref());
                break;
            }
            _ => rest.push(a),
        }
    }

    let cmd = parse_cmd(&rest)?;
    Ok(Parsed { cmd, machine, json })
}

fn parse_cmd(rest: &[String]) -> Result<Cmd, String> {
    let head = rest.first().map(String::as_str).unwrap_or("");
    let arg = |i: usize| rest.get(i).cloned();
    match head {
        "" | "watch" => Ok(Cmd::Watch),
        "ls" | "list" => Ok(Cmd::Ls),
        "help" => Ok(Cmd::Help),
        "version" => Ok(Cmd::Version),
        "reply" => {
            let session = arg(1).ok_or("кому отвечаем? jarvis reply <сессия> <текст>")?;
            let text = rest[2..].join(" ");
            if text.trim().is_empty() {
                return Err("пустой ответ агенту не поможет".into());
            }
            Ok(Cmd::Reply { session, text })
        }
        "answer" => {
            let session = arg(1).ok_or("кому отвечаем? jarvis answer <сессия> <номер>")?;
            let n: u8 = arg(2)
                .ok_or("какой вариант? jarvis answer <сессия> <номер>")?
                .parse()
                .map_err(|_| "номер варианта — число от 1 до 9".to_string())?;
            if !(1..=9).contains(&n) {
                return Err("номер варианта — число от 1 до 9".into());
            }
            Ok(Cmd::Answer { session, option: n })
        }
        "stop" => Ok(Cmd::Stop {
            session: arg(1).ok_or("кого прерываем? jarvis stop <сессия>")?,
        }),
        "chat" => Ok(Cmd::Chat {
            session: arg(1).ok_or("чей чат? jarvis chat <сессия> [-f]")?,
            follow: rest.iter().any(|a| a == "-f" || a == "--follow"),
        }),
        "projects" | "proj" => Ok(Cmd::Projects),
        "machines" | "машины" => Ok(Cmd::Machines),
        "machine" => {
            let sub = rest.get(1).map(String::as_str).unwrap_or("");
            match sub {
                "add" => {
                    let name =
                        arg(2).ok_or("как назвать машину? jarvis machine add <имя> <ssh-хост>")?;
                    let host = arg(3).ok_or("куда ходить? jarvis machine add <имя> user@host")?;
                    let dir = rest
                        .iter()
                        .position(|a| a == "--dir")
                        .and_then(|i| rest.get(i + 1).cloned())
                        .unwrap_or_else(|| "~/.jarvis".into());
                    Ok(Cmd::MachineAdd { name, host, dir })
                }
                "rm" | "remove" | "delete" => Ok(Cmd::MachineRm {
                    name: arg(2).ok_or("какую машину убрать? jarvis machine rm <имя>")?,
                }),
                _ => Err("есть machine add и machine rm; список — jarvis machines".into()),
            }
        }
        "cmd" | "slash" => {
            let session = arg(1).ok_or("кому команду? jarvis cmd <сессия> /model opus")?;
            let cmd = rest[2..].join(" ");
            if !cmd.trim_start().starts_with('/') {
                return Err("слэш-команда начинается с /: jarvis cmd <сессия> /model opus".into());
            }
            Ok(Cmd::Control { session, cmd })
        }
        "screen" => Ok(Cmd::Screen {
            session: arg(1).ok_or("чей экран? jarvis screen <сессия>")?,
        }),
        "run" => {
            let dir = arg(1).ok_or("где запускать? jarvis run <каталог>")?;
            // `--agent codex` ищем в хвосте: он относится к команде, а не ко
            // всему вызову, и в общих флагах ему делать нечего.
            let agent = rest
                .iter()
                .position(|a| a == "--agent")
                .and_then(|i| rest.get(i + 1).cloned())
                .unwrap_or_else(|| "claude".into());
            Ok(Cmd::Run { dir, agent })
        }
        "limits" => Ok(Cmd::Limits {
            fresh: rest.iter().any(|a| a == "--fresh"),
        }),
        "loop" | "loops" => parse_loop(rest),
        "bundle" | "bundles" | "svyazka" => parse_bundle(rest),
        "notify" => Ok(Cmd::Notify {
            once: rest.iter().any(|a| a == "--once"),
        }),
        other => Err(format!(
            "не знаю команду «{other}». jarvis --help покажет, что умею"
        )),
    }
}

fn parse_loop(rest: &[String]) -> Result<Cmd, String> {
    let sub = rest.get(1).map(String::as_str).unwrap_or("ls");
    let id = || {
        rest.get(2)
            .cloned()
            .ok_or_else(|| "какой цикл? jarvis loop <действие> <id>".to_string())
    };
    let action = match sub {
        "ls" | "list" => LoopAction::Ls,
        "new" | "создать" => LoopAction::New,
        "presets" | "заготовки" => LoopAction::Presets,
        "rm" | "remove" | "delete" => LoopAction::Rm { id: id()? },
        "say" | "answer" | "ответ" => {
            let id = id()?;
            let text = rest[3..].join(" ");
            if text.trim().is_empty() {
                return Err("что сказать циклу? jarvis loop say <цикл> <текст>".into());
            }
            LoopAction::Say { id, text }
        }
        "show" => LoopAction::Show { id: id()? },
        "start" | "run" => LoopAction::Start { id: id()? },
        "stop" => LoopAction::Stop { id: id()? },
        other => {
            return Err(format!(
                "не знаю «loop {other}»: есть new, ls, show, start, stop, say, presets, rm"
            ))
        }
    };
    Ok(Cmd::Loop { action })
}

fn parse_bundle(rest: &[String]) -> Result<Cmd, String> {
    let sub = rest.get(1).map(String::as_str).unwrap_or("ls");
    let id = || {
        rest.get(2)
            .cloned()
            .ok_or_else(|| "какая связка? jarvis bundle <действие> <id>".to_string())
    };
    let action = match sub {
        "ls" | "list" => BundleAction::Ls,
        "new" | "создать" => BundleAction::New,
        "hand" | "рука" => {
            let id = id()?;
            let task = rest[3..].join(" ");
            if task.trim().is_empty() {
                return Err("чем займётся рука? jarvis bundle hand <связка> <задача>".into());
            }
            BundleAction::Hand { id, task }
        }
        "show" => BundleAction::Show { id: id()? },
        "merge" => BundleAction::Merge {
            id: id()?,
            hand: rest
                .get(3)
                .cloned()
                .ok_or("какую руку вливаем? jarvis bundle merge <id> <рука>")?,
        },
        "rm" | "remove" | "delete" => BundleAction::Rm {
            id: id()?,
            // Живые руки просто так не выбрасываем, а worktree не убираем без
            // просьбы: и то и другое — чужая работа.
            force: rest.iter().any(|a| a == "--force"),
            clean: rest.iter().any(|a| a == "--clean"),
        },
        "pause" => BundleAction::Pause {
            id: id()?,
            on: !rest.iter().any(|a| a == "--off"),
        },
        other => {
            return Err(format!(
                "не знаю «bundle {other}»: есть new, ls, show, hand, merge, pause, rm"
            ))
        }
    };
    Ok(Cmd::Bundle { action })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Parsed, String> {
        parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn bare_call_opens_the_live_screen() {
        let r = p(&[]).unwrap();
        assert_eq!(r.cmd, Cmd::Watch);
        assert_eq!(r.machine, "local");
        assert!(!r.json);
    }

    #[test]
    fn reply_keeps_the_whole_text() {
        let r = p(&["reply", "abc", "давай", "по-другому"]).unwrap();
        assert_eq!(
            r.cmd,
            Cmd::Reply {
                session: "abc".into(),
                text: "давай по-другому".into()
            }
        );
    }

    /// Текст ответа может начинаться с дефиса — «--» отделяет его от флагов.
    #[test]
    fn double_dash_protects_the_message() {
        let r = p(&["reply", "abc", "--", "--json", "это", "текст"]).unwrap();
        assert_eq!(
            r.cmd,
            Cmd::Reply {
                session: "abc".into(),
                text: "--json это текст".into()
            }
        );
        assert!(!r.json, "флаг после -- принадлежит сообщению, а не нам");
    }

    #[test]
    fn machine_flag_works_before_and_after_command() {
        assert_eq!(p(&["-m", "vps", "ls"]).unwrap().machine, "vps");
        assert_eq!(p(&["ls", "--machine", "vps"]).unwrap().machine, "vps");
        assert!(p(&["--machine"]).is_err(), "имя машины обязательно");
    }

    #[test]
    fn answer_validates_the_option_number() {
        assert_eq!(
            p(&["answer", "s", "2"]).unwrap().cmd,
            Cmd::Answer {
                session: "s".into(),
                option: 2
            }
        );
        assert!(p(&["answer", "s", "0"]).is_err());
        assert!(p(&["answer", "s", "12"]).is_err());
        assert!(p(&["answer", "s", "два"]).is_err());
    }

    #[test]
    fn empty_reply_is_refused_with_a_reason() {
        let err = p(&["reply", "abc"]).unwrap_err();
        assert!(err.contains("пустой ответ"), "{err}");
    }

    #[test]
    fn loop_and_bundle_subcommands() {
        assert_eq!(
            p(&["loop"]).unwrap().cmd,
            Cmd::Loop {
                action: LoopAction::Ls
            }
        );
        assert_eq!(
            p(&["loop", "show", "l1"]).unwrap().cmd,
            Cmd::Loop {
                action: LoopAction::Show { id: "l1".into() }
            }
        );
        assert_eq!(
            p(&["bundle", "merge", "b1", "h2"]).unwrap().cmd,
            Cmd::Bundle {
                action: BundleAction::Merge {
                    id: "b1".into(),
                    hand: "h2".into()
                }
            }
        );
        assert_eq!(
            p(&["bundle", "pause", "b1", "--off"]).unwrap().cmd,
            Cmd::Bundle {
                action: BundleAction::Pause {
                    id: "b1".into(),
                    on: false
                }
            }
        );
        assert!(p(&["loop", "летать"]).is_err());
    }

    #[test]
    fn run_takes_the_agent_from_its_own_flag() {
        assert_eq!(
            p(&["run", "/tmp/proj", "--agent", "codex"]).unwrap().cmd,
            Cmd::Run {
                dir: "/tmp/proj".into(),
                agent: "codex".into()
            }
        );
        assert_eq!(
            p(&["run", "/tmp/proj"]).unwrap().cmd,
            Cmd::Run {
                dir: "/tmp/proj".into(),
                agent: "claude".into()
            }
        );
    }

    /// Незнакомая команда обязана предлагать выход, а не просто ругаться.
    #[test]
    fn unknown_command_points_at_help() {
        let err = p(&["полетели"]).unwrap_err();
        assert!(err.contains("--help"), "{err}");
    }

    #[test]
    fn help_mentions_every_command() {
        let text = help(&crate::ui::style::Caps {
            color: false,
            theme: crate::ui::style::Theme::Dark,
            truecolor: false,
            unicode: true,
            width: 100,
        });
        for word in [
            "reply", "answer", "loop", "bundle", "limits", "notify", "screen", "chat", "projects",
        ] {
            assert!(text.contains(word), "справка молчит про {word}");
        }
        // Описания стоят одной колонкой: раньше три длинные команды сбивали
        // выравнивание, и справка читалась как черновик.
        let starts: Vec<usize> = text
            .lines()
            .filter(|l| l.starts_with("  jarvis "))
            .filter_map(|l| {
                // Считаем в ЯЧЕЙКАХ, а не в байтах: «<текст>» кириллицей
                // сдвигает байтовое смещение, и ровная колонка выглядела бы
                // кривой.
                let body = &l[2..];
                let gap = body.find("  ")?;
                let spaces = body[gap..].chars().take_while(|c| *c == ' ').count();
                Some(2 + crate::ui::style::width(&body[..gap]) + spaces)
            })
            .collect();
        let first = starts.first().copied().unwrap_or(0);
        assert!(
            starts.len() > 20 && starts.iter().all(|s| *s == first),
            "описания встали вкось: {starts:?}"
        );
    }

    #[test]
    fn machines_are_managed_from_here_too() {
        assert_eq!(p(&["machines"]).unwrap().cmd, Cmd::Machines);
        assert_eq!(
            p(&["machine", "add", "vps", "me@vps", "--dir", "/srv/jarvis"])
                .unwrap()
                .cmd,
            Cmd::MachineAdd {
                name: "vps".into(),
                host: "me@vps".into(),
                dir: "/srv/jarvis".into()
            }
        );
        // Без каталога — общее умолчание узла, а не пустая строка.
        assert_eq!(
            p(&["machine", "add", "vps", "me@vps"]).unwrap().cmd,
            Cmd::MachineAdd {
                name: "vps".into(),
                host: "me@vps".into(),
                dir: "~/.jarvis".into()
            }
        );
        assert!(p(&["machine", "add", "vps"]).is_err());
    }

    /// Вопрос цикла без способа ответить — тупик: подсказка обещает команду,
    /// которая обязана существовать.
    #[test]
    fn loop_can_be_answered() {
        assert_eq!(
            p(&["loop", "say", "l1", "да,", "снимай"]).unwrap().cmd,
            Cmd::Loop {
                action: LoopAction::Say {
                    id: "l1".into(),
                    text: "да, снимай".into()
                }
            }
        );
        assert!(p(&["loop", "say", "l1"]).is_err());
    }

    #[test]
    fn builders_have_their_own_words() {
        assert_eq!(
            p(&["loop", "new"]).unwrap().cmd,
            Cmd::Loop {
                action: LoopAction::New
            }
        );
        assert_eq!(
            p(&["bundle", "new"]).unwrap().cmd,
            Cmd::Bundle {
                action: BundleAction::New
            }
        );
        assert_eq!(
            p(&["bundle", "hand", "b1", "починить", "флаки"])
                .unwrap()
                .cmd,
            Cmd::Bundle {
                action: BundleAction::Hand {
                    id: "b1".into(),
                    task: "починить флаки".into()
                }
            }
        );
        // Рука без задачи — пустая сессия, которой никто не скажет, что делать.
        assert!(p(&["bundle", "hand", "b1"]).is_err());
    }

    #[test]
    fn control_requires_a_slash() {
        assert_eq!(
            p(&["cmd", "s", "/model", "opus"]).unwrap().cmd,
            Cmd::Control {
                session: "s".into(),
                cmd: "/model opus".into()
            }
        );
        // Без слэша это не команда пульта, а обычный текст — и уйдёт он не туда.
        assert!(p(&["cmd", "s", "model"]).is_err());
    }

    #[test]
    fn chat_takes_the_follow_flag() {
        assert_eq!(
            p(&["chat", "abc", "-f"]).unwrap().cmd,
            Cmd::Chat {
                session: "abc".into(),
                follow: true
            }
        );
        assert_eq!(
            p(&["chat", "abc"]).unwrap().cmd,
            Cmd::Chat {
                session: "abc".into(),
                follow: false
            }
        );
        assert!(p(&["chat"]).is_err());
    }
}
