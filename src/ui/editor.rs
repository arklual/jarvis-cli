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
}

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

    pub fn insert(&mut self, c: char) {
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    /// Перевод строки внутри сообщения: длинную мысль пишут в несколько строк,
    /// и Enter, отправляющий недописанное, — главный способ это испортить.
    pub fn newline(&mut self) {
        self.insert('\n');
    }

    pub fn backspace(&mut self) {
        let Some(prev) = self.prev_boundary(self.cursor) else {
            return;
        };
        self.text.replace_range(prev..self.cursor, "");
        self.cursor = prev;
    }

    /// Delete: убрать символ ПОД курсором, не сдвигая его.
    pub fn delete(&mut self) {
        let Some(next) = self.next_boundary(self.cursor) else {
            return;
        };
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
        let start = self.line_start(self.cursor);
        if start == self.cursor {
            self.clear();
            return;
        }
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
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
