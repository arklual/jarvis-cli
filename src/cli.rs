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
    Show { id: String },
    Start { id: String },
    Stop { id: String },
}

#[derive(Debug, Clone, PartialEq)]
pub enum BundleAction {
    Ls,
    Show { id: String },
    Merge { id: String, hand: String },
    Pause { id: String, on: bool },
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

pub const HELP: &str = "\
jarvis — агенты, циклы и связка прямо в терминале

  jarvis                     живой экран: кто работает, кто спрашивает
  jarvis ls                  список сессий разово
  jarvis reply <id> <текст>  ответить агенту
  jarvis answer <id> <n>     ответить на вопрос вариантом n
  jarvis stop <id>           прервать агента
  jarvis screen <id>         показать экран сессии
  jarvis chat <id> [-f]      лента чата; -f — следить вживую
  jarvis projects            проекты машины
  jarvis cmd <id> /model …   слэш-команда пульта в сессию
  jarvis run <каталог>       поднять агента в каталоге
  jarvis limits              лимиты аккаунта

  jarvis loop ls             циклы: рутина, которую агент крутит сам
  jarvis loop show <id>      журнал итераций цикла
  jarvis loop start <id>     запустить цикл
  jarvis loop stop <id>      остановить цикл

  jarvis bundle ls           связки: несколько агентов над одним проектом
  jarvis bundle show <id>    пульт связки: руки, очередь слияний
  jarvis bundle merge <id> <рука>   влить голову очереди
  jarvis bundle pause <id>   пауза всем рукам

  jarvis notify              печатать события по мере появления

Общие флаги:
  -m, --machine <имя>        где работать: local или имя узла (по умолчанию local)
      --json                 машинный вывод вместо человеческого
  -h, --help                 эта справка
  -V, --version              версия

Сессию можно называть началом её идентификатора или именем проекта —
достаточно, чтобы совпадение было одно.";

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
        "show" => LoopAction::Show { id: id()? },
        "start" | "run" => LoopAction::Start { id: id()? },
        "stop" => LoopAction::Stop { id: id()? },
        other => {
            return Err(format!(
                "не знаю «loop {other}»: есть ls, show, start, stop"
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
        "show" => BundleAction::Show { id: id()? },
        "merge" => BundleAction::Merge {
            id: id()?,
            hand: rest
                .get(3)
                .cloned()
                .ok_or("какую руку вливаем? jarvis bundle merge <id> <рука>")?,
        },
        "pause" => BundleAction::Pause {
            id: id()?,
            on: !rest.iter().any(|a| a == "--off"),
        },
        other => {
            return Err(format!(
                "не знаю «bundle {other}»: есть ls, show, merge, pause"
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
        for word in [
            "reply", "answer", "loop", "bundle", "limits", "notify", "screen", "chat", "projects",
        ] {
            assert!(HELP.contains(word), "справка молчит про {word}");
        }
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
