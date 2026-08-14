//! Строка ввода: курсор, многострочность, правка посередине.
//!
//! Раньше ввод был `String`, куда буквы добавлялись в конец: опечатку в начале
//! длинного сообщения приходилось стирать целиком. Здесь курсор живёт отдельно
//! от текста, и всё, что человек ждёт от поля ввода, работает: стрелки, начало
//! и конец строки, перевод строки внутри сообщения.
//!
//! Курсор — байтовый индекс, но двигается только по границам символов: в
//! русском тексте байт и буква не одно и то же, а срез по середине буквы —
//! паника, то есть потеря всего набранного.

/// Многострочное поле ввода с курсором.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Editor {
    text: String,
    /// Байтовое смещение курсора; всегда на границе символа.
    cursor: usize,
    /// Снимки для отмены. Клавиша «стереть слово» без отмены — это ловушка:
    /// одно нажатие уносит работу, и вернуть её нечем.
    undo: Vec<(String, usize)>,
    /// Кольцо убитого текста: что стёрли — можно вернуть (Ctrl+Y), как в
    /// readline. Стирание без возврата заставляет набирать заново.
    kill: Vec<String>,
    /// Отправленное раньше: стрелки поднимают прошлые сообщения, как в шелле.
    /// Без истории повтор «того же, но иначе» набирают с нуля.
    history: Vec<String>,
    /// Где стоим в истории: `len` — в свежем, ещё не отправленном тексте.
    hist_at: usize,
    /// Черновик, отложенный на время прогулки по истории.
    hist_draft: String,
}

/// Сколько снимков отмены храним. Больше сотни — это уже не «ой», а
/// переписывание, для которого есть внешний редактор.
const UNDO_DEPTH: usize = 100;
/// Сколько отправленного помним. Столько же, сколько показывает `history` в
/// шелле по умолчанию, — дальше листать всё равно перестают.
const HISTORY_DEPTH: usize = 100;

impl Editor {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    pub fn set(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.cursor = self.text.len();
    }

    /// Запомнить состояние перед правкой — для отмены.
    ///
    /// Подряд идущие вставки букв в один снимок не сливаем намеренно: «отменить
    /// по слову» звучит умно, но на практике человек жмёт отмену, пока не
    /// увидит нужное, и мелкий шаг предсказуемее.
    fn snapshot(&mut self) {
        if self
            .undo
            .last()
            .is_some_and(|(t, c)| *t == self.text && *c == self.cursor)
        {
            return;
        }
        self.undo.push((self.text.clone(), self.cursor));
        let extra = self.undo.len().saturating_sub(UNDO_DEPTH);
        if extra > 0 {
            self.undo.drain(..extra);
        }
    }

    /// Отменить последнюю правку.
    pub fn undo(&mut self) -> bool {
        match self.undo.pop() {
            Some((t, c)) => {
                self.text = t;
                self.cursor = c.min(self.text.len());
                true
            }
            None => false,
        }
    }

    pub fn insert(&mut self, c: char) {
        self.snapshot();
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    /// Вставить кусок текста целиком — как приходит из буфера обмена.
    ///
    /// Приходит он одним куском и часто многострочный: если разбирать его по
    /// символам через `insert`, каждый перевод строки выглядел бы как нажатый
    /// Enter — то есть отправкой недописанного. Поэтому вставка — отдельная
    /// операция, и переводы строк в ней остаются текстом.
    pub fn paste(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.snapshot();
        // \r из буфера обмена (Windows, некоторые терминалы) в поле ввода не
        // нужен: он невидим и ломает подсчёт строк.
        let clean = text.replace("\r\n", "\n").replace('\r', "\n");
        self.text.insert_str(self.cursor, &clean);
        self.cursor += clean.len();
    }

    /// Перевод строки внутри сообщения: длинную мысль пишут в несколько строк,
    /// и Enter, отправляющий недописанное, — главный способ это испортить.
    pub fn newline(&mut self) {
        self.insert('\n');
    }

    /* ---------- слова ---------- */

    /// Начало слова слева от курсора.
    ///
    /// Слово — как в readline: пробелы пропускаем, потом идём по «телу» слова.
    /// Знаки препинания считаем отдельным словом: `foo(bar` стирается по
    /// кусочкам, а не целиком — так ведут себя все поля ввода, к которым
    /// человек привык.
    pub fn word_start(&self) -> usize {
        let mut i = self.cursor;
        let bytes = self.text.as_bytes();
        let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80;
        while i > 0 && (self.text[..i].chars().next_back()).is_some_and(|c| c.is_whitespace()) {
            i -= self.text[..i]
                .chars()
                .next_back()
                .map(char::len_utf8)
                .unwrap_or(1);
        }
        if i == 0 {
            return 0;
        }
        let word_like = is_word(bytes[i - 1]);
        while i > 0 {
            let Some(c) = self.text[..i].chars().next_back() else {
                break;
            };
            if c.is_whitespace() {
                break;
            }
            let b = c.len_utf8();
            let this_word = is_word(self.text.as_bytes()[i - b]);
            if this_word != word_like {
                break;
            }
            i -= b;
        }
        i
    }

    /// Конец слова справа от курсора.
    pub fn word_end(&self) -> usize {
        let mut i = self.cursor;
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        let rest: Vec<char> = self.text[i..].chars().collect();
        let mut k = 0;
        while k < rest.len() && rest[k].is_whitespace() {
            i += rest[k].len_utf8();
            k += 1;
        }
        if k >= rest.len() {
            return i;
        }
        let word_like = is_word(rest[k]);
        while k < rest.len() && !rest[k].is_whitespace() && is_word(rest[k]) == word_like {
            i += rest[k].len_utf8();
            k += 1;
        }
        i
    }

    pub fn word_left(&mut self) {
        self.cursor = self.word_start();
    }

    pub fn word_right(&mut self) {
        self.cursor = self.word_end();
    }

    /// Стереть слово слева (Ctrl+W) — в кольцо, чтобы его можно было вернуть.
    pub fn kill_word_left(&mut self) {
        let from = self.word_start();
        if from == self.cursor {
            return;
        }
        self.snapshot();
        let dead = self.text[from..self.cursor].to_string();
        self.text.replace_range(from..self.cursor, "");
        self.cursor = from;
        self.push_kill(dead);
    }

    /// Стереть слово справа.
    pub fn kill_word_right(&mut self) {
        let to = self.word_end();
        if to == self.cursor {
            return;
        }
        self.snapshot();
        let dead = self.text[self.cursor..to].to_string();
        self.text.replace_range(self.cursor..to, "");
        self.push_kill(dead);
    }

    /* ---------- кольцо убитого ---------- */

    fn push_kill(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        self.kill.push(text);
        let extra = self.kill.len().saturating_sub(20);
        if extra > 0 {
            self.kill.drain(..extra);
        }
    }

    /// Вернуть последнее убитое (Ctrl+Y).
    pub fn yank(&mut self) -> bool {
        let Some(text) = self.kill.last().cloned() else {
            return false;
        };
        self.paste(&text);
        true
    }

    /// Пройтись по кольцу назад (Alt+Y после Ctrl+Y): вставить предыдущее
    /// убитое вместо только что вставленного.
    pub fn yank_pop(&mut self) -> bool {
        if self.kill.len() < 2 {
            return false;
        }
        let last = self.kill.pop().unwrap();
        self.kill.insert(0, last.clone());
        // Снимаем только что вставленное и кладём предыдущее.
        let at = self.cursor.saturating_sub(last.len());
        if self.text[at..].starts_with(&last) {
            self.text.replace_range(at..self.cursor, "");
            self.cursor = at;
        }
        let now = self.kill.last().cloned().unwrap_or_default();
        self.paste(&now);
        true
    }

    /* ---------- история отправленного ---------- */

    /// Запомнить отправленное. Повтор подряд не задваиваем — в шелле так же.
    pub fn remember(&mut self, text: &str) {
        let t = text.trim();
        if t.is_empty() {
            return;
        }
        if self.history.last().map(String::as_str) == Some(t) {
            self.hist_at = self.history.len();
            return;
        }
        self.history.push(t.to_string());
        let extra = self.history.len().saturating_sub(HISTORY_DEPTH);
        if extra > 0 {
            self.history.drain(..extra);
        }
        self.hist_at = self.history.len();
    }

    /// Шаг назад по истории. `false` — дальше некуда.
    pub fn history_prev(&mut self) -> bool {
        if self.history.is_empty() || self.hist_at == 0 {
            return false;
        }
        if self.hist_at == self.history.len() {
            // Уходя в историю, откладываем недописанное: вернуться к нему
            // человек захочет обязательно.
            self.hist_draft = self.text.clone();
        }
        self.hist_at -= 1;
        let t = self.history[self.hist_at].clone();
        self.set(t);
        true
    }

    /// Шаг вперёд; в конце возвращает отложенный черновик.
    pub fn history_next(&mut self) -> bool {
        if self.hist_at >= self.history.len() {
            return false;
        }
        self.hist_at += 1;
        if self.hist_at == self.history.len() {
            let draft = std::mem::take(&mut self.hist_draft);
            self.set(draft);
        } else {
            let t = self.history[self.hist_at].clone();
            self.set(t);
        }
        true
    }

    /// Ходим ли мы сейчас по истории — от этого зависит, кому принадлежат
    /// стрелки вверх-вниз.
    pub fn in_history(&self) -> bool {
        self.hist_at < self.history.len()
    }

    pub fn backspace(&mut self) {
        let Some(prev) = self.prev_boundary(self.cursor) else {
            return;
        };
        self.snapshot();
        self.text.replace_range(prev..self.cursor, "");
        self.cursor = prev;
    }

    /// Delete: убрать символ ПОД курсором, не сдвигая его.
    pub fn delete(&mut self) {
        let Some(next) = self.next_boundary(self.cursor) else {
            return;
        };
        self.snapshot();
        self.text.replace_range(self.cursor..next, "");
    }

    pub fn left(&mut self) {
        if let Some(p) = self.prev_boundary(self.cursor) {
            self.cursor = p;
        }
    }

    pub fn right(&mut self) {
        if let Some(n) = self.next_boundary(self.cursor) {
            self.cursor = n;
        }
    }

    /// В начало текущей строки (а не всего текста): так ведёт себя любое поле.
    pub fn home(&mut self) {
        self.cursor = self.line_start(self.cursor);
    }

    pub fn end(&mut self) {
        self.cursor = self.line_end(self.cursor);
    }

    /// Вверх и вниз по строкам с сохранением колонки — как в любом редакторе.
    pub fn up(&mut self) {
        let (row, col) = self.row_col();
        if row == 0 {
            return;
        }
        self.go_to(row - 1, col);
    }

    pub fn down(&mut self) {
        let (row, col) = self.row_col();
        if row + 1 >= self.lines().len() {
            return;
        }
        self.go_to(row + 1, col);
    }

    /// Ctrl+U: стереть строку до курсора. Пустая строка — стереть всё.
    pub fn kill_line(&mut self) {
        self.snapshot();
        let start = self.line_start(self.cursor);
        if start == self.cursor {
            let all = std::mem::take(&mut self.text);
            self.cursor = 0;
            self.push_kill(all);
            return;
        }
        let dead = self.text[start..self.cursor].to_string();
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
        self.push_kill(dead);
    }

    /// Ctrl+K: стереть от курсора до конца строки.
    pub fn kill_to_end(&mut self) {
        let end = self.line_end(self.cursor);
        if end == self.cursor {
            return;
        }
        self.snapshot();
        let dead = self.text[self.cursor..end].to_string();
        self.text.replace_range(self.cursor..end, "");
        self.push_kill(dead);
    }

    pub fn lines(&self) -> Vec<&str> {
        self.text.split('\n').collect()
    }

    pub fn is_multiline(&self) -> bool {
        self.text.contains('\n')
    }

    /// Где курсор: строка и колонка В СИМВОЛАХ — рисовать его надо по видимым
    /// ячейкам, а не по байтам.
    pub fn row_col(&self) -> (usize, usize) {
        let before = &self.text[..self.cursor];
        let row = before.matches('\n').count();
        let col = before
            .rsplit('\n')
            .next()
            .map(|l| l.chars().count())
            .unwrap_or(0);
        (row, col)
    }

    fn go_to(&mut self, row: usize, col: usize) {
        let mut at = 0usize;
        for (i, line) in self.lines().iter().enumerate() {
            if i == row {
                // Колонка длиннее строки — встаём в её конец: так же ведёт
                // себя курсор в любом редакторе, и это ожидаемо.
                let take: usize = line
                    .chars()
                    .take(col)
                    .map(|c| c.len_utf8())
                    .sum::<usize>()
                    .min(line.len());
                self.cursor = at + take;
                return;
            }
            at += line.len() + 1; // сама строка и перевод после неё
        }
    }

    fn line_start(&self, at: usize) -> usize {
        self.text[..at].rfind('\n').map(|i| i + 1).unwrap_or(0)
    }

    fn line_end(&self, at: usize) -> usize {
        self.text[at..]
            .find('\n')
            .map(|i| at + i)
            .unwrap_or(self.text.len())
    }

    fn prev_boundary(&self, at: usize) -> Option<usize> {
        if at == 0 {
            return None;
        }
        let mut i = at - 1;
        while i > 0 && !self.text.is_char_boundary(i) {
            i -= 1;
        }
        Some(i)
    }

    fn next_boundary(&self, at: usize) -> Option<usize> {
        if at >= self.text.len() {
            return None;
        }
        let mut i = at + 1;
        while i < self.text.len() && !self.text.is_char_boundary(i) {
            i += 1;
        }
        Some(i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ed(text: &str) -> Editor {
        let mut e = Editor::default();
        e.set(text);
        e
    }

    /// Вставка приходит одним куском и часто многострочная. Если разбирать её
    /// по символам, каждый перевод строки выглядел бы нажатым Enter — то есть
    /// отправкой недописанного. Это главный ввод, который надо не сломать.
    #[test]
    fn paste_keeps_newlines_as_text() {
        let mut e = ed("начало ");
        e.paste("раз\nдва\r\nтри\r");
        assert_eq!(
            e.text(),
            "начало раз\nдва\nтри\n",
            "переводы строк потерялись или удвоились"
        );
        assert_eq!(e.lines().len(), 4);
        // Курсор — в конце вставленного, как в любом поле.
        assert_eq!(e.row_col(), (3, 0));
    }

    #[test]
    fn words_move_and_die_like_in_readline() {
        let mut e = ed("починить очередь слияний");
        e.word_left();
        assert_eq!(
            e.row_col(),
            (0, 17),
            "курсор встал не в начало последнего слова"
        );
        // Стираем СЛЕВА от курсора — то есть «очередь», а не слово под ним.
        e.kill_word_left();
        assert_eq!(e.text(), "починить слияний");

        let mut e = ed("починить очередь слияний");
        e.kill_word_left();
        assert_eq!(
            e.text(),
            "починить очередь ",
            "с конца уходит последнее слово"
        );

        // Знаки препинания — отдельное слово: foo(bar стирается по кусочкам.
        let mut e = ed("foo(bar");
        e.kill_word_left();
        assert_eq!(e.text(), "foo(");
        e.kill_word_left();
        assert_eq!(e.text(), "foo");
    }

    /// Стирание без возврата заставляет набирать заново — поэтому убитое
    /// живёт в кольце и возвращается.
    #[test]
    fn killed_text_comes_back() {
        let mut e = ed("почини очередь");
        e.kill_word_left();
        assert_eq!(e.text(), "почини ");
        assert!(e.yank());
        assert_eq!(e.text(), "почини очередь");
        // Пустое кольцо ничего не выдумывает.
        let mut fresh = ed("");
        assert!(!fresh.yank());
    }

    #[test]
    fn kill_to_end_and_to_start_both_work() {
        let mut e = ed("раз два");
        e.home();
        for _ in 0..4 {
            e.right();
        }
        e.kill_to_end();
        assert_eq!(e.text(), "раз ");
        e.kill_line();
        assert_eq!(e.text(), "");
    }

    /// Одно нажатие «стереть слово» не должно уносить работу безвозвратно.
    #[test]
    fn undo_returns_what_an_edit_took() {
        let mut e = ed("почини очередь слияний");
        e.kill_word_left();
        assert!(e.undo());
        assert_eq!(e.text(), "почини очередь слияний");
        e.insert('!');
        assert!(e.undo());
        assert_eq!(e.text(), "почини очередь слияний");
        // Пустая стопка не врёт про успех.
        let mut fresh = ed("");
        assert!(!fresh.undo());
    }

    /// История — то, ради чего в шелле жмут вверх: повторить прошлое, чуть
    /// изменив. Недописанное при этом не должно пропасть.
    #[test]
    fn history_walks_and_keeps_the_draft() {
        let mut e = ed("");
        e.remember("первое");
        e.remember("второе");
        e.set("черновик");
        assert!(e.history_prev());
        assert_eq!(e.text(), "второе");
        assert!(e.history_prev());
        assert_eq!(e.text(), "первое");
        assert!(!e.history_prev(), "дальше истории нет");
        assert!(e.history_next());
        assert_eq!(e.text(), "второе");
        assert!(e.history_next());
        assert_eq!(e.text(), "черновик", "недописанное вернулось");
        assert!(!e.history_next());
    }

    #[test]
    fn history_does_not_double_the_same_line() {
        let mut e = ed("");
        e.remember("да");
        e.remember("да");
        e.history_prev();
        assert_eq!(e.text(), "да");
        assert!(
            !e.history_prev(),
            "повтор подряд не должен занимать две строки"
        );
    }

    #[test]
    fn typing_and_moving_stay_on_character_boundaries() {
        let mut e = ed("привет");
        e.left();
        e.left();
        // Две «влево» — это две БУКВЫ назад, а не два байта: иначе курсор
        // встал бы в середину «е» и первая же вставка уронила бы программу.
        e.insert('!');
        assert_eq!(e.text(), "прив!ет");
        // Курсор остаётся после вставленного знака.
        e.insert('?');
        assert_eq!(e.text(), "прив!?ет");
    }

    #[test]
    fn backspace_and_delete_take_whole_letters() {
        let mut e = ed("да");
        e.backspace();
        assert_eq!(e.text(), "д", "стёрлась буква целиком, а не байт");
        let mut e = ed("даёт");
        e.home();
        e.right();
        e.delete();
        assert_eq!(e.text(), "дёт");
        // На краях ничего не ломается.
        let mut e = ed("");
        e.backspace();
        e.delete();
        assert_eq!(e.text(), "");
    }

    #[test]
    fn home_and_end_work_per_line_not_per_text() {
        let mut e = ed("первая\nвторая");
        e.home();
        assert_eq!(e.row_col(), (1, 0));
        e.end();
        assert_eq!(e.row_col(), (1, 6));
        e.up();
        e.home();
        assert_eq!(e.row_col(), (0, 0));
    }

    #[test]
    fn newline_splits_the_text_at_the_cursor() {
        let mut e = ed("разадва");
        e.home();
        for _ in 0..3 {
            e.right();
        }
        e.newline();
        assert_eq!(e.text(), "раз\nадва");
        assert_eq!(e.row_col(), (1, 0));
        assert!(e.is_multiline());
    }

    /// Вверх и вниз держат колонку, а на короткой строке встают в её конец.
    #[test]
    fn vertical_movement_keeps_the_column() {
        let mut e = ed("длинная строка\nкоротко\nещё одна длинная");
        e.home();
        for _ in 0..10 {
            e.right();
        }
        assert_eq!(e.row_col(), (2, 10));
        e.up();
        assert_eq!(
            e.row_col(),
            (1, 7),
            "колонка длиннее строки — встаём в конец"
        );
        e.up();
        assert_eq!(e.row_col(), (0, 7));
        e.down();
        e.down();
        assert_eq!(e.row_col(), (2, 7));
        // За края не уезжаем.
        e.down();
        e.down();
        assert_eq!(e.row_col().0, 2);
    }

    #[test]
    fn kill_line_clears_to_the_line_start_then_everything() {
        let mut e = ed("первая\nвторая");
        e.kill_line();
        assert_eq!(e.text(), "первая\n");
        e.kill_line();
        assert_eq!(e.text(), "", "второй раз — начисто");
    }

    #[test]
    fn empty_means_nothing_but_spaces() {
        assert!(ed("   \n ").is_empty());
        assert!(!ed(" x ").is_empty());
    }
}
