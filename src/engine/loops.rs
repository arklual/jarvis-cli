//! Движок цикла в терминале: итерация за итерацией, вживую на глазах.
//!
//! Тот же договор, что у настольного: у цикла есть КОНЕЦ (условие выхода) и
//! СТЕНЫ (ограничители), а ограничитель проверяется ПЕРЕД итерацией — смысл
//! стены в том, чтобы не начинать работу, на которую нет бюджета.
//!
//! Отличие терминала от панели одно, и оно в его пользу: человек видит ход
//! прямо в потоке вывода, а не в журнале постфактум.

use crate::core::machine::{self, Machine};
use crate::core::state::{self, GateRun, Iteration, Loop, Run, RunState, StopReason, Verdict};
use crate::core::util::{ellipsize, now_ms, one_line, shell_quote};
use std::time::Duration;

const ITERATION: Duration = Duration::from_secs(3600);
const CRITIC: Duration = Duration::from_secs(600);
const GATE: Duration = Duration::from_secs(1800);
const DIFF_FOR_CRITIC: usize = 60_000;

/// Что цикл рассказывает о себе по ходу.
///
/// Движок ничего не печатает: у него два зрителя — команда, которая пишет ход
/// в поток, и живое окно, где печать мимо кадра порвала бы экран. Раньше
/// печать была зашита внутрь, и потому цикл нельзя было запустить из окна.
#[derive(Debug, Clone)]
pub enum Note {
    /// Начало прогона: где и с каким условием выхода.
    Head {
        name: String,
        branch: String,
        dir: String,
        streak: u32,
    },
    /// Итерация началась.
    Started(u32),
    /// Итерация закончилась.
    Done(Iteration),
    /// Критик спрашивает человека — цикл встал.
    Ask { name: String, question: String },
    /// Прогон завершён.
    Finished {
        reason: StopReason,
        iterations: usize,
        tokens: u64,
        pending: usize,
    },
}

/// Что вернул headless-вызов агента.
#[derive(Debug, Default, Clone)]
pub struct AgentOut {
    pub text: String,
    pub tokens: u64,
    pub cost_usd: f64,
    pub failed: bool,
}

/// Разбор `--output-format json`. Формат внутренний — читаем defensive: нет
/// расхода, значит ноль, а не отказ от итерации.
pub fn parse_agent_json(stdout: &str) -> AgentOut {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(stdout.trim()) else {
        // Не JSON — агент печатал обычным текстом. Работа сделана, расход
        // просто неизвестен.
        return AgentOut {
            text: stdout.trim().to_string(),
            ..Default::default()
        };
    };
    let field = |name: &str| -> u64 {
        v.get("usage")
            .and_then(|u| u.get(name))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
    };
    AgentOut {
        text: v
            .get("result")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        // Кэш считаем наравне: лимит аккаунта расходуется и им.
        tokens: field("input_tokens")
            + field("output_tokens")
            + field("cache_creation_input_tokens")
            + field("cache_read_input_tokens"),
        cost_usd: v
            .get("total_cost_usd")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0),
        failed: v
            .get("is_error")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
    }
}

/// Вердикт критика.
#[derive(Debug, PartialEq)]
pub enum CriticSays {
    Fine,
    Return(String),
    Ask(String),
}

/// Читаем строго с первой строки — договор напечатан в самом промте.
/// Всё непонятое — возврат: «выглядит нормально, но тесты снял» не имеет права
/// пройти за одобрение из-за слова «нормально».
pub fn parse_critic(text: &str) -> CriticSays {
    let body = text.trim();
    let first = body.lines().next().unwrap_or("").trim().to_uppercase();
    let rest = body
        .lines()
        .skip(1)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();
    let payload = if rest.is_empty() {
        body.to_string()
    } else {
        rest
    };
    if first.starts_with("OK") {
        CriticSays::Fine
    } else if first.starts_with("ASK") {
        CriticSays::Ask(payload)
    } else {
        CriticSays::Return(payload)
    }
}

/// Первая непустая строка ответа — сводка итерации.
pub fn summarize(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("без ответа");
    ellipsize(&one_line(line), 160)
}

pub fn is_sampled(every: u32, n: u32) -> bool {
    every > 0 && n.is_multiple_of(every)
}

/// Промт итерации: цель, задачи, дневник и реплики человека.
pub fn iteration_prompt(
    l: &Loop,
    run: &Run,
    tasks: &str,
    notes: &str,
    last_return: &str,
) -> String {
    let mut p = format!(
        "Ты — итерация {} автономного цикла «{}».\n\nЦель цикла: {}\n\n",
        run.iterations.len() + 1,
        l.name,
        l.source.goal
    );
    if !tasks.trim().is_empty() {
        p.push_str(&format!("Задачи из источника:\n{}\n\n", tasks.trim()));
    }
    if !notes.trim().is_empty() {
        // Дневник — против дня сурка: без него агент второй раз наступает на
        // те же грабли и второй раз их описывает.
        p.push_str(&format!(
            "Дневник цикла — решения, грабли и что не делать. Прочитай прежде, чем начинать:\n{}\n\n",
            notes.trim()
        ));
    }
    if !last_return.trim().is_empty() {
        p.push_str(&format!(
            "Прошлую итерацию вернули на доработку: {}\n\n",
            last_return.trim()
        ));
    }
    if !run.interventions.is_empty() {
        p.push_str(&format!(
            "Человек вмешался в цикл — это важнее всего остального:\n{}\n\n",
            run.interventions.join("\n")
        ));
    }
    if !l.exit.gates.is_empty() {
        let names: Vec<&str> = l.exit.gates.iter().map(|g| g.name.as_str()).collect();
        p.push_str(&format!(
            "Работа считается сделанной, когда проходят гейты: {}. Их прогонят после тебя.\n\n",
            names.join(", ")
        ));
    }
    if l.memory.enabled {
        p.push_str(&format!(
            "Допиши в {} то, что стоит помнить следующей итерации: решения, грабли, что НЕ делать.\n\n",
            l.memory.file
        ));
    }
    p.push_str("Сделай ОДИН шаг к цели и остановись. Первой строкой ответа — что именно ты сделал, одним предложением.");
    p
}

fn critic_prompt(l: &Loop, summary: &str, diff: &str) -> String {
    let own = l.exit.critic.prompt.trim();
    let head = if own.is_empty() {
        "Ты ревьюишь работу автономного цикла. Смотри по существу: сделано ли то, что просили, \
         нет ли обхода проблемы вместо решения (снятые тесты, заглушки, ослабленные проверки)."
    } else {
        own
    };
    format!(
        "{head}\n\nЦель цикла: {goal}\n\nЧто сделала итерация: {summary}\n\n\
         Ответь РОВНО в таком виде. Первая строка — вердикт одним словом:\n\
         OK — работу можно принять\nRETURN — вернуть на доработку\nASK — решение спорное, нужен человек\n\
         Со второй строки — причина, коротко и по делу.\n\nДифф:\n{diff}",
        goal = l.source.goal,
    )
}

/* ---------- исполнение ---------- */

/// Команда цикла — всегда через машину, даже локальную.
///
/// Один путь на оба случая: то, что цикл крутится на сервере, не должно быть
/// отдельным режимом с отдельными ошибками. `JARVIS_IGNORE` объявляем внутри
/// команды, а не переменной окружения процесса: по ssh окружение не уедет.
async fn sh(m: &Machine, cwd: &str, cmd: &str, timeout: Duration) -> (i32, String) {
    machine::run(m, cwd, &ignored(cmd), timeout).await
}

/// Пометить команду как «не считать работой агента».
pub fn ignored(cmd: &str) -> String {
    format!("export JARVIS_IGNORE=1\n{cmd}")
}

pub fn tail(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

/// Команда headless-вызова агента.
///
/// Отдельной функцией, потому что она уезжает на чужую машину строкой: там нет
/// ни нашего окружения, ни нашей кавычки. Stderr гасим прямо в команде —
/// удалённый запуск склеивает потоки, а к JSON агента чужая строка не
/// прилипнет только так.
pub fn agent_cmd(prompt: &str, model: Option<&str>) -> String {
    let mut cmd = format!(
        "claude -p {} --output-format json --dangerously-skip-permissions",
        shell_quote(prompt)
    );
    if let Some(m) = model {
        cmd.push_str(&format!(" --model {}", shell_quote(m)));
    }
    // Цикл работает в песочнице и без человека: спрашивать разрешения не у
    // кого, а остановка на вопросе означала бы запуск, висящий до утра.
    cmd.push_str(" 2>/dev/null");
    cmd
}

async fn run_agent(
    m: &Machine,
    cwd: &str,
    prompt: &str,
    model: Option<&str>,
    timeout: Duration,
) -> AgentOut {
    let (code, out) = sh(m, cwd, &agent_cmd(prompt, model), timeout).await;
    if code == -1 {
        return AgentOut {
            failed: true,
            text: "агент не уложился в отведённое время".into(),
            ..Default::default()
        };
    }
    if code != 0 {
        return AgentOut {
            failed: true,
            text: "агент завершился с ошибкой".into(),
            ..Default::default()
        };
    }
    parse_agent_json(&out)
}

async fn run_gates(m: &Machine, gates: &[state::Gate], cwd: &str) -> Vec<GateRun> {
    let mut out = Vec::new();
    for g in gates {
        let (code, text) = sh(m, cwd, &g.command, GATE).await;
        let ok = code == 0;
        out.push(GateRun {
            name: g.name.clone(),
            ok,
            output: tail(&text, 40),
        });
        if !ok {
            break; // после красного гонять остальные нечего
        }
    }
    out
}

/// Поднять песочницу: отдельный worktree на своей ветке.
async fn make_sandbox(m: &Machine, l: &Loop, run_n: u32) -> Result<(String, String), String> {
    let repo = l.sandbox.repo.trim().to_string();
    // Про чужую машину «есть ли .git» знает только она сама.
    let (code, why) = sh(
        m,
        "/",
        &format!("test -d {}/.git", shell_quote(&repo)),
        Duration::from_secs(20),
    )
    .await;
    if code != 0 {
        // `test -d` молчит, поэтому любой вывод здесь — жалоба транспорта.
        // Без этого различия человек читает «не репозиторий git» и идёт
        // проверять путь, хотя ssh просто не пустил.
        let why = one_line(why.trim());
        return Err(if why.is_empty() {
            format!("{repo} — не репозиторий git (на {})", m.name)
        } else {
            format!("не добрался до «{}»: {why}", m.name)
        });
    }
    let branch = l
        .sandbox
        .branch
        .replace("{name}", &slug(&l.name))
        .replace("{n}", &run_n.to_string());
    if !l.sandbox.worktree {
        return Ok((repo, branch));
    }
    // Песочница живёт в каталоге данных ТОЙ машины, где крутится цикл.
    let root = machine::data_dir(m).await?;
    let dir = format!("{root}/worktrees/{}-{run_n}", slug(&l.name));
    let (exists, _) = sh(
        m,
        "/",
        &format!("test -d {}", shell_quote(&dir)),
        Duration::from_secs(20),
    )
    .await;
    if exists == 0 {
        return Ok((dir, branch)); // продолжаем прежний запуск, а не падаем
    }
    let cmd = format!(
        "git worktree add -b {} {} HEAD",
        shell_quote(&branch),
        shell_quote(&dir)
    );
    let (code, out) = sh(m, &repo, &cmd, Duration::from_secs(120)).await;
    if code != 0 {
        return Err(format!("git worktree: {}", tail(&out, 4)));
    }
    Ok((dir, branch))
}

/// Имя цикла в вид, пригодный для ветки git.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for ch in name.chars() {
        if ch.is_alphanumeric() && ch.is_ascii() {
            out.push(ch.to_ascii_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    let t = out.trim_matches('-').to_string();
    if t.is_empty() {
        "loop".into()
    } else {
        t
    }
}

/// Прогнать цикл, показывая ход прямо в терминале.
pub async fn start(l: &Loop, note: &mut (dyn FnMut(Note) + Send)) -> Result<(), String> {
    // Машину берём по имени из самого цикла: где заведён, там и крутится.
    let m = machine::find(&l.machine)?;
    let run_n = state::load_run(&l.id).map(|r| r.n + 1).unwrap_or(1);
    let (dir, branch) = make_sandbox(&m, l, run_n).await?;

    note(Note::Head {
        name: l.name.clone(),
        branch: branch.clone(),
        dir: dir.clone(),
        streak: l.exit.streak,
    });

    let mut run = Run {
        loop_id: l.id.clone(),
        n: run_n,
        state: RunState::Running,
        started_at: now_ms(),
        branch,
        worktree: dir.clone(),
        ..Default::default()
    };
    let _ = state::save_run(&run);

    let mut last_return = String::new();
    loop {
        let now = now_ms();
        if let Some(reason) = run.tripped(&l.limits, now) {
            return finish(&mut run, reason, note);
        }
        // Остановку спрашиваем у файла: её мог поставить кто угодно — окно,
        // другой терминал, панель. Раньше «стоп» только помечал журнал, а
        // живой прогон его не замечал и работал дальше.
        if state::load_run(&l.id)
            .is_some_and(|r| r.n == run.n && matches!(r.state, RunState::Stopped))
        {
            return finish(&mut run, StopReason::Stopped, note);
        }
        let n = run.iterations.len() as u32 + 1;
        let mut it = Iteration {
            n,
            started_at: now,
            verdict: Verdict::Running,
            ..Default::default()
        };

        let tasks = if l.source.command.trim().is_empty() {
            String::new()
        } else {
            let (_, out) = sh(&m, &dir, &l.source.command, Duration::from_secs(300)).await;
            tail(&out, 40)
        };
        let notes = if l.memory.enabled {
            std::fs::read_to_string(std::path::Path::new(&dir).join(&l.memory.file))
                .map(|t| tail(&t, 120))
                .unwrap_or_default()
        } else {
            String::new()
        };

        note(Note::Started(n));
        let out = run_agent(
            &m,
            &dir,
            &iteration_prompt(l, &run, &tasks, &notes, &last_return),
            None,
            ITERATION,
        )
        .await;
        it.tokens = out.tokens;
        it.cost_usd = out.cost_usd;
        it.summary = summarize(&out.text);
        run.tokens += out.tokens;
        run.cost_usd += out.cost_usd;
        if out.failed {
            it.verdict = Verdict::Failed;
            it.ended_at = now_ms();
            run.iterations.push(it);
            run.stop_note = out.text;
            return finish(&mut run, StopReason::Failed, note);
        }
        run.interventions.clear();

        it.gates = run_gates(&m, &l.exit.gates, &dir).await;
        if !it.gates.iter().all(|g| g.ok) {
            it.verdict = Verdict::GateFailed;
            last_return = it
                .gates
                .iter()
                .find(|g| !g.ok)
                .map(|g| format!("красный гейт «{}»:\n{}", g.name, tail(&g.output, 20)))
                .unwrap_or_default();
            run.streak = 0;
        } else if l.exit.critic.enabled {
            let (_, diff) = sh(
                &m,
                &dir,
                "git --no-pager diff HEAD",
                Duration::from_secs(60),
            )
            .await;
            let verdict = run_agent(
                &m,
                &dir,
                &critic_prompt(l, &it.summary, &ellipsize(&diff, DIFF_FOR_CRITIC)),
                Some(&l.exit.critic.model)
                    .filter(|m| !m.is_empty())
                    .map(|s| s.as_str()),
                CRITIC,
            )
            .await;
            run.tokens += verdict.tokens;
            run.cost_usd += verdict.cost_usd;
            match parse_critic(&verdict.text) {
                CriticSays::Fine => {
                    it.verdict = Verdict::Passed;
                    run.streak += 1;
                }
                CriticSays::Return(why) => {
                    it.verdict = Verdict::Returned;
                    it.critic = why.clone();
                    last_return = why;
                    run.streak = 0;
                }
                CriticSays::Ask(what) => {
                    // human by exception: спорное решение уходит человеку, а
                    // цикл встаёт. Продолжать «на своё усмотрение» — ровно то,
                    // из-за чего к автономности теряют доверие.
                    it.verdict = Verdict::Returned;
                    it.critic = what.clone();
                    it.ended_at = now_ms();
                    run.iterations.push(it);
                    run.state = RunState::Asking;
                    run.ask = Some(state::Ask {
                        at: now_ms(),
                        question: what.clone(),
                        options: Vec::new(),
                        iteration: n,
                    });
                    let _ = state::save_run(&run);
                    note(Note::Ask {
                        name: l.name.clone(),
                        question: what,
                    });
                    return Ok(());
                }
            }
        } else {
            it.verdict = Verdict::Passed;
            run.streak += 1;
        }

        it.sampled = is_sampled(l.sampling.every, n);
        it.ended_at = now_ms();
        note(Note::Done(it.clone()));
        run.iterations.push(it);
        let _ = state::save_run(&run);

        if run.streak >= l.exit.streak.max(1) {
            return finish(&mut run, StopReason::Exit, note);
        }
    }
}

fn finish(
    run: &mut Run,
    reason: StopReason,
    note: &mut (dyn FnMut(Note) + Send),
) -> Result<(), String> {
    run.state = if reason == StopReason::Exit {
        RunState::Done
    } else {
        RunState::Stopped
    };
    run.stop = reason;
    run.ended_at = now_ms();
    let _ = state::save_run(run);
    note(Note::Finished {
        reason,
        iterations: run.iterations.len(),
        tokens: run.tokens,
        pending: run.pending_review(),
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Команда агента уезжает на чужую машину СТРОКОЙ: там нет ни нашего
    /// окружения, ни нашей кавычки. Промт с кавычками и переводами строк
    /// обязан доехать целым.
    #[test]
    fn agent_command_survives_a_hostile_prompt() {
        let prompt = "сделай так:\n  echo 'привет' && ls $HOME `date`";
        let cmd = agent_cmd(prompt, Some("opus"));
        assert!(cmd.starts_with("claude -p '"));
        assert!(cmd.contains("--output-format json"));
        assert!(cmd.contains("--dangerously-skip-permissions"));
        assert!(cmd.contains("--model 'opus'"));
        // Stderr гасим прямо в команде: удалённый запуск склеивает потоки, и
        // чужая строка прилипла бы к JSON агента.
        assert!(cmd.ends_with("2>/dev/null"));
        // Ни одна опасная часть промта не осталась вне кавычек.
        for danger in ["&&", "$HOME", "`date`"] {
            let at = cmd.find(danger).expect(danger);
            let quoted = cmd[..at].matches('\'').count() % 2 == 1;
            assert!(quoted, "«{danger}» вылезло из кавычек: {cmd}");
        }
    }

    /// Пометка «не считать это работой агента» должна пережить составную
    /// команду: `JARVIS_IGNORE=1 a; b` пометил бы только `a`.
    #[test]
    fn ignore_mark_covers_the_whole_command() {
        let c = ignored("make test; make lint");
        assert!(c.starts_with("export JARVIS_IGNORE=1\n"));
        assert!(c.ends_with("make test; make lint"));
    }

    #[test]
    fn agent_json_gives_text_and_real_spend() {
        let out = parse_agent_json(
            r#"{"result":"Починил флаки-тест","total_cost_usd":0.42,
                "usage":{"input_tokens":100,"output_tokens":20,
                         "cache_creation_input_tokens":3,"cache_read_input_tokens":7}}"#,
        );
        assert_eq!(out.text, "Починил флаки-тест");
        assert_eq!(out.tokens, 130, "кэш входит в расход");
        assert!(!out.failed);
    }

    #[test]
    fn plain_text_output_is_not_a_failure() {
        let out = parse_agent_json("просто текст");
        assert_eq!(out.text, "просто текст");
        assert_eq!(out.tokens, 0);
        assert!(!out.failed);
    }

    #[test]
    fn unrecognised_verdict_never_passes_work_through() {
        assert_eq!(parse_critic("OK\nвсё хорошо"), CriticSays::Fine);
        assert_eq!(
            parse_critic("ASK\nснимать ли тест"),
            CriticSays::Ask("снимать ли тест".into())
        );
        match parse_critic("выглядит нормально, но тесты снял") {
            CriticSays::Return(why) => assert!(why.contains("тесты снял")),
            other => panic!("непонятое обязано быть возвратом, а не {other:?}"),
        }
        assert!(matches!(parse_critic(""), CriticSays::Return(_)));
    }

    #[test]
    fn sampling_shows_every_nth() {
        assert!(!is_sampled(3, 2));
        assert!(is_sampled(3, 3));
        assert!(!is_sampled(0, 5), "ноль — выключено, а не «каждая»");
    }

    #[test]
    fn prompt_carries_goal_notes_and_human_words() {
        let mut l = Loop {
            name: "test-fix".into(),
            ..Default::default()
        };
        l.source.goal = "чинить флаки".into();
        l.exit.gates = vec![state::Gate {
            name: "тесты".into(),
            command: "cargo test".into(),
        }];
        let run = Run {
            interventions: vec!["не трогай CI".into()],
            ..Default::default()
        };
        let p = iteration_prompt(&l, &run, "#12 упал тест", "не трогать adopt_tmux", "");
        assert!(p.contains("чинить флаки") && p.contains("#12 упал тест"));
        assert!(
            p.contains("не трогать adopt_tmux"),
            "дневник обязан попасть в промт"
        );
        assert!(
            p.contains("не трогай CI"),
            "реплика человека обязана попасть в промт"
        );
        assert!(p.contains("ОДИН шаг"));
    }

    #[test]
    fn memory_off_is_not_mentioned() {
        let mut l = Loop::default();
        l.memory.enabled = false;
        assert!(!iteration_prompt(&l, &Run::default(), "", "", "").contains("Допиши"));
    }

    #[test]
    fn slug_survives_cyrillic() {
        assert_eq!(slug("ночной test-fix"), "test-fix");
        assert_eq!(slug("утренний триаж"), "loop", "мусорной ветки не будет");
    }

    #[test]
    fn summary_is_the_first_meaningful_line() {
        assert_eq!(summarize("\n\n  Починил тест \nещё"), "Починил тест");
        assert_eq!(summarize(""), "без ответа");
    }

    #[tokio::test]
    async fn gates_stop_at_the_first_red() {
        let gates = vec![
            state::Gate {
                name: "первый".into(),
                command: "true".into(),
            },
            state::Gate {
                name: "второй".into(),
                command: "false".into(),
            },
            state::Gate {
                name: "третий".into(),
                command: "true".into(),
            },
        ];
        let runs = run_gates(&Machine::local(), &gates, "/tmp").await;
        assert_eq!(runs.len(), 2);
        assert!(runs[0].ok && !runs[1].ok);
    }
}
