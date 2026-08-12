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
    let args: Vec<String> = std::env::args().skip(1).collect();
    let parsed = match cli::parse(&args) {
        Ok(p) => p,
        Err(e) => {
            fail(&e);
            std::process::exit(2);
        }
    };
    // Многопоточный рантайм не нужен: работа целиком в ожидании ввода-вывода,
    // а однопоточный экономит и память, и время старта — для инструмента,
    // который зовут из шелла по десять раз в минуту, это заметно.
    let rt = match tokio::runtime::Builder::new_current_thread()
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

/// Ошибку — в stderr: stdout принадлежит данным, и пайп не должен получать
/// жалобы вперемешку с выводом.
fn fail(msg: &str) {
    let caps = Caps::detect();
    eprintln!("{} {msg}", paint(&caps, Role::Bad, "×"));
}
