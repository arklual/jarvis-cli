//! Jarvis в терминале: агенты, циклы, связка — поверх узлового протокола.
//!
//! Состояние общее с настольной версией (тот же каталог данных), транспорт —
//! тот же, которым живёт мобильный клиент. Отсюда главное свойство порта: он
//! не второй Jarvis, а второй интерфейс к одному и тому же.

mod app;
mod cli;
mod core;
mod engine;
mod ui;

use ui::style::{paint, Caps, Role};

fn main() {
    restore_sigpipe();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let parsed = match cli::parse(&args) {
        Ok(p) => p,
        Err(e) => {
            fail(&e);
            std::process::exit(2);
        }
    };
    // Два рабочих потока — не про скорость, а про живучесть окна. Живой экран
    // ждёт клавиши блокирующим опросом, и на однопоточном рантайме это
    // затыкает всё: фоновое слияние или подъём руки не двигались, пока цикл
    // сидит на клавишах. Проверено — «вливаю…» висело двадцать секунд.
    // Больше двух не нужно: работа целиком в ожидании ввода-вывода.
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            fail(&format!("не поднялся рантайм: {e}"));
            std::process::exit(1);
        }
    };
    if let Err(e) = rt.block_on(app::run(parsed)) {
        fail(&e);
        std::process::exit(1);
    }
}

/// Вернуть SIGPIPE поведение по умолчанию.
///
/// Rust глушит этот сигнал, и запись в закрытую трубу становится ошибкой:
/// `jarvis ls | head` заканчивался паникой «failed printing to stdout». Для
/// программы, которую зовут из шелла, это не мелочь — пайп в `head`, `grep` и
/// `less` и есть обычный способ ею пользоваться.
fn restore_sigpipe() {
    // SAFETY: установка обработчика сигнала до старта потоков — ровно тот
    // случай, для которого этот вызов и существует.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

/// Ошибку — в stderr: stdout принадлежит данным, и пайп не должен получать
/// жалобы вперемешку с выводом.
///
/// Переносим по ширине окна и с отступом: длинный путь внутри сообщения иначе
/// рвётся посреди слова, и прочитать его нельзя. Строки после первой — это
/// советы «что делать», их приглушаем: сперва читают, ЧТО сломалось.
fn fail(msg: &str) {
    let caps = Caps::detect();
    let room = (caps.width as usize).saturating_sub(4).max(20);
    let mut first = true;
    for para in msg.lines() {
        for line in ui::style::wrap(para, room) {
            if first {
                eprintln!("{} {line}", paint(&caps, Role::Bad, "×"));
                first = false;
            } else {
                eprintln!("  {}", paint(&caps, Role::Dim, &line));
            }
        }
    }
}
