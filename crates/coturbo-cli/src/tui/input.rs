//! The text box at the bottom of the screen: multi-line editing with a
//! cursor, and its layout wrapped to the box width.

use unicode_width::UnicodeWidthChar;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Input {
    text: String,
    /// Byte offset of the cursor, always on a char boundary.
    cursor: usize,
}

impl Input {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Replaces the text, cursor at the end.
    pub fn set(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.cursor = self.text.len();
    }

    /// Empties the box, returning what was in it.
    pub fn take(&mut self) -> String {
        self.cursor = 0;
        std::mem::take(&mut self.text)
    }

    pub fn insert(&mut self, c: char) {
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    /// Inserts pasted text; Windows line endings become `\n`.
    pub fn insert_str(&mut self, s: &str) {
        let s = s.replace("\r\n", "\n").replace('\r', "\n");
        self.text.insert_str(self.cursor, &s);
        self.cursor += s.len();
    }

    fn prev(&self) -> Option<usize> {
        self.text[..self.cursor].char_indices().next_back().map(|(i, _)| i)
    }

    fn next(&self) -> Option<usize> {
        self.text[self.cursor..].chars().next().map(|c| self.cursor + c.len_utf8())
    }

    pub fn backspace(&mut self) {
        if let Some(i) = self.prev() {
            self.text.replace_range(i..self.cursor, "");
            self.cursor = i;
        }
    }

    pub fn delete(&mut self) {
        if let Some(i) = self.next() {
            self.text.replace_range(self.cursor..i, "");
        }
    }

    pub fn left(&mut self) {
        if let Some(i) = self.prev() {
            self.cursor = i;
        }
    }

    pub fn right(&mut self) {
        if let Some(i) = self.next() {
            self.cursor = i;
        }
    }

    /// Start of the previous word.
    pub fn word_left(&mut self) {
        let before = &self.text[..self.cursor];
        let trimmed = before.trim_end();
        self.cursor = trimmed.rfind(char::is_whitespace).map_or(0, |i| i + 1);
    }

    /// End of the next word.
    pub fn word_right(&mut self) {
        let after = &self.text[self.cursor..];
        let skip = after.len() - after.trim_start().len();
        let rest = &after[skip..];
        self.cursor += skip + rest.find(char::is_whitespace).unwrap_or(rest.len());
    }

    /// Deletes back to the start of the word (Ctrl+W, Ctrl+Backspace).
    pub fn delete_word(&mut self) {
        let end = self.cursor;
        self.word_left();
        self.text.replace_range(self.cursor..end, "");
    }

    fn line_start(&self) -> usize {
        self.text[..self.cursor].rfind('\n').map_or(0, |i| i + 1)
    }

    fn line_end(&self) -> usize {
        self.text[self.cursor..].find('\n').map_or(self.text.len(), |i| self.cursor + i)
    }

    pub fn home(&mut self) {
        self.cursor = self.line_start();
    }

    pub fn end(&mut self) {
        self.cursor = self.line_end();
    }

    /// Byte offset `col` chars into the line starting at `start`, clamped to its end.
    fn at_col(&self, start: usize, col: usize) -> usize {
        let line = &self.text[start..];
        let line = &line[..line.find('\n').unwrap_or(line.len())];
        start + line.char_indices().nth(col).map_or(line.len(), |(i, _)| i)
    }

    /// Moves to the line above; `false` on the first line.
    pub fn up(&mut self) -> bool {
        let start = self.line_start();
        if start == 0 {
            return false;
        }
        let col = self.text[start..self.cursor].chars().count();
        let prev_start = self.text[..start - 1].rfind('\n').map_or(0, |i| i + 1);
        self.cursor = self.at_col(prev_start, col);
        true
    }

    /// Moves to the line below; `false` on the last line.
    pub fn down(&mut self) -> bool {
        let end = self.line_end();
        if end == self.text.len() {
            return false;
        }
        let col = self.text[self.line_start()..self.cursor].chars().count();
        self.cursor = self.at_col(end + 1, col);
        true
    }

    /// The text wrapped to `width` cells, and the cursor's (row, column).
    /// `mask` shows every character as `•` (API keys).
    pub fn layout(&self, width: usize, mask: bool) -> (Vec<String>, (usize, usize)) {
        let width = width.max(1);
        let mut rows = vec![String::new()];
        let (mut col, mut at) = (0, None);
        for (i, c) in self.text.char_indices() {
            if i == self.cursor {
                at = Some((rows.len() - 1, col));
            }
            if c == '\n' {
                rows.push(String::new());
                col = 0;
                continue;
            }
            let shown = if mask { '•' } else { c };
            let w = shown.width().unwrap_or(0);
            if col + w > width {
                rows.push(String::new());
                col = 0;
                if i == self.cursor {
                    at = Some((rows.len() - 1, 0));
                }
            }
            rows.last_mut().expect("rows is never empty").push(shown);
            col += w;
        }
        let mut cursor = at.unwrap_or((rows.len() - 1, col));
        if cursor.1 >= width {
            rows.push(String::new());
            cursor = (rows.len() - 1, 0);
        }
        (rows, cursor)
    }
}
