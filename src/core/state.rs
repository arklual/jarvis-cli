//! Состояние на диске: циклы и связки — те же файлы, что у настольной версии.
//!
//! Общий каталог данных — решение, а не совпадение: цикл, заведённый в панели,
//! виден в терминале и наоборот. Два интерфейса, одно состояние. Отсюда и
//! требование к разбору: неизвестные поля пропускаем молча, чтобы CLI не ронял
//! файл, дописанный более новой панелью, и наоборот.

use crate::core::util::{jarvis_dir, now_ms};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/* ================= циклы ================= */

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Gate {
    pub name: String,
    pub command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Critic {
    pub enabled: bool,
    pub model: String,
    pub prompt: String,
}

impl Default for Critic {
    fn default() -> Self {
        Self {
            enabled: true,
            model: "opus".into(),
            prompt: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Exit {
    pub gates: Vec<Gate>,
    pub critic: Critic,
    /// Сколько итераций подряд всё зелёное. Одной мало: гейт мог пройти
    /// случайно — ровно тот флаки-тест, ради которого цикл и заводят.
    pub streak: u32,
}

impl Default for Exit {
    fn default() -> Self {
        Self {
            gates: Vec::new(),
            critic: Critic::default(),
            streak: 2,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Limits {
    pub tokens: u64,
    pub iterations: u32,
    pub minutes: u32,
    pub stop_on_drift: bool,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            tokens: 200_000,
            iterations: 20,
            minutes: 480,
            stop_on_drift: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Source {
    pub goal: String,
    pub command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Sandbox {
    pub repo: String,
    pub branch: String,
    pub worktree: bool,
}

impl Default for Sandbox {
    fn default() -> Self {
        Self {
            repo: String::new(),
            branch: "loop/{name}-{n}".into(),
            worktree: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Memory {
    pub enabled: bool,
    pub file: String,
}

impl Default for Memory {
    fn default() -> Self {
        Self {
            enabled: true,
            file: "notes.md".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Wake {
    #[default]
    Manual,
    Daily {
        at: String,
    },
    Every {
        minutes: u32,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Schedule {
    pub wake: Wake,
    pub resume_after_limit: bool,
    pub keep_awake: bool,
}

impl Default for Schedule {
    fn default() -> Self {
        Self {
            wake: Wake::Manual,
            resume_after_limit: true,
            keep_awake: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Sampling {
    pub every: u32,
}

impl Default for Sampling {
    fn default() -> Self {
        Self { every: 3 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Loop {
    pub id: String,
    pub name: String,
    pub agent: String,
    pub source: Source,
    pub sandbox: Sandbox,
    pub exit: Exit,
    pub memory: Memory,
    pub schedule: Schedule,
    pub limits: Limits,
    pub sampling: Sampling,
    pub created_at: i64,
    pub last_run_at: i64,
}

impl Loop {
    /// Чего не хватает для запуска. Списком, а не первой ошибкой: человек
    /// заполняет форму целиком и вправе увидеть все дыры разом.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.name.trim().is_empty() {
            out.push("у цикла нет имени".into());
        }
        if self.sandbox.repo.trim().is_empty() {
            out.push("не указан репозиторий".into());
        }
        if self.source.goal.trim().is_empty() && self.source.command.trim().is_empty() {
            out.push("не задан источник задач: ни цели, ни команды".into());
        }
        if self.exit.gates.is_empty() && !self.exit.critic.enabled {
            out.push("нет условия выхода: ни гейтов, ни критика".into());
        }
        if self.limits.tokens == 0 && self.limits.iterations == 0 && self.limits.minutes == 0 {
            out.push("нет ни одного ограничителя".into());
        }
        out
    }

    pub fn wake_label(&self) -> String {
        match &self.schedule.wake {
            Wake::Manual => "только руками".into(),
            Wake::Daily { at } => format!("каждый день в {at}"),
            Wake::Every { minutes } => match *minutes {
                0 => "только руками".into(),
                m if m % (24 * 60) == 0 => {
                    let d = m / (24 * 60);
                    if d == 7 {
                        "раз в неделю".into()
                    } else {
                        format!("раз в {d} дн.")
                    }
                }
                m if m % 60 == 0 => format!("каждые {} ч", m / 60),
                m => format!("каждые {m} мин"),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Verdict {
    #[default]
    Running,
    Passed,
    Returned,
    GateFailed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct GateRun {
    pub name: String,
    pub ok: bool,
    pub output: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Iteration {
    pub n: u32,
    pub started_at: i64,
    pub ended_at: i64,
    pub verdict: Verdict,
    pub summary: String,
    pub gates: Vec<GateRun>,
    pub critic: String,
    pub tokens: u64,
    pub cost_usd: f64,
    pub files: Vec<String>,
    pub sampled: bool,
    pub reviewed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum StopReason {
    #[default]
    None,
    Exit,
    Tokens,
    Iterations,
    Time,
    Drift,
    Stopped,
    Failed,
}

impl StopReason {
    #[allow(dead_code)] // читается панелью; в CLI пригодится для «возобновить»
    pub fn is_limit(self) -> bool {
        matches!(
            self,
            StopReason::Tokens | StopReason::Iterations | StopReason::Time
        )
    }

    pub fn word(self) -> &'static str {
        match self {
            StopReason::None => "",
            StopReason::Exit => "условие выхода выполнено",
            StopReason::Tokens => "ограничитель: токены",
            StopReason::Iterations => "ограничитель: итерации",
            StopReason::Time => "ограничитель: время",
            StopReason::Drift => "ушёл от цели",
            StopReason::Stopped => "остановлен вручную",
            StopReason::Failed => "сорвался",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum RunState {
    #[default]
    Idle,
    Running,
    Asking,
    Stopped,
    Done,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Ask {
    pub at: i64,
    pub question: String,
    pub options: Vec<String>,
    pub iteration: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Run {
    pub loop_id: String,
    pub n: u32,
    pub state: RunState,
    pub started_at: i64,
    pub ended_at: i64,
    pub branch: String,
    pub worktree: String,
    pub iterations: Vec<Iteration>,
    pub stop: StopReason,
    pub stop_note: String,
    pub tokens: u64,
    pub cost_usd: f64,
    pub ask: Option<Ask>,
    pub interventions: Vec<String>,
    pub streak: u32,
}

impl Run {
    pub fn pending_review(&self) -> usize {
        self.iterations
            .iter()
            .filter(|i| i.sampled && !i.reviewed)
            .count()
    }

    pub fn minutes(&self, now: i64) -> u32 {
        let end = if self.ended_at > 0 {
            self.ended_at
        } else {
            now
        };
        ((end - self.started_at).max(0) / 60_000) as u32
    }

    /// Какой ограничитель сработал. Проверяется ПЕРЕД итерацией: смысл стены —
    /// не начинать работу, на которую нет бюджета.
    pub fn tripped(&self, limits: &Limits, now: i64) -> Option<StopReason> {
        if limits.tokens > 0 && self.tokens >= limits.tokens {
            return Some(StopReason::Tokens);
        }
        if limits.iterations > 0 && self.iterations.len() as u32 >= limits.iterations {
            return Some(StopReason::Iterations);
        }
        if limits.minutes > 0 && self.minutes(now) >= limits.minutes {
            return Some(StopReason::Time);
        }
        None
    }
}

/* ================= связки ================= */

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum HandState {
    #[default]
    New,
    Working,
    Ready,
    Conflict,
    Merged,
    Failed,
}

impl HandState {
    pub fn word(self) -> &'static str {
        match self {
            HandState::New => "ждёт запуска",
            HandState::Working => "работает",
            HandState::Ready => "готов к мержу",
            HandState::Conflict => "конфликт",
            HandState::Merged => "влита",
            HandState::Failed => "не поднялась",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Hand {
    pub id: String,
    pub name: String,
    pub task: String,
    pub branch: String,
    pub worktree: String,
    pub pane: String,
    pub state: HandState,
    pub ready_at: i64,
    pub checked_sha: String,
    pub gates_ok: bool,
    pub attempt: u32,
    pub conflict_files: Vec<String>,
    pub touched: Vec<String>,
    pub merged_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Event {
    pub at: i64,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Bundle {
    pub id: String,
    pub name: String,
    pub machine: String,
    /// Директория проекта. Старые файлы звали её `repo` — читаем и так.
    #[serde(alias = "repo")]
    pub dir: String,
    pub base: String,
    pub gates: Vec<Gate>,
    pub budget_tokens: u64,
    pub paused: bool,
    pub hands: Vec<Hand>,
    pub events: Vec<Event>,
    pub created_at: i64,
    pub last_merge_at: i64,
}

impl Bundle {
    /// Очередь слияний: готовые руки в порядке готовности.
    pub fn queue(&self) -> Vec<&Hand> {
        let mut q: Vec<&Hand> = self
            .hands
            .iter()
            .filter(|h| h.state == HandState::Ready)
            .collect();
        q.sort_by_key(|h| h.ready_at);
        q
    }

    #[allow(dead_code)] // симметрия с панелью: связка «жива», пока есть кому работать
    pub fn active(&self) -> bool {
        self.hands.iter().any(|h| {
            matches!(
                h.state,
                HandState::Working | HandState::Ready | HandState::Conflict
            )
        })
    }

    pub fn event(&mut self, text: impl Into<String>) {
        self.events.push(Event {
            at: now_ms(),
            text: text.into(),
        });
        let extra = self.events.len().saturating_sub(50);
        if extra > 0 {
            self.events.drain(..extra);
        }
    }
}

/* ================= чтение и запись ================= */

fn atomic_write(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

pub fn loops_path(root: &Path) -> PathBuf {
    root.join("loops.json")
}

pub fn run_path(root: &Path, loop_id: &str) -> PathBuf {
    root.join("loops").join(format!("{loop_id}.json"))
}

pub fn bundles_path(root: &Path) -> PathBuf {
    root.join("bundles.json")
}

/// Битый файл — не повод падать: состояние важно, но не настолько, чтобы
/// из-за него не запускался инструмент.
fn read_json<T: serde::de::DeserializeOwned + Default>(path: &Path) -> T {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn load_loops_at(root: &Path) -> Vec<Loop> {
    read_json(&loops_path(root))
}

pub fn load_loops() -> Vec<Loop> {
    load_loops_at(&jarvis_dir())
}

pub fn save_loops_at(root: &Path, items: &[Loop]) -> std::io::Result<()> {
    atomic_write(&loops_path(root), &serde_json::to_string_pretty(items)?)
}

pub fn save_loops(items: &[Loop]) -> std::io::Result<()> {
    save_loops_at(&jarvis_dir(), items)
}

pub fn load_run_at(root: &Path, loop_id: &str) -> Option<Run> {
    let text = std::fs::read_to_string(run_path(root, loop_id)).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn load_run(loop_id: &str) -> Option<Run> {
    load_run_at(&jarvis_dir(), loop_id)
}

pub fn save_run_at(root: &Path, run: &Run) -> std::io::Result<()> {
    atomic_write(
        &run_path(root, &run.loop_id),
        &serde_json::to_string_pretty(run)?,
    )
}

pub fn save_run(run: &Run) -> std::io::Result<()> {
    save_run_at(&jarvis_dir(), run)
}

pub fn load_bundles_at(root: &Path) -> Vec<Bundle> {
    read_json(&bundles_path(root))
}

pub fn load_bundles() -> Vec<Bundle> {
    load_bundles_at(&jarvis_dir())
}

pub fn save_bundles_at(root: &Path, items: &[Bundle]) -> std::io::Result<()> {
    atomic_write(&bundles_path(root), &serde_json::to_string_pretty(items)?)
}

pub fn save_bundles(items: &[Bundle]) -> std::io::Result<()> {
    save_bundles_at(&jarvis_dir(), items)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scoped(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("jcli-state-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn loops_round_trip_through_disk() {
        let dir = scoped("loops");
        let mut l = Loop {
            id: "a".into(),
            name: "ночной test-fix".into(),
            ..Default::default()
        };
        l.schedule.wake = Wake::Daily { at: "02:00".into() };
        save_loops_at(&dir, &[l.clone()]).unwrap();
        let back = load_loops_at(&dir);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0], l);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Формат общий с панелью — вид на проводе обязан совпадать до буквы,
    /// иначе один интерфейс молча перестанет понимать другой.
    #[test]
    fn wire_format_matches_the_desktop() {
        let j = |w: Wake| serde_json::to_string(&w).unwrap();
        assert_eq!(j(Wake::Manual), "\"manual\"");
        assert_eq!(
            j(Wake::Daily { at: "02:00".into() }),
            r#"{"daily":{"at":"02:00"}}"#
        );
        assert_eq!(
            j(Wake::Every { minutes: 60 }),
            r#"{"every":{"minutes":60}}"#
        );
        assert_eq!(
            serde_json::to_string(&Verdict::GateFailed).unwrap(),
            "\"gateFailed\""
        );
        assert_eq!(
            serde_json::to_string(&StopReason::Tokens).unwrap(),
            "\"tokens\""
        );
        assert_eq!(
            serde_json::to_string(&HandState::Ready).unwrap(),
            "\"ready\""
        );

        let l = Loop {
            id: "a".into(),
            ..Default::default()
        };
        let text = serde_json::to_string(&l).unwrap();
        assert!(
            text.contains("\"lastRunAt\""),
            "camelCase на проводе: {text}"
        );
    }

    /// Файл, дописанный более новой панелью, обязан читаться: незнакомые поля
    /// пропускаются, недостающие берут умолчания.
    #[test]
    fn unknown_fields_do_not_break_reading() {
        let raw =
            r#"[{ "id": "a", "name": "цикл", "квантовыйРежим": true, "limits": { "tokens": 5 } }]"#;
        let items: Vec<Loop> = serde_json::from_str(raw).unwrap();
        assert_eq!(items[0].limits.tokens, 5);
        assert_eq!(items[0].limits.iterations, Limits::default().iterations);
        assert_eq!(items[0].sampling.every, 3);
    }

    #[test]
    fn broken_file_does_not_break_startup() {
        let dir = scoped("broken");
        std::fs::write(loops_path(&dir), "{ это не json").unwrap();
        assert!(load_loops_at(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bundle_reads_the_legacy_repo_field() {
        let b: Bundle = serde_json::from_str(r#"{ "id": "b", "repo": "/старый/путь" }"#).unwrap();
        assert_eq!(b.dir, "/старый/путь");
    }

    #[test]
    fn queue_is_ordered_by_readiness() {
        let hand = |id: &str, st: HandState, at: i64| Hand {
            id: id.into(),
            state: st,
            ready_at: at,
            ..Default::default()
        };
        let b = Bundle {
            hands: vec![
                hand("a", HandState::Working, 0),
                hand("b", HandState::Ready, 200),
                hand("c", HandState::Ready, 100),
                hand("d", HandState::Conflict, 5),
            ],
            ..Default::default()
        };
        let ids: Vec<&str> = b.queue().iter().map(|h| h.id.as_str()).collect();
        assert_eq!(
            ids,
            ["c", "b"],
            "конфликтная выпала, порядок — по готовности"
        );
    }

    #[test]
    fn limits_trip_before_the_iteration_starts() {
        let limits = Limits::default();
        let mut run = Run {
            tokens: 199_999,
            ..Default::default()
        };
        assert_eq!(run.tripped(&limits, 1_000), None);
        run.tokens = 200_000;
        assert_eq!(run.tripped(&limits, 1_000), Some(StopReason::Tokens));

        // Ноль — «без ограничения», а не «сразу стоп».
        let free = Limits {
            tokens: 0,
            iterations: 0,
            minutes: 0,
            stop_on_drift: false,
        };
        let heavy = Run {
            tokens: u64::MAX,
            ..Default::default()
        };
        assert_eq!(heavy.tripped(&free, i64::MAX / 2), None);
    }

    #[test]
    fn problems_name_every_gap_at_once() {
        let bare = Loop::default();
        let p = bare.problems();
        assert!(p.iter().any(|x| x.contains("имени")));
        assert!(p.iter().any(|x| x.contains("репозиторий")));
        assert!(p.iter().any(|x| x.contains("источник")));

        let mut ok = Loop {
            name: "test-fix".into(),
            source: Source {
                goal: "чинить флаки".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        ok.sandbox.repo = "/repo".into();
        assert!(ok.problems().is_empty(), "{:?}", ok.problems());
    }

    #[test]
    fn events_are_capped() {
        let mut b = Bundle::default();
        for i in 0..80 {
            b.event(format!("событие {i}"));
        }
        assert_eq!(b.events.len(), 50);
    }
}
