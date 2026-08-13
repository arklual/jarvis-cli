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

/// Команды, подходящие под начало имени: для палитры и дополнения.
pub fn matching(prefix: &str) -> Vec<&'static Command> {
    let p = prefix.trim_start_matches('/').to_lowercase();
    all()
        .iter()
        .filter(|c| c.name.starts_with(&p))
        .collect::<Vec<_>>()
}

/// Дополнение по Tab: общее начало всех подходящих.
///
/// Дополняем до общего префикса, а не до первой попавшейся команды: `/m` — это
/// и `model`, и `merge`, и подставлять одну из них молча значит запускать не
/// то, что человек имел в виду.
pub fn complete(prefix: &str) -> Option<String> {
    let hits = matching(prefix);
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
