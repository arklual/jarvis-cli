//! Заведение связки прямо в окне: несколько вопросов подряд.
//!
//! Конструктор в командах спрашивает диалогом, но окно в сыром режиме так не
//! умеет — и связку приходилось заводить снаружи, выйдя из окна. Здесь те же
//! вопросы, заданные по одному в той же строке ввода: Enter принимает (пустой
//! ответ — умолчание), Esc отменяет всё.
//!
//! Состояние формы — чистые данные, а переходы — чистая функция: что именно
//! получится из ответов, проверяют тесты, а не глаза на живом экране.

use crate::core::state::{Bundle, Gate};
use crate::core::util::now_ms;
use crate::engine::builder;
use crate::engine::presets::Slot;

/// Что спрашиваем сейчас.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Dir,
    Name,
    Base,
    Gates,
}

/// Собираемая связка: ответы по шагам.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Form {
    pub dir: String,
    pub name: String,
    pub base: String,
    pub gates: Vec<Gate>,
    step: usize,
}

const STEPS: [Step; 4] = [Step::Dir, Step::Name, Step::Base, Step::Gates];

impl Form {
    /// Новая форма: каталог, откуда запущено окно, — самый вероятный ответ.
    pub fn new(cwd: &str) -> Self {
        Self {
            dir: cwd.to_string(),
            ..Default::default()
        }
    }

    pub fn step(&self) -> Step {
        STEPS[self.step.min(STEPS.len() - 1)]
    }

    /// Вопрос текущего шага и умолчание, которое примет пустой Enter.
    pub fn question(&self) -> (&'static str, String) {
        match self.step() {
            Step::Dir => ("Каталог проекта", self.dir.clone()),
            Step::Name => ("Имя связки", dir_name(&self.dir)),
            Step::Base => ("Базовая ветка", "main".to_string()),
            Step::Gates => (
                "Гейты перед вливанием: номера через запятую",
                "пусто — без гейтов".to_string(),
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
        self.step >= STEPS.len()
    }

    /// Чего не хватает для запуска — то же, что проверяет команда.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.dir.starts_with('/') && !self.dir.starts_with('~') {
            out.push("каталог нужен полным путём".into());
        }
        if self.name.trim().is_empty() {
            out.push("у связки нет имени".into());
        }
        out
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
        for (i, s) in STEPS.iter().enumerate() {
            if i >= self.step {
                break;
            }
            out.push(match s {
                Step::Dir => ("каталог", self.dir.clone()),
                Step::Name => ("имя", self.name.clone()),
                Step::Base => ("база", self.base.clone()),
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
