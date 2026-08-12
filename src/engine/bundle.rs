//! Связка в терминале: очередь слияний и её единственное решение — «влить».
//!
//! Автоматике принадлежит подготовка (ребейз, гейты, порядок), человеку —
//! решение. Поэтому из терминала доступно ровно то же, что кнопкой в панели:
//! влить голову очереди, и только когда она перебазирована и зелена.

use crate::app::App;
use crate::core::machine::{self, Machine};
use crate::core::state::{self, Bundle, HandState};
use crate::core::util::now_ms;
use crate::ui::style::{paint, Role};
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
    app: &App,
    mut all: Vec<Bundle>,
    idx: usize,
    hand_needle: &str,
) -> Result<(), String> {
    let b = all[idx].clone();
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

    app.say(paint(
        &app.caps,
        Role::Accent,
        &format!("влито: {name} → {base}"),
    ));
    let left = all[idx].queue().len();
    if left > 0 {
        app.say(paint(
            &app.caps,
            Role::Dim,
            &format!("в очереди ещё {left} — хвост переребейзится и пересдаст гейты"),
        ));
    }
    Ok(())
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
        let app = App::new(false);
        let err = merge(&app, vec![b], 0, "docs").await.unwrap_err();
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
        let app = App::new(false);
        let err = merge(&app, vec![b], 0, "auth").await.unwrap_err();
        assert!(err.contains("очередь пуста"), "{err}");
    }

    #[tokio::test]
    async fn red_gates_block_the_merge() {
        let b = Bundle {
            hands: vec![hand("h1", "auth", HandState::Ready, 1, false)],
            ..Default::default()
        };
        let app = App::new(false);
        let err = merge(&app, vec![b], 0, "auth").await.unwrap_err();
        assert!(err.contains("гейты не зелёные"), "{err}");
    }
}
