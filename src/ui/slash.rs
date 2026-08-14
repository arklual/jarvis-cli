//! Командная строка окна: `/` и дальше то же, что умеет CLI.
//!
//! Мысль простая: человек уже знает команды `jarvis` — незачем заставлять его
//! помнить ещё и раскладку клавиш окна. `/limits`, `/chat lct`, `/loops`
//! работают внутри окна ровно как снаружи, а клавиши остаются быстрым путём
//! для частого.
//!
//! Неизвестная команда в чате не считается ошибкой: у агента свои слэш-команды
//! (`/compact`, `/clear`, `/model`), и человек, набравший их, хочет попасть
//! именно к нему. Поэтому непонятое уходит агенту как есть — это ожидаемое
//! поведение, а не догадка.

/// Одна команда: имя, подсказка об аргументах и что она делает.
pub struct Command {
    pub name: &'static str,
    pub args: &'static str,
    pub what: &'static str,
}

const fn c(name: &'static str, args: &'static str, what: &'static str) -> Command {
    Command { name, args, what }
}

/// Всё, что понимает окно. Порядок — порядок показа в палитре: сверху то, чем
/// пользуются чаще.
pub fn all() -> &'static [Command] {
    &ALL
}

static ALL: [Command; 17] = [
    c("new", "", "завести связку"),
    c("chat", "<проект>", "открыть чат сессии"),
    c("screen", "", "экран сессии как есть"),
    c("stop", "", "прервать агента"),
    c("model", "<модель>", "сменить модель агента"),
    c("effort", "<уровень>", "сменить усилие агента"),
    c("limits", "", "обновить лимиты сейчас"),
    c("list", "", "вернуться к списку сессий"),
    c("loops", "", "циклы"),
    c("bundles", "", "связки"),
    c("merge", "", "влить голову очереди связки"),
    c("hand", "<задача>", "новая рука связки"),
    c("pause", "", "пауза связке и обратно"),
    c("rm", "", "убрать связку (спросит подтверждение)"),
    c("run", "<каталог>", "поднять агента в каталоге"),
    c("help", "", "клавиши окна"),
    c("quit", "", "выйти"),
];

/// Разобрать строку `/имя аргументы`.
///
/// Двойной слэш — это не команда, а текст, начинающийся со слэша: агенту тоже
/// иногда пишут про `/etc/hosts`.
pub enum Line {
    /// Команда окна или агента: имя без слэша и хвост.
    Cmd { name: String, rest: String },
    /// Обычный текст (в том числе с экранированным слэшем).
    Text(String),
}

pub fn parse(input: &str) -> Line {
    let s = input.trim_start();
    let Some(tail) = s.strip_prefix('/') else {
        return Line::Text(input.trim().to_string());
    };
    if let Some(escaped) = tail.strip_prefix('/') {
        return Line::Text(format!("/{}", escaped.trim_end()));
    }
    let mut it = tail.splitn(2, char::is_whitespace);
    let name = it.next().unwrap_or("").trim().to_lowercase();
    let rest = it.next().unwrap_or("").trim().to_string();
    Line::Cmd { name, rest }
}

/// Насколько запрос похож на слово. Меньше — лучше; `None` — не подходит.
///
/// Правила те же, что у pi: буквы запроса должны идти по порядку, но не
/// обязательно подряд. Подряд идущие — награда, разрывы — штраф, попадание в
/// начало слова — большая награда. Так `/nb` находит «new bundle», а человеку
/// не надо помнить, с какой буквы команда начинается.
pub fn fuzzy_score(query: &str, text: &str) -> Option<f32> {
    let q: Vec<char> = query.trim().to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    if q.is_empty() {
        return Some(0.0);
    }
    if q.len() > t.len() {
        return None;
    }
    let mut qi = 0usize;
    let mut score = 0.0f32;
    let mut last: i32 = -1;
    let mut run = 0.0f32;
    for (i, ch) in t.iter().enumerate() {
        if qi >= q.len() {
            break;
        }
        if *ch != q[qi] {
            continue;
        }
        let boundary = i == 0 || matches!(t[i - 1], ' ' | '-' | '_' | '.' | '/' | ':');
        if last == i as i32 - 1 {
            run += 1.0;
            score -= run * 5.0;
        } else {
            run = 0.0;
            if last >= 0 {
                score += (i as f32 - last as f32 - 1.0) * 2.0;
            }
        }
        if boundary {
            score -= 10.0;
        }
        score += i as f32 * 0.1;
        last = i as i32;
        qi += 1;
    }
    if qi < q.len() {
        return None;
    }
    if q.len() == t.len() {
        score -= 100.0;
    }
    Some(score)
}

/// Команды, подходящие под введённое: сначала по началу имени, потом по
/// похожести.
///
/// Начало имени идёт первым не из вежливости: человек, набравший `/li`, ждёт
/// `limits` и `list` — и увидеть их ниже «похожих» было бы издевательством.
pub fn matching(prefix: &str) -> Vec<&'static Command> {
    let p = prefix.trim_start_matches('/').to_lowercase();
    let mut exact: Vec<&'static Command> = Vec::new();
    let mut fuzzy: Vec<(f32, &'static Command)> = Vec::new();
    for c in all() {
        if c.name.starts_with(&p) {
            exact.push(c);
        } else if let Some(score) = fuzzy_score(&p, c.name) {
            fuzzy.push((score, c));
        }
    }
    fuzzy.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    exact.extend(fuzzy.into_iter().map(|(_, c)| c));
    exact
}

/// Что команда ждёт вторым словом — по этому окно и подбирает подсказки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arg {
    /// Ничего.
    None,
    /// Имя сессии: подставляем из живого списка.
    Session,
    /// Каталог на этой машине.
    Dir,
    /// Слово из готового набора (например «цикл» или «связка»).
    Word(&'static [&'static str]),
    /// Свободный текст — дополнять нечем.
    Free,
}

pub fn arg_of(name: &str) -> Arg {
    match name {
        "chat" => Arg::Session,
        "run" => Arg::Dir,
        "new" => Arg::Word(&["связка", "цикл"]),
        "model" => Arg::Word(&["opus", "fable", "sonnet", "haiku"]),
        "effort" => Arg::Word(&["low", "medium", "high", "max"]),
        "hand" => Arg::Free,
        _ => Arg::None,
    }
}

/// Дополнить аргумент по набранному куску: общее начало подходящих.
pub fn complete_arg(part: &str, candidates: &[String]) -> Option<String> {
    let hits = rank(part, candidates);
    let first = hits.first()?;
    let mut common = first.clone();
    for h in &hits[1..] {
        while !h.starts_with(&common) {
            common.pop();
            if common.is_empty() {
                return None;
            }
        }
    }
    Some(common)
}

/// Отсортировать кандидатов по похожести на набранное.
pub fn rank(part: &str, candidates: &[String]) -> Vec<String> {
    let mut scored: Vec<(f32, &String)> = candidates
        .iter()
        .filter_map(|c| fuzzy_score(part, c).map(|s| (s, c)))
        .collect();
    scored.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().map(|(_, c)| c.clone()).collect()
}

/// Дополнение по Tab: общее начало всех подходящих.
///
/// Дополняем до общего префикса, а не до первой попавшейся команды: `/m` — это
/// и `model`, и `merge`, и подставлять одну из них молча значит запускать не
/// то, что человек имел в виду.
pub fn complete(prefix: &str) -> Option<String> {
    let p = prefix.trim_start_matches('/').to_lowercase();
    // Дополняем по НАЧАЛУ имени: похожесть хороша для показа списка, но
    // подставлять по ней — значит угадывать за человека. Общее начало
    // подходящих — ровно то, в чём сомнений нет.
    let starts: Vec<&'static Command> = all().iter().filter(|c| c.name.starts_with(&p)).collect();
    let hits = if starts.is_empty() {
        // Ничего не начинается так — но если похожая ровно одна, выбора всё
        // равно нет, и подставить её честно.
        let fuzzy = matching(prefix);
        if fuzzy.len() == 1 {
            fuzzy
        } else {
            return None;
        }
    } else {
        starts
    };
    let first = hits.first()?;
    let mut common = first.name.to_string();
    for h in &hits[1..] {
        while !h.name.starts_with(&common) {
            common.pop();
            if common.is_empty() {
                return None;
            }
        }
    }
    // Единственная команда — сразу с пробелом под аргумент, если он нужен.
    let tail = if hits.len() == 1 && !first.args.is_empty() {
        " "
    } else {
        ""
    };
    Some(format!("/{common}{tail}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(s: &str) -> (String, String) {
        match parse(s) {
            Line::Cmd { name, rest } => (name, rest),
            Line::Text(t) => panic!("это оказался текст: {t}"),
        }
    }

    fn text(s: &str) -> String {
        match parse(s) {
            Line::Text(t) => t,
            Line::Cmd { name, .. } => panic!("это оказалась команда: {name}"),
        }
    }

    #[test]
    fn command_splits_into_name_and_tail() {
        assert_eq!(cmd("/chat lct"), ("chat".into(), "lct".into()));
        assert_eq!(cmd("/limits"), ("limits".into(), String::new()));
        assert_eq!(
            cmd("/hand починить флаки в очереди"),
            ("hand".into(), "починить флаки в очереди".into())
        );
        assert_eq!(cmd("/MODEL Opus").0, "model", "имя команды без регистра");
    }

    /// Слэш в начале сообщения — обычное дело; экранируем вторым слэшем.
    #[test]
    fn double_slash_is_plain_text() {
        assert_eq!(text("//etc/hosts не трогай"), "/etc/hosts не трогай");
        assert_eq!(text("обычный текст"), "обычный текст");
    }

    #[test]
    fn matching_narrows_by_prefix() {
        assert!(matching("/li").iter().any(|c| c.name == "limits"));
        assert!(matching("/li").iter().all(|c| c.name.starts_with("li")));
        assert_eq!(matching("/quit").len(), 1);
        assert!(matching("/нетакой").is_empty());
        assert_eq!(matching("/").len(), all().len(), "пустой префикс — все");
    }

    /// Похожесть — чтобы не помнить, с какой буквы начинается команда.
    #[test]
    fn fuzzy_finds_by_letters_in_order() {
        assert!(
            fuzzy_score("nb", "new bundle").is_some(),
            "«nb» обязано находить «new bundle»"
        );
        assert!(
            fuzzy_score("chat", "chat").unwrap() < fuzzy_score("cht", "chat").unwrap(),
            "точное совпадение обязано быть лучше приблизительного"
        );
        assert!(
            fuzzy_score("xyz", "chat").is_none(),
            "чужие буквы не подходят"
        );
        assert!(
            fuzzy_score("", "chat").is_some(),
            "пустой запрос подходит всему"
        );
        // Порядок важен: «tahc» — не «chat».
        assert!(fuzzy_score("tahc", "chat").is_none());
    }

    /// Набравший `/li` ждёт limits и list первыми: видеть их под «похожими»
    /// было бы издевательством.
    #[test]
    fn prefix_matches_come_before_fuzzy_ones() {
        let names: Vec<&str> = matching("/li").iter().map(|c| c.name).collect();
        assert!(names[0].starts_with("li"), "{names:?}");
        assert!(
            names.iter().take(2).all(|n| n.starts_with("li")),
            "{names:?}"
        );
    }

    #[test]
    fn arguments_are_known_per_command() {
        assert_eq!(arg_of("chat"), Arg::Session);
        assert_eq!(arg_of("run"), Arg::Dir);
        assert_eq!(arg_of("limits"), Arg::None);
        assert!(matches!(arg_of("model"), Arg::Word(_)));
    }

    #[test]
    fn argument_completion_stops_at_the_common_start() {
        let cands = vec![
            "jarvis".to_string(),
            "jarvis-cli".to_string(),
            "lct".to_string(),
        ];
        assert_eq!(complete_arg("ja", &cands).as_deref(), Some("jarvis"));
        assert_eq!(complete_arg("lc", &cands).as_deref(), Some("lct"));
        assert_eq!(complete_arg("zzz", &cands), None);
        // Ранжирование отдаёт подходящее первым, а не в порядке списка.
        assert_eq!(
            rank("cli", &cands).first().map(String::as_str),
            Some("jarvis-cli")
        );
    }

    /// Дополнение не должно угадывать за человека: `/m` — это и model, и merge.
    #[test]
    fn completion_stops_at_the_common_prefix() {
        assert_eq!(
            complete("/m").as_deref(),
            Some("/m"),
            "model и merge — не угадываем"
        );
        assert_eq!(
            complete("/li").as_deref(),
            Some("/li"),
            "limits и list — тоже"
        );
        // Как только кандидат один — дополняем целиком, с пробелом под
        // аргумент там, где он нужен.
        assert_eq!(complete("/mo").as_deref(), Some("/model "));
        assert_eq!(complete("/lim").as_deref(), Some("/limits"));
        assert_eq!(complete("/нетакой"), None);
    }

    /// Аргументы у команды — часть договора: без них подсказка врёт.
    #[test]
    fn commands_with_arguments_say_so() {
        for name in ["chat", "model", "effort", "hand", "run"] {
            let c = all().iter().find(|c| c.name == name).unwrap();
            assert!(!c.args.is_empty(), "{name} без подсказки об аргументе");
        }
        for c in all() {
            assert!(!c.what.is_empty(), "{} без объяснения", c.name);
        }
    }
}
