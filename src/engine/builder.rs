//! Конструкторы цикла и связки: диалог вместо простыни флагов.
//!
//! Урок панели: цикл не заводят не потому, что он сложен, а потому, что для
//! его описания надо ПОМНИТЬ — синтаксис `gh`, флаги тест-раннера, как обрезать
//! вывод. Здесь ровно то же лекарство: заготовки каталогом, выбор номером,
//! разумное умолчание на каждый вопрос. Всё вставленное остаётся строкой,
//! которую человек тут же правит — много кастомизации, мало запоминания.

use crate::core::state::{Bundle, Critic, Exit, Gate, Limits, Loop, Sampling, Sandbox, Wake};
use crate::core::util::now_ms;
use crate::engine::presets::{self, Slot};
use crate::ui::prompt::{self, Choice};
use crate::ui::style::{paint, rule, Caps, Role};

/// Модели агента — те же, что предлагает конструктор в панели.
const MODELS: [(&str, &str); 4] = [
    ("opus", "Opus — ровный выбор для критика"),
    ("fable", "Fable — самый вдумчивый, дороже"),
    ("sonnet", "Sonnet — быстрый и дешёвый"),
    ("haiku", "Haiku — для совсем механических проверок"),
];

/// Слаг из имени: для веток и имён файлов.
pub fn slug(name: &str) -> String {
    let s: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let s = s.trim_matches('-').to_string();
    let mut out = String::new();
    let mut dash = false;
    for c in s.chars() {
        if c == '-' {
            dash = true;
            continue;
        }
        if dash && !out.is_empty() {
            out.push('-');
        }
        dash = false;
        out.push(c);
    }
    if out.is_empty() {
        "loop".into()
    } else {
        out.chars().take(32).collect()
    }
}

fn gate_of(p: &presets::Preset) -> Gate {
    Gate {
        name: p.name.to_string(),
        command: p.command.to_string(),
    }
}

/// Имя каталога — самое вероятное имя цикла и связки.
fn dir_name(dir: &str) -> String {
    dir.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("проект")
        .to_string()
}

/// Итог словами: что именно человек сейчас заведёт. Показывается перед
/// сохранением — согласие вслепую хуже отказа.
pub fn summary(caps: &Caps, l: &Loop) -> Vec<String> {
    let mut out = vec![
        format!("имя      {}", l.name),
        format!("каталог  {}", l.sandbox.repo),
        format!("цель     {}", l.source.goal),
    ];
    if !l.source.command.is_empty() {
        out.push(format!("задачи   {}", l.source.command));
    }
    out.push(if l.exit.gates.is_empty() {
        "гейты    нет".to_string()
    } else {
        format!(
            "гейты    {}",
            l.exit
                .gates
                .iter()
                .map(|g| g.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    });
    out.push(format!(
        "конец    {}{}, подряд {}",
        if l.exit.critic.enabled {
            format!("критик ({}) ", l.exit.critic.model)
        } else {
            "без критика ".into()
        },
        if l.exit.gates.is_empty() {
            "по гейтам нечему"
        } else {
            "+ зелёные гейты"
        },
        l.exit.streak
    ));
    out.push(format!(
        "стены    {} итераций · {} мин · {} токенов",
        l.limits.iterations,
        l.limits.minutes,
        crate::ui::render::fmt_tokens(l.limits.tokens)
    ));
    out.push(format!("будильник {}", l.wake_label()));
    out.iter()
        .map(|s| paint(caps, Role::Dim, s))
        .collect::<Vec<_>>()
}

/// Заготовки одного слота — в порядке показа, разделами подряд.
///
/// Раздел, встреченный дважды («Rust» сверху и «Rust» через три пункта),
/// читается как ошибка каталога: человек ищет свой язык в одном месте.
/// Порядок самих разделов — тот, в котором они впервые встретились: он
/// осмысленный, от частого к редкому.
pub fn catalog(slot: Slot) -> Vec<presets::Preset> {
    let items: Vec<presets::Preset> = presets::all()
        .into_iter()
        .filter(|p| p.slot == slot)
        .collect();
    let mut order: Vec<&str> = Vec::new();
    for p in &items {
        if !order.contains(&p.category) {
            order.push(p.category);
        }
    }
    let mut out = Vec::with_capacity(items.len());
    for cat in order {
        out.extend(items.iter().filter(|p| p.category == cat).cloned());
    }
    out
}

/// Пункты меню из заготовок: название, подсказка, раздел.
pub fn choices(items: &[presets::Preset]) -> Vec<Choice> {
    items
        .iter()
        .map(|p| Choice::new(p.name, p.hint).in_group(p.category))
        .collect()
}

/// Диалог нового цикла. Возвращает готовый к сохранению цикл.
pub fn new_loop(caps: &Caps, cwd: &str, machine: &str) -> Result<Loop, String> {
    println!("{}", rule(caps, "Новый цикл"));
    println!(
        "{}",
        paint(
            caps,
            Role::Dim,
            "Рутина, которую агент крутит сам. Enter — согласиться с тем, что в скобках."
        )
    );

    let dir = prompt::ask(caps, "Каталог проекта", cwd)?;
    let name = prompt::ask(caps, "Имя цикла", &dir_name(&dir))?;
    let goal = prompt::ask(caps, "Цель одной фразой", "")?;

    // Источник задач. Первый пункт — «без команды»: цикл по одной цели тоже
    // законен, и заставлять человека придумывать команду ради галочки незачем.
    let src = catalog(Slot::Source);
    let mut items = vec![Choice::new("без команды", "работать по одной цели").in_group("Просто")];
    items.extend(choices(&src));
    let pick = prompt::choose(caps, "Откуда брать задачи", &items, 0)?;
    let command = if pick == 0 {
        String::new()
    } else {
        // Заготовка вставляется как умолчание — её видно и можно поправить
        // прямо здесь, а не «принять как есть».
        prompt::ask(caps, "Команда", src[pick - 1].command)?
    };

    // Гейты: множественный выбор, ничего не выбрать — тоже ответ.
    let gates_cat = catalog(Slot::Gate);
    let chosen = prompt::choose_many(caps, "Чем проверять работу", &choices(&gates_cat))?;
    let gates: Vec<Gate> = chosen.iter().map(|i| gate_of(&gates_cat[*i])).collect();

    let critic_on = prompt::yes(caps, "Критик проверяет итог итерации", true)?;
    let model = if critic_on {
        let items: Vec<Choice> = MODELS
            .iter()
            .map(|(id, hint)| Choice::new(*id, *hint))
            .collect();
        MODELS[prompt::choose(caps, "Модель критика", &items, 0)?]
            .0
            .to_string()
    } else {
        "opus".to_string()
    };

    let iterations = prompt::number(caps, "Стена: итераций", 20)? as u32;
    let minutes = prompt::number(caps, "Стена: минут", 480)? as u32;
    let tokens = prompt::number(caps, "Стена: токенов", 200_000)?;

    let wake_items = [
        Choice::new("только руками", "запускать самому: jarvis loop start"),
        Choice::new("каждый день", "в заданное время"),
        Choice::new("каждые N минут", "для коротких обходов"),
    ];
    let wake = match prompt::choose(caps, "Когда просыпаться", &wake_items, 0)? {
        1 => Wake::Daily {
            at: prompt::ask(caps, "Во сколько (ЧЧ:ММ)", "03:00")?,
        },
        2 => Wake::Every {
            minutes: prompt::number(caps, "Раз в сколько минут", 60)? as u32,
        },
        _ => Wake::Manual,
    };

    let mut l = Loop {
        id: format!("loop-{}", now_ms()),
        name,
        agent: "claude".into(),
        machine: machine.to_string(),
        sandbox: Sandbox {
            repo: dir,
            ..Default::default()
        },
        exit: Exit {
            gates,
            critic: Critic {
                enabled: critic_on,
                model,
                ..Default::default()
            },
            ..Default::default()
        },
        limits: Limits {
            tokens,
            iterations,
            minutes,
            ..Default::default()
        },
        sampling: Sampling::default(),
        created_at: now_ms(),
        ..Default::default()
    };
    l.schedule.wake = wake;
    l.source.goal = goal;
    l.source.command = command;
    Ok(l)
}

/// Диалог новой связки. Руки заводятся здесь же: связка без рук — пустая полка.
pub fn new_bundle(caps: &Caps, cwd: &str, machine: &str) -> Result<Bundle, String> {
    println!("{}", rule(caps, "Новая связка"));
    println!(
        "{}",
        paint(
            caps,
            Role::Dim,
            "Несколько агентов над одним проектом: у каждой руки свой worktree, слияния очередью."
        )
    );

    let dir = prompt::ask(caps, "Каталог проекта", cwd)?;
    let name = prompt::ask(caps, "Имя связки", &dir_name(&dir))?;
    let base = prompt::ask(caps, "Базовая ветка", "main")?;

    let gates_cat = catalog(Slot::Gate);
    let gates: Vec<Gate> = prompt::choose_many(
        caps,
        "Что должно быть зелёным перед вливанием",
        &choices(&gates_cat),
    )?
    .iter()
    .map(|i| gate_of(&gates_cat[*i]))
    .collect();

    Ok(Bundle {
        id: format!("bundle-{}", now_ms()),
        name,
        machine: machine.to_string(),
        dir,
        base,
        gates,
        created_at: now_ms(),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ответ про будильник обязан доехать до цикла: «каждый день в 03:00»,
    /// молча ставшее «только руками», — это цикл, который никогда не проснётся.
    #[test]
    fn wake_answer_reaches_the_loop() {
        let mut l = Loop::default();
        l.schedule.wake = Wake::Daily { at: "03:00".into() };
        assert_eq!(l.wake_label(), "каждый день в 03:00");
    }

    #[test]
    fn slug_survives_spaces_punctuation_and_cyrillic() {
        assert_eq!(slug("Ночной обход"), "ночной-обход");
        assert_eq!(slug("fix:  tests!!"), "fix-tests");
        assert_eq!(slug("---"), "loop", "пустой слаг ветку не назовёт");
        assert!(slug(&"а".repeat(80)).chars().count() <= 32);
    }

    #[test]
    fn summary_tells_the_whole_setup_before_saving() {
        let mut l = Loop {
            name: "ночной обход".into(),
            ..Default::default()
        };
        l.sandbox.repo = "/srv/proj".into();
        l.source.goal = "чинить красные тесты".into();
        l.exit.gates = vec![Gate {
            name: "cargo test".into(),
            command: "cargo test".into(),
        }];
        let text = summary(
            &Caps {
                color: false,
                truecolor: false,
                unicode: true,
                width: 80,
            },
            &l,
        )
        .join("\n");
        for must in [
            "ночной обход",
            "/srv/proj",
            "чинить красные тесты",
            "cargo test",
            "стены",
        ] {
            assert!(text.contains(must), "в итоге нет «{must}»:\n{text}");
        }
    }

    /// Разделы идут подряд: один и тот же язык не должен встречаться дважды.
    #[test]
    fn catalog_keeps_categories_together() {
        for slot in [Slot::Source, Slot::Gate] {
            let mut seen: Vec<&str> = Vec::new();
            let mut last = "";
            for p in catalog(slot) {
                if p.category != last {
                    assert!(
                        !seen.contains(&p.category),
                        "раздел «{}» встретился второй раз",
                        p.category
                    );
                    seen.push(p.category);
                    last = p.category;
                }
            }
        }
    }

    /// Каталог обязан делиться на слоты: гейт в поле источника — мусор.
    #[test]
    fn catalog_splits_by_slot() {
        let sources = catalog(Slot::Source);
        let gates = catalog(Slot::Gate);
        assert!(sources.len() >= 5 && gates.len() >= 5);
        assert!(sources.iter().all(|p| p.slot == Slot::Source));
        assert!(gates.iter().all(|p| p.slot == Slot::Gate));
    }
}
