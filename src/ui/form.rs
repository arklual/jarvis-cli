//! Заведение связки прямо в окне: несколько вопросов подряд.
//!
//! Конструктор в командах спрашивает диалогом, но окно в сыром режиме так не
//! умеет — и связку приходилось заводить снаружи, выйдя из окна. Здесь те же
//! вопросы, заданные по одному в той же строке ввода: Enter принимает (пустой
//! ответ — умолчание), Esc отменяет всё.
//!
//! Состояние формы — чистые данные, а переходы — чистая функция: что именно
//! получится из ответов, проверяют тесты, а не глаза на живом экране.

use crate::core::state::{Bundle, Critic, Exit, Gate, Limits, Loop, Sandbox};
use crate::core::util::now_ms;
use crate::engine::builder;
use crate::engine::presets::Slot;

/// Что заводим.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Kind {
    #[default]
    Bundle,
    Loop,
}

/// Что спрашиваем сейчас.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Dir,
    Name,
    Base,
    Gates,
    /// Цель цикла одной фразой.
    Goal,
    /// Откуда цикл берёт задачи.
    Source,
}

/// Собираемая связка: ответы по шагам.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Form {
    pub kind: Kind,
    pub dir: String,
    pub name: String,
    pub base: String,
    pub goal: String,
    pub command: String,
    pub gates: Vec<Gate>,
    step: usize,
}

const BUNDLE_STEPS: [Step; 4] = [Step::Dir, Step::Name, Step::Base, Step::Gates];
/// У цикла спрашиваем только то, без чего он не поедет: остальное — разумные
/// умолчания, которые правятся в панели или в файле. Длинная форма в окне
/// отпугивает ровно так же, как отпугивал конструктор в панели.
const LOOP_STEPS: [Step; 5] = [Step::Dir, Step::Name, Step::Goal, Step::Source, Step::Gates];

impl Form {
    /// Новая форма связки. Каталог передаёт вызывающий: на этой машине это
    /// каталог запуска, на удалённой — её домашний, и знать разницу форме
    /// незачем.
    pub fn new(cwd: &str) -> Self {
        Self {
            kind: Kind::Bundle,
            dir: cwd.to_string(),
            ..Default::default()
        }
    }

    pub fn new_loop(cwd: &str) -> Self {
        Self {
            kind: Kind::Loop,
            dir: cwd.to_string(),
            ..Default::default()
        }
    }

    fn steps(&self) -> &'static [Step] {
        match self.kind {
            Kind::Bundle => &BUNDLE_STEPS,
            Kind::Loop => &LOOP_STEPS,
        }
    }

    pub fn title(&self) -> &'static str {
        match self.kind {
            Kind::Bundle => "Новая связка",
            Kind::Loop => "Новый цикл",
        }
    }

    pub fn step(&self) -> Step {
        let steps = self.steps();
        steps[self.step.min(steps.len() - 1)]
    }

    /// Вопрос текущего шага и умолчание, которое примет пустой Enter.
    pub fn question(&self) -> (&'static str, String) {
        match self.step() {
            Step::Dir => ("Каталог проекта", self.dir.clone()),
            Step::Name => ("Имя связки", dir_name(&self.dir)),
            Step::Base => ("Базовая ветка", "main".to_string()),
            Step::Gates => (
                match self.kind {
                    Kind::Bundle => "Гейты перед вливанием: номера через запятую",
                    Kind::Loop => "Чем проверять работу: номера через запятую",
                },
                "пусто — без гейтов".to_string(),
            ),
            Step::Goal => ("Цель цикла одной фразой", String::new()),
            Step::Source => (
                "Откуда брать задачи: номер",
                "пусто — работать по цели".to_string(),
            ),
        }
    }

    /// Принять ответ и перейти дальше. `true` — форма заполнена.
    ///
    /// Пустой ответ означает умолчание, а не пустое поле: человек жмёт Enter
    /// именно за этим.
    pub fn accept(&mut self, answer: &str) -> bool {
        let a = answer.trim();
        match self.step() {
            Step::Dir => {
                if !a.is_empty() {
                    self.dir = a.trim_end_matches('/').to_string();
                }
            }
            Step::Name => {
                self.name = if a.is_empty() {
                    dir_name(&self.dir)
                } else {
                    a.to_string()
                };
            }
            Step::Base => {
                self.base = if a.is_empty() {
                    "main".into()
                } else {
                    a.into()
                };
            }
            Step::Goal => {
                // Цель — единственное, чего не придумать за человека: цикл без
                // неё крутит непонятно что и останавливается непонятно когда.
                if a.is_empty() {
                    return false;
                }
                self.goal = a.to_string();
            }
            Step::Source => {
                if !a.is_empty() {
                    let cat = builder::catalog(Slot::Source);
                    let Some(i) = crate::ui::prompt::parse_choice(a, cat.len(), usize::MAX) else {
                        return false;
                    };
                    self.command = cat[i].command.to_string();
                }
            }
            Step::Gates => {
                let cat = builder::catalog(Slot::Gate);
                // Непонятые номера не молчат: лучше переспросить, чем завести
                // связку без проверок, о которых человек думал, что задал их.
                let Some(picked) = crate::ui::prompt::parse_many(a, cat.len()) else {
                    return false;
                };
                self.gates = picked
                    .iter()
                    .filter_map(|i| cat.get(*i))
                    .map(|p| Gate {
                        name: p.name.to_string(),
                        command: p.command.to_string(),
                    })
                    .collect();
            }
        }
        self.step += 1;
        self.step >= self.steps().len()
    }

    /// Чего не хватает для запуска — то же, что проверяет команда.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.dir.starts_with('/') && !self.dir.starts_with('~') {
            out.push("каталог нужен полным путём".into());
        }
        if self.name.trim().is_empty() {
            out.push("нет имени".into());
        }
        if self.kind == Kind::Loop && self.goal.trim().is_empty() {
            out.push("у цикла нет цели".into());
        }
        out
    }

    /// Готовый цикл на выбранной машине., — умолчания: критик включён,
    /// стены на месте, будильник ручной. Цикл, который проснётся сам в первую
    /// же ночь после заведения, — не то, чего ждут от пяти вопросов.
    pub fn build_loop(&self, machine: &str) -> Loop {
        let mut l = Loop {
            id: format!("loop-{}", now_ms()),
            name: self.name.clone(),
            agent: "claude".into(),
            machine: machine.to_string(),
            sandbox: Sandbox {
                repo: self.dir.clone(),
                ..Default::default()
            },
            exit: Exit {
                gates: self.gates.clone(),
                critic: Critic::default(),
                ..Default::default()
            },
            limits: Limits::default(),
            created_at: now_ms(),
            ..Default::default()
        };
        l.source.goal = self.goal.clone();
        l.source.command = self.command.clone();
        l
    }

    /// Готовая связка. Машина — та, в которой открыто окно: руки поднимутся
    /// там же, где смотрят за ними.
    pub fn build(&self, machine: &str) -> Bundle {
        Bundle {
            id: format!("bundle-{}", now_ms()),
            name: self.name.clone(),
            machine: machine.to_string(),
            dir: self.dir.clone(),
            base: if self.base.is_empty() {
                "main".into()
            } else {
                self.base.clone()
            },
            gates: self.gates.clone(),
            created_at: now_ms(),
            ..Default::default()
        }
    }

    /// Уже отвеченное — чтобы человек видел, что набрал, а не помнил.
    pub fn filled(&self) -> Vec<(&'static str, String)> {
        let mut out = Vec::new();
        for (i, s) in self.steps().iter().enumerate() {
            if i >= self.step {
                break;
            }
            out.push(match s {
                Step::Dir => ("каталог", self.dir.clone()),
                Step::Name => ("имя", self.name.clone()),
                Step::Base => ("база", self.base.clone()),
                Step::Goal => ("цель", self.goal.clone()),
                Step::Source => (
                    "задачи",
                    if self.command.is_empty() {
                        "по цели".to_string()
                    } else {
                        self.command.clone()
                    },
                ),
                Step::Gates => (
                    "гейты",
                    if self.gates.is_empty() {
                        "нет".to_string()
                    } else {
                        self.gates
                            .iter()
                            .map(|g| g.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    },
                ),
            });
        }
        out
    }
}

fn dir_name(dir: &str) -> String {
    dir.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("проект")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(answers: &[&str]) -> Form {
        let mut f = Form::new("/home/bob/proj");
        for a in answers {
            f.accept(a);
        }
        f
    }

    /// Enter по всем вопросам обязан давать рабочую связку: умолчания и есть
    /// главный смысл формы.
    #[test]
    fn empty_answers_give_working_defaults() {
        let f = run(&["", "", "", ""]);
        let b = f.build("local");
        assert_eq!(b.dir, "/home/bob/proj");
        assert_eq!(b.name, "proj", "имя берётся из каталога");
        assert_eq!(b.base, "main");
        assert!(b.gates.is_empty());
        assert!(b.hands.is_empty());
        assert!(f.problems().is_empty(), "{:?}", f.problems());
    }

    #[test]
    fn answers_override_the_defaults() {
        let f = run(&["/srv/api/", "платежи", "release", ""]);
        let b = f.build("vps");
        assert_eq!(b.dir, "/srv/api", "хвостовой слэш убран");
        assert_eq!(b.name, "платежи");
        assert_eq!(b.base, "release");
        assert_eq!(b.machine, "vps");
    }

    /// Гейты выбираются номерами из каталога — тем же, что в конструкторе
    /// команды: помнить команды проверок человек не обязан.
    #[test]
    fn gates_are_picked_by_number_from_the_catalog() {
        let cat = builder::catalog(Slot::Gate);
        assert!(cat.len() >= 2);
        let f = run(&["", "", "", "1, 2"]);
        let b = f.build("local");
        assert_eq!(b.gates.len(), 2);
        assert_eq!(b.gates[0].name, cat[0].name);
        assert_eq!(b.gates[1].command, cat[1].command);
    }

    /// Чужой номер — повод переспросить, а не завести связку без проверок.
    #[test]
    fn a_wrong_number_keeps_the_question_open() {
        let mut f = Form::new("/srv/p");
        f.accept("");
        f.accept("");
        f.accept("");
        assert_eq!(f.step(), Step::Gates);
        assert!(!f.accept("999"), "форма закрылась на кривом ответе");
        assert_eq!(f.step(), Step::Gates, "вопрос обязан остаться");
        assert!(f.accept(""), "пустой ответ закрывает форму");
    }

    /// Цикл заводится теми же пятью вопросами, и всё неспрошенное — разумные
    /// умолчания: критик на месте, стены на месте, будильник ручной.
    #[test]
    fn a_loop_gets_defaults_for_everything_unasked() {
        let mut f = Form::new_loop("/srv/proj");
        assert_eq!(f.title(), "Новый цикл");
        f.accept("");
        f.accept("ночной обход");
        assert!(!f.accept(""), "цель обязательна — вопрос остаётся");
        assert_eq!(f.step(), Step::Goal);
        f.accept("чинить красные тесты");
        f.accept("");
        assert!(f.accept("1"));

        let l = f.build_loop("vps");
        assert_eq!(l.machine, "vps", "цикл обязан помнить, где ему крутиться");
        assert_eq!(l.name, "ночной обход");
        assert_eq!(l.sandbox.repo, "/srv/proj");
        assert_eq!(l.source.goal, "чинить красные тесты");
        assert!(l.source.command.is_empty(), "источник не выбирали");
        assert_eq!(l.exit.gates.len(), 1);
        assert!(
            l.exit.critic.enabled,
            "критик — умолчание, а не забытое поле"
        );
        assert!(
            l.limits.iterations > 0 && l.limits.tokens > 0,
            "цикл без стен"
        );
        assert_eq!(l.wake_label(), "только руками", "сам просыпаться не должен");
        assert!(f.problems().is_empty());
    }

    /// Источник задач берётся номером из каталога — как в конструкторе команды.
    #[test]
    fn a_loop_source_comes_from_the_catalog() {
        let cat = builder::catalog(Slot::Source);
        let mut f = Form::new_loop("/srv/proj");
        f.accept("");
        f.accept("");
        f.accept("чинить");
        assert!(!f.accept("999"), "чужой номер не проходит");
        f.accept("2");
        assert_eq!(f.command, cat[1].command);
    }

    #[test]
    fn filled_shows_only_what_is_answered() {
        let mut f = Form::new("/srv/p");
        assert!(f.filled().is_empty());
        f.accept("");
        assert_eq!(f.filled(), vec![("каталог", "/srv/p".to_string())]);
    }

    /// Относительный путь до добра не доведёт: worktree руки живёт соседом
    /// проекта, и «.» сделал бы его непонятно где.
    #[test]
    fn relative_directory_is_refused() {
        let mut f = Form::new("проект");
        f.accept("");
        f.accept("");
        assert!(f.problems().iter().any(|p| p.contains("полным путём")));
    }
}
