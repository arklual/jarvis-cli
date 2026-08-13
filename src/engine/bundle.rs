//! Связка в терминале: очередь слияний и её единственное решение — «влить».
//!
//! Автоматике принадлежит подготовка (ребейз, гейты, порядок), человеку —
//! решение. Поэтому из терминала доступно ровно то же, что кнопкой в панели:
//! влить голову очереди, и только когда она перебазирована и зелена.
//!
//! Движок ничего не печатает, а ВОЗВРАЩАЕТ отчёт строками. Читателей двое:
//! команда, которая печатает их в поток, и живое окно, где любая печать мимо
//! кадра порвала бы экран.

use crate::core::machine::{self, Machine};
use crate::core::state::{self, Bundle, HandState};
use crate::core::util::{now_ms, one_line};
use std::time::Duration;

fn host_for(b: &Bundle) -> Result<Machine, String> {
    let name = if b.machine.trim().is_empty() {
        "local"
    } else {
        b.machine.trim()
    };
    machine::list()
        .into_iter()
        .find(|m| m.name == name)
        .ok_or_else(|| format!("узел «{name}» не найден в настройках"))
}

async fn git(m: &Machine, dir: &str, args: &[&str]) -> (i32, String) {
    // Через шелл с цитированием: путь и ветка приходят из конфигурации, где
    // бывают пробелы, а один непроцитированный аргумент стоил бы ветки.
    let mut cmd = String::from("git");
    for a in args {
        cmd.push(' ');
        cmd.push_str(&crate::core::util::shell_quote(a));
    }
    machine::run(m, dir, &cmd, Duration::from_secs(120)).await
}

/// Ветка стоит на актуальной базе (база — предок ветки).
async fn rebased(m: &Machine, dir: &str, base: &str, branch: &str) -> bool {
    git(m, dir, &["merge-base", "--is-ancestor", base, branch])
        .await
        .0
        == 0
}

/// В дереве есть незакоммиченное.
async fn dirty(m: &Machine, dir: &str) -> bool {
    let (code, out) = git(m, dir, &["status", "--porcelain", "--untracked-files=all"]).await;
    // Не смогли спросить — считаем грязным: осторожность дешевле.
    code != 0 || !out.trim().is_empty()
}

/// Где база выписана: (каталог, чистое ли дерево).
async fn base_checkout(m: &Machine, dir: &str, base: &str) -> Option<(String, bool)> {
    let (code, out) = git(m, dir, &["worktree", "list", "--porcelain"]).await;
    if code != 0 {
        return None;
    }
    let mut path: Option<String> = None;
    for line in out.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            path = Some(p.trim().to_string());
        } else if let Some(br) = line.strip_prefix("branch ") {
            if br.trim() == format!("refs/heads/{base}") {
                let p = path.clone()?;
                let clean = !dirty(m, &p).await;
                return Some((p, clean));
            }
        }
    }
    None
}

/// Влить готовую руку: докрутить базу до её головы.
///
/// Осторожность одна, но принципиальная: база может быть выписана в рабочем
/// дереве человека. Чистое двигаем честным `merge --ff-only`; грязное не
/// трогаем вовсе — «закоммить или спрячь» лучше, чем молча испортить правку.
pub async fn merge(
    mut all: Vec<Bundle>,
    idx: usize,
    hand_needle: &str,
) -> Result<Vec<String>, String> {
    let b = all[idx].clone();
    // Пауза — это решение человека «стоп»; вливать в обход него значит делать
    // ровно то, от чего он остановился.
    if b.paused {
        return Err("связка на паузе — сними паузу, тогда волью".into());
    }
    let m = host_for(&b)?;
    let base = if b.base.trim().is_empty() {
        "main"
    } else {
        b.base.trim()
    };

    let queue = b.queue();
    let Some(head) = queue.first() else {
        return Err("очередь пуста — руки встанут в неё сами, когда закончат".into());
    };
    let needle = hand_needle.trim().to_lowercase();
    if !(head.name.to_lowercase() == needle
        || head.id.starts_with(&needle)
        || head.branch.to_lowercase().ends_with(&needle))
    {
        return Err(format!(
            "вливается только голова очереди — сейчас это «{}». По одному, с гейтами между",
            head.name
        ));
    }
    if !head.gates_ok {
        return Err("гейты не зелёные — вливать рано".into());
    }
    if !rebased(&m, &b.dir, base, &head.branch).await {
        return Err("ветка не на актуальной базе — дождись авторебейза".into());
    }
    match base_checkout(&m, &b.dir, base).await {
        Some((path, false)) => {
            return Err(format!(
                "{base} выписана в {path} с незакоммиченной правкой — закоммить или спрячь, тогда волью"
            ))
        }
        Some((path, true)) => {
            let (code, out) = git(&m, &path, &["merge", "--ff-only", &head.branch]).await;
            if code != 0 {
                return Err(format!("вливание не прошло: {}", crate::core::util::one_line(&out)));
            }
        }
        None => {
            let (code, sha) = git(&m, &b.dir, &["rev-parse", &head.branch]).await;
            if code != 0 {
                return Err("не узнал голову ветки".into());
            }
            let (code, out) = git(
                &m,
                &b.dir,
                &["update-ref", &format!("refs/heads/{base}"), sha.trim()],
            )
            .await;
            if code != 0 {
                return Err(format!("вливание не прошло: {}", crate::core::util::one_line(&out)));
            }
        }
    }

    let head_id = head.id.clone();
    let branch = head.branch.clone();
    let name = head.name.clone();
    if let Some(h) = all[idx].hands.iter_mut().find(|h| h.id == head_id) {
        h.state = HandState::Merged;
        h.merged_at = now_ms();
    }
    all[idx].last_merge_at = now_ms();
    all[idx].event(format!(
        "ты влил {branch} → {base} · хвост переребейзится сам"
    ));
    state::save_bundles(&all).map_err(|e| format!("не записал состояние: {e}"))?;

    let mut report = vec![format!("влито: {name} → {base}")];
    let left = all[idx].queue().len();
    if left > 0 {
        report.push(format!(
            "в очереди ещё {left} — хвост переребейзится и пересдаст гейты"
        ));
    }
    Ok(report)
}

/* ---------- убрать связку ---------- */

/// Живые руки — те, за которыми ещё стоит работа.
pub fn alive(b: &Bundle) -> Vec<&crate::core::state::Hand> {
    b.hands
        .iter()
        .filter(|h| {
            matches!(
                h.state,
                HandState::Working | HandState::Ready | HandState::Conflict
            )
        })
        .collect()
}

/// Убрать связку из реестра.
///
/// Две вещи, о которых легко забыть и потом жалеть. Первая: у рук есть живые
/// сессии, worktree и ветки — запись в файле это не они, и удаление записи их
/// не трогает. Вторая: ветки мы не удаляем НИКОГДА, даже по `--clean`, — в них
/// лежит работа, и восстановить её после `branch -D` нечем.
pub async fn remove(
    mut all: Vec<Bundle>,
    idx: usize,
    force: bool,
    clean: bool,
) -> Result<Vec<String>, String> {
    let b = all[idx].clone();
    let live = alive(&b);
    if !live.is_empty() && !force {
        let names: Vec<&str> = live.iter().map(|h| h.name.as_str()).collect();
        return Err(format!(
            "{} ещё в работе: {}. Останови их (pause) или повтори с --force",
            crate::core::util::plural(names.len() as u64, "рука", "руки", "рук"),
            names.join(", ")
        ));
    }

    let mut left: Vec<String> = Vec::new();
    let mut cleaned: Vec<String> = Vec::new();
    if clean {
        let m = host_for(&b)?;
        for h in b.hands.iter().filter(|h| !h.worktree.trim().is_empty()) {
            // `worktree remove` сам откажется убирать дерево с незакоммиченной
            // правкой — на это и рассчитываем: чистим только то, что не жалко.
            let (code, out) = git(&m, &b.dir, &["worktree", "remove", &h.worktree]).await;
            if code == 0 {
                cleaned.push(h.name.clone());
            } else {
                left.push(format!("{} ({})", h.worktree, one_line(&tail(&out))));
            }
        }
    } else {
        left.extend(
            b.hands
                .iter()
                .filter(|h| !h.worktree.trim().is_empty())
                .map(|h| h.worktree.clone()),
        );
    }

    all.remove(idx);
    state::save_bundles(&all).map_err(|e| format!("не записал состояние: {e}"))?;
    Ok(removal_report(&b, &cleaned, &left))
}

/// Что сказать человеку после удаления.
///
/// Чистая функция: она и проверяется тестами. Запись на диск проверять нечем —
/// а тест, который её вызовет, перепишет настоящий файл состояния (уже ловил
/// себя на этом: пустой `bundles.json` появился в `~/.jarvis` от одного
/// неосторожного прогона).
pub fn removal_report(b: &Bundle, cleaned: &[String], left: &[String]) -> Vec<String> {
    let mut report = vec![format!("связка «{}» убрана", b.name)];
    for name in cleaned {
        report.push(format!("{name}: worktree убран"));
    }
    if !left.is_empty() {
        report.push(format!("осталось на диске: {}", left.join(", ")));
    }
    let branches: Vec<&str> = b
        .hands
        .iter()
        .map(|h| h.branch.as_str())
        .filter(|br| !br.trim().is_empty())
        .collect();
    if !branches.is_empty() {
        // Ветки живут дальше — и это осознанно: в них работа рук.
        report.push(format!("ветки целы: {}", branches.join(", ")));
    }
    report
}

/// Хвост вывода git — только чтобы объяснить отказ одной строкой.
fn tail(out: &str) -> String {
    out.lines().last().unwrap_or("").to_string()
}

/* ---------- новая рука ---------- */

/// Имя руки из задачи, если человек не назвал сам: первые осмысленные слова.
pub fn hand_name(task: &str) -> String {
    let joined = task
        .split_whitespace()
        .take(3)
        .collect::<Vec<_>>()
        .join(" ");
    crate::core::util::ellipsize(&joined, 24)
}

/// Слаг, не занятый другой рукой связки: ветки не должны сталкиваться.
pub fn unique_slug(b: &Bundle, name: &str) -> String {
    let taken: Vec<String> = b
        .hands
        .iter()
        .map(|h| h.branch.trim_start_matches("team/").to_string())
        .collect();
    let base = crate::engine::builder::slug(name);
    if !taken.contains(&base) {
        return base;
    }
    (2..99)
        .map(|n| format!("{base}-{n}"))
        .find(|c| !taken.contains(c))
        .unwrap_or_else(|| format!("{base}-{}", now_ms() % 1000))
}

/// Родительский каталог: worktree живёт соседом проекта, а не в недрах
/// `~/.jarvis` — человек в него заглядывает и правит руками.
fn parent_of(dir: &str) -> String {
    let d = dir.trim_end_matches('/');
    match d.rsplit_once('/') {
        Some(("", _)) => "/".to_string(),
        Some((head, _)) => head.to_string(),
        None => ".".to_string(),
    }
}

/// Завести руку и поднять её агента: worktree, сессия, задача.
pub async fn add_hand(mut all: Vec<Bundle>, idx: usize, task: &str) -> Result<Vec<String>, String> {
    let b = all[idx].clone();
    if b.paused {
        return Err("связка на паузе — новые руки не поднимаются".into());
    }
    let m = host_for(&b)?;
    let base = if b.base.trim().is_empty() {
        "main"
    } else {
        b.base.trim()
    };
    let name = hand_name(task);
    let slug = unique_slug(&b, &name);
    let branch = format!("team/{slug}");
    let wt = format!("{}/wt-{slug}", parent_of(&b.dir));

    let (code, out) = git(&m, &b.dir, &["worktree", "add", "-b", &branch, &wt, base]).await;
    if code != 0 {
        return Err(format!(
            "не завёл worktree: {}",
            crate::core::util::one_line(&out)
        ));
    }

    // Задача плюс правила руки. Про коммиты говорим прямо: очередь слияний
    // видит только закоммиченное, и рука без коммитов не станет готовой
    // никогда — сколько бы работы она ни сделала.
    let brief = format!(
        "{task}\n\nТы — рука связки «{name}» в отдельном worktree ({wt}), ветка {branch}. \
         Работай только в этом каталоге. Закончив, закоммить всё осмысленными коммитами — \
         несделанный коммит для очереди слияний не существует. Проверки перед готовностью: {gates}.",
        task = task.trim(),
        name = b.name,
        gates = if b.gates.is_empty() {
            "нет".to_string()
        } else {
            b.gates
                .iter()
                .map(|g| g.command.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        },
    );

    let (client, _tunnel) = machine::connect(&m).await?;
    let pane = client
        .launch(&wt, "claude --dangerously-skip-permissions", &slug)
        .await
        .map_err(|e| format!("worktree готов, но агент не поднялся: {e}"))?;
    client
        .reply(&pane, &brief)
        .await
        .map_err(|e| format!("агент поднялся, но задача не доехала: {e}"))?;

    all[idx].hands.push(crate::core::state::Hand {
        id: format!("hand-{}", now_ms()),
        name: name.clone(),
        task: task.trim().to_string(),
        branch: branch.clone(),
        worktree: wt.clone(),
        pane,
        state: HandState::Working,
        ..Default::default()
    });
    all[idx].event(format!("{name}: рука запущена · {branch}"));
    state::save_bundles(&all).map_err(|e| format!("не записал состояние: {e}"))?;

    Ok(vec![
        format!("рука «{name}» работает · {branch}"),
        format!("worktree {wt}"),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::state::Hand;

    fn hand(id: &str, name: &str, st: HandState, ready: i64, gates: bool) -> Hand {
        Hand {
            id: id.into(),
            name: name.into(),
            branch: format!("team/{name}"),
            state: st,
            ready_at: ready,
            gates_ok: gates,
            ..Default::default()
        }
    }

    /// Живые руки — это работа, идущая прямо сейчас: выбросить их запись
    /// молча значит потерять из виду и агентов, и их ветки.
    #[tokio::test]
    async fn a_bundle_with_working_hands_is_not_removed_silently() {
        let b = Bundle {
            name: "api".into(),
            hands: vec![
                hand("h1", "очередь", HandState::Working, 0, false),
                hand("h2", "доки", HandState::Merged, 0, true),
            ],
            ..Default::default()
        };
        // Отказ приходит ДО записи на диск — состояние не тронуто.
        let err = remove(vec![b], 0, false, false).await.unwrap_err();
        assert!(err.contains("1 рука ещё в работе"), "{err}");
        assert!(
            err.contains("очередь"),
            "отказ обязан назвать, кто именно: {err}"
        );
        assert!(err.contains("--force"), "и путь дальше: {err}");
    }

    /// Ветки не удаляем никогда: в них лежит работа рук, а после `branch -D`
    /// восстановить её нечем.
    #[test]
    fn the_report_promises_nothing_about_branches() {
        let b = Bundle {
            name: "api".into(),
            hands: vec![hand("h1", "доки", HandState::Merged, 0, true)],
            ..Default::default()
        };
        let text = removal_report(&b, &[], &["/srv/wt-доки".into()]).join(" · ");
        assert!(text.contains("«api» убрана"), "{text}");
        assert!(text.contains("осталось на диске: /srv/wt-доки"), "{text}");
        assert!(text.contains("ветки целы: team/доки"), "{text}");
        assert!(
            !text.to_lowercase().contains("удал"),
            "нигде не обещаем удаление веток: {text}"
        );
    }

    /// Голова очереди определяется готовностью, а не местом в списке.
    #[test]
    fn queue_head_is_the_earliest_ready() {
        let b = Bundle {
            hands: vec![
                hand("h1", "auth", HandState::Working, 0, false),
                hand("h2", "docs", HandState::Ready, 200, true),
                hand("h3", "reducer", HandState::Ready, 100, true),
            ],
            ..Default::default()
        };
        assert_eq!(b.queue()[0].name, "reducer");
    }

    /// Пауза сильнее любой клавиши: человек остановил связку сам.
    #[tokio::test]
    async fn a_paused_bundle_refuses_both_merge_and_new_hands() {
        let b = Bundle {
            paused: true,
            hands: vec![hand("h1", "auth", HandState::Ready, 1, true)],
            ..Default::default()
        };
        let err = merge(vec![b.clone()], 0, "auth").await.unwrap_err();
        assert!(err.contains("на паузе"), "{err}");
        let err = add_hand(vec![b], 0, "почини тесты").await.unwrap_err();
        assert!(err.contains("на паузе"), "{err}");
    }

    #[tokio::test]
    async fn merging_a_non_head_is_refused_by_name() {
        let b = Bundle {
            id: "b1".into(),
            name: "релиз".into(),
            dir: "/nonexistent".into(),
            hands: vec![
                hand("h2", "docs", HandState::Ready, 200, true),
                hand("h3", "reducer", HandState::Ready, 100, true),
            ],
            ..Default::default()
        };
        let err = merge(vec![b], 0, "docs").await.unwrap_err();
        assert!(err.contains("только голова"), "{err}");
        assert!(
            err.contains("reducer"),
            "ошибка обязана назвать, кто голова: {err}"
        );
    }

    #[tokio::test]
    async fn empty_queue_says_so() {
        let b = Bundle {
            hands: vec![hand("h1", "auth", HandState::Working, 0, false)],
            ..Default::default()
        };
        let err = merge(vec![b], 0, "auth").await.unwrap_err();
        assert!(err.contains("очередь пуста"), "{err}");
    }

    #[tokio::test]
    async fn red_gates_block_the_merge() {
        let b = Bundle {
            hands: vec![hand("h1", "auth", HandState::Ready, 1, false)],
            ..Default::default()
        };
        let err = merge(vec![b], 0, "auth").await.unwrap_err();
        assert!(err.contains("гейты не зелёные"), "{err}");
    }
}
