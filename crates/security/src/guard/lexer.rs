//! Shell command-line lexers for the dangerous-action guard.
//!
//! The goal is **not** to execute shell grammar faithfully but to recover,
//! conservatively, every simple command a command line would run:
//!
//! * words with quotes removed (and whether they contained variables,
//!   substitutions or globs — "dynamic" targets are treated as unknown);
//! * command separators (`;`, `&&`, `||`, `&`, newlines) and pipelines;
//! * redirection targets (files written by `>`/`>>`);
//! * nested command text (`$(…)`, backticks, `<(…)`, PowerShell `$(…)`,
//!   `(…)`, `{…}` script blocks) which the guard classifies recursively;
//! * here-document bodies (so `psql <<EOF … EOF` SQL can be inspected).
//!
//! Anything the lexer cannot make sense of (unbalanced quotes/brackets)
//! yields [`LexError`], which the guard maps to `Dangerous(Unparseable)`
//! (fail closed).

/// Shell grammar family used for lexing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Family {
    /// bash / sh / zsh / dash / fish (approximately) / Git Bash / WSL.
    Posix,
    /// Windows PowerShell 5.1 and PowerShell 7.
    PowerShell,
    /// `cmd.exe`.
    Cmd,
}

/// One lexed word.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Word {
    /// Text with quotes removed (substitutions kept verbatim).
    pub text: String,
    /// Some part was quoted.
    pub quoted: bool,
    /// Contains a variable, command substitution or script block — the real
    /// value is unknown until runtime.
    pub dynamic: bool,
    /// Contains an unquoted glob character.
    pub glob: bool,
}

impl Word {
    /// Creates a literal word (tests, synthesized commands).
    #[must_use]
    pub fn literal(text: &str) -> Self {
        Self { text: text.to_owned(), ..Self::default() }
    }
}

/// A simple command: program + arguments, plus side information.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SimpleCommand {
    /// Program and arguments.
    pub words: Vec<Word>,
    /// Files written via output redirection.
    pub redirects: Vec<Word>,
    /// Here-document / here-string bodies fed to the command.
    pub heredocs: Vec<String>,
    /// Stdin comes from the previous command (pipeline stage > 0).
    pub piped: bool,
}

/// Result of lexing a command line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Parsed {
    /// Simple commands in order of appearance.
    pub commands: Vec<SimpleCommand>,
    /// Nested command texts (substitutions, script blocks) to classify recursively.
    pub nested: Vec<String>,
}

/// Lexing failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LexError {
    /// Unterminated quote or bracket.
    #[error("unbalanced {0}")]
    Unbalanced(&'static str),
}

/// Lexes `input` according to `family`.
pub fn parse(input: &str, family: Family) -> Result<Parsed, LexError> {
    let mut lx = Lexer::new(input, family);
    match family {
        Family::Posix => lx.run_posix()?,
        Family::PowerShell => lx.run_powershell()?,
        Family::Cmd => lx.run_cmd()?,
    }
    lx.end_cmd();
    Ok(lx.out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    None,
    /// Next word is an output redirection target.
    Redirect,
    /// Next word is an input/ignored redirection operand.
    Ignore,
    /// Next word is a here-document delimiter (`<<-` strips tabs).
    HeredocDelim { strip_tabs: bool },
}

struct Lexer {
    chars: Vec<char>,
    i: usize,
    family: Family,
    out: Parsed,
    cur_cmd: SimpleCommand,
    cur_word: Option<Word>,
    pending: Pending,
    next_piped: bool,
    /// Delimiters requested by the current command.
    cur_heredocs: Vec<(String, bool)>,
    /// (command index, delimiter, strip tabs) waiting for the next newline.
    heredoc_queue: Vec<(usize, String, bool)>,
}

const POSIX_KEYWORDS: &[&str] =
    &["if", "then", "else", "elif", "fi", "do", "done", "while", "until", "!", "{", "}", "(", ")", "case", "esac", "in"];

impl Lexer {
    fn new(input: &str, family: Family) -> Self {
        Self {
            chars: input.chars().collect(),
            i: 0,
            family,
            out: Parsed::default(),
            cur_cmd: SimpleCommand::default(),
            cur_word: None,
            pending: Pending::None,
            next_piped: false,
            cur_heredocs: Vec::new(),
            heredoc_queue: Vec::new(),
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.i).copied()
    }

    fn peek_at(&self, off: usize) -> Option<char> {
        self.chars.get(self.i + off).copied()
    }

    fn word(&mut self) -> &mut Word {
        self.cur_word.get_or_insert_with(Word::default)
    }

    fn push(&mut self, c: char) {
        self.word().text.push(c);
    }

    fn push_str(&mut self, s: &str) {
        self.word().text.push_str(s);
    }

    fn end_word(&mut self) {
        let Some(w) = self.cur_word.take() else { return };
        match std::mem::replace(&mut self.pending, Pending::None) {
            Pending::Redirect => self.cur_cmd.redirects.push(w),
            Pending::Ignore => {}
            Pending::HeredocDelim { strip_tabs } => self.cur_heredocs.push((w.text, strip_tabs)),
            Pending::None => {
                // Drop grammar keywords at command start (`do rm -rf x` → `rm -rf x`).
                if self.cur_cmd.words.is_empty()
                    && !w.quoted
                    && self.family == Family::Posix
                    && POSIX_KEYWORDS.contains(&w.text.as_str())
                {
                    return;
                }
                self.cur_cmd.words.push(w);
            }
        }
    }

    fn end_cmd(&mut self) {
        self.end_word();
        self.pending = Pending::None;
        let mut cmd = std::mem::take(&mut self.cur_cmd);
        let heredocs = std::mem::take(&mut self.cur_heredocs);
        if cmd.words.is_empty() && cmd.redirects.is_empty() && heredocs.is_empty() {
            return;
        }
        cmd.piped = std::mem::take(&mut self.next_piped);
        let idx = self.out.commands.len();
        for (d, s) in heredocs {
            self.heredoc_queue.push((idx, d, s));
        }
        self.out.commands.push(cmd);
    }

    fn pipe(&mut self) {
        self.end_cmd();
        self.next_piped = true;
    }

    /// Consumes here-document bodies after a newline (POSIX).
    fn consume_heredocs(&mut self) {
        let queue = std::mem::take(&mut self.heredoc_queue);
        for (idx, delim, strip_tabs) in queue {
            let mut body = String::new();
            loop {
                if self.i >= self.chars.len() {
                    break;
                }
                let start = self.i;
                while self.i < self.chars.len() && self.chars[self.i] != '\n' {
                    self.i += 1;
                }
                let line: String = self.chars[start..self.i].iter().collect();
                if self.i < self.chars.len() {
                    self.i += 1; // newline
                }
                let cmp = if strip_tabs { line.trim_start_matches('\t') } else { line.as_str() };
                if cmp.trim_end_matches('\r') == delim {
                    break;
                }
                body.push_str(&line);
                body.push('\n');
            }
            if let Some(cmd) = self.out.commands.get_mut(idx) {
                cmd.heredocs.push(body);
            }
        }
    }

    /// Captures text up to the matching `close`, starting right after `open`.
    /// Quotes inside are skipped. Returns the inner text.
    fn capture_balanced(&mut self, open: char, close: char) -> Result<String, LexError> {
        let mut depth = 1usize;
        let mut inner = String::new();
        while let Some(c) = self.peek() {
            self.i += 1;
            match c {
                '\'' if self.family != Family::Cmd => {
                    inner.push(c);
                    loop {
                        let Some(q) = self.peek() else { return Err(LexError::Unbalanced("quote")) };
                        self.i += 1;
                        inner.push(q);
                        if q == '\'' {
                            break;
                        }
                    }
                }
                '"' => {
                    inner.push(c);
                    loop {
                        let Some(q) = self.peek() else { return Err(LexError::Unbalanced("quote")) };
                        self.i += 1;
                        inner.push(q);
                        if (q == '\\' && self.family == Family::Posix) || (q == '`' && self.family == Family::PowerShell)
                        {
                            if let Some(n) = self.peek() {
                                self.i += 1;
                                inner.push(n);
                            }
                            continue;
                        }
                        if q == '"' {
                            break;
                        }
                    }
                }
                '\\' if self.family == Family::Posix => {
                    inner.push(c);
                    if let Some(n) = self.peek() {
                        self.i += 1;
                        inner.push(n);
                    }
                }
                '`' if self.family == Family::PowerShell => {
                    inner.push(c);
                    if let Some(n) = self.peek() {
                        self.i += 1;
                        inner.push(n);
                    }
                }
                _ if c == close => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(inner);
                    }
                    inner.push(c);
                }
                _ if c == open => {
                    depth += 1;
                    inner.push(c);
                }
                _ => inner.push(c),
            }
        }
        Err(LexError::Unbalanced(match close {
            ')' => "parenthesis",
            '}' => "brace",
            ']' => "bracket",
            _ => "delimiter",
        }))
    }

    fn skip_to_eol(&mut self) {
        while let Some(c) = self.peek() {
            if c == '\n' {
                break;
            }
            self.i += 1;
        }
    }

    /// Removes a pure file-descriptor number word before a redirection (`2>`).
    fn drop_fd_word(&mut self) {
        if let Some(w) = &self.cur_word {
            if !w.quoted && !w.text.is_empty() && w.text.chars().all(|c| c.is_ascii_digit() || c == '*') {
                self.cur_word = None;
            }
        }
        self.end_word();
    }

    /// Handles `>`, `>>`, `>|`, `>&N` at the cursor (cursor on `>`).
    fn output_redirect(&mut self) {
        self.drop_fd_word();
        self.i += 1;
        while matches!(self.peek(), Some('>') | Some('|')) {
            self.i += 1;
        }
        if self.peek() == Some('&') {
            // fd duplication (`2>&1`, `>&2`): the operand is a descriptor.
            self.i += 1;
            self.pending = Pending::Ignore;
            return;
        }
        self.pending = Pending::Redirect;
    }

    // ------------------------------------------------------------------
    // POSIX
    // ------------------------------------------------------------------

    fn posix_dollar(&mut self) -> Result<(), LexError> {
        // cursor on '$'
        match self.peek_at(1) {
            Some('(') => {
                self.i += 2;
                let inner = self.capture_balanced('(', ')')?;
                let inner_cmd = inner.strip_prefix('(').and_then(|s| s.strip_suffix(')')).unwrap_or(&inner);
                self.out.nested.push(inner_cmd.to_owned());
                self.push_str(&format!("$({inner})"));
                self.word().dynamic = true;
            }
            Some('{') => {
                self.i += 2;
                let inner = self.capture_balanced('{', '}')?;
                self.push_str(&format!("${{{inner}}}"));
                self.word().dynamic = true;
            }
            Some(c) if c.is_ascii_alphanumeric() || "_@*#?$!-".contains(c) => {
                self.i += 1;
                let mut name = String::from("$");
                if c.is_ascii_alphabetic() || c == '_' {
                    while let Some(n) = self.peek() {
                        if n.is_ascii_alphanumeric() || n == '_' {
                            name.push(n);
                            self.i += 1;
                        } else {
                            break;
                        }
                    }
                } else {
                    name.push(c);
                    self.i += 1;
                }
                self.push_str(&name);
                self.word().dynamic = true;
            }
            _ => {
                self.i += 1;
                self.push('$');
            }
        }
        Ok(())
    }

    fn posix_backtick(&mut self) -> Result<(), LexError> {
        self.i += 1;
        let mut inner = String::new();
        loop {
            let Some(c) = self.peek() else { return Err(LexError::Unbalanced("backtick")) };
            self.i += 1;
            match c {
                '`' => break,
                '\\' => {
                    if let Some(n) = self.peek() {
                        self.i += 1;
                        inner.push(n);
                    }
                }
                _ => inner.push(c),
            }
        }
        self.out.nested.push(inner.clone());
        self.push_str(&format!("`{inner}`"));
        self.word().dynamic = true;
        Ok(())
    }

    fn run_posix(&mut self) -> Result<(), LexError> {
        while let Some(c) = self.peek() {
            match c {
                ' ' | '\t' | '\r' => {
                    self.end_word();
                    self.i += 1;
                }
                '\n' => {
                    self.end_cmd();
                    self.i += 1;
                    self.consume_heredocs();
                }
                ';' => {
                    self.end_cmd();
                    self.i += 1;
                }
                '&' => {
                    if self.peek_at(1) == Some('&') {
                        self.end_cmd();
                        self.i += 2;
                    } else if self.peek_at(1) == Some('>') {
                        // `&>file` redirect both streams.
                        self.end_word();
                        self.i += 1;
                        self.output_redirect();
                    } else {
                        self.end_cmd();
                        self.i += 1;
                    }
                }
                '|' => {
                    if self.peek_at(1) == Some('|') {
                        self.end_cmd();
                        self.i += 2;
                    } else {
                        self.pipe();
                        self.i += 1;
                        if self.peek() == Some('&') {
                            self.i += 1;
                        }
                    }
                }
                '(' => {
                    if self.cur_word.is_some() {
                        // `name()` function definition or `=(` array: keep literal.
                        self.push('(');
                    } else {
                        self.end_cmd();
                    }
                    self.i += 1;
                }
                ')' => {
                    if self.cur_word.as_ref().is_some_and(|w| w.text.ends_with('(')) {
                        self.push(')');
                    } else {
                        self.end_cmd();
                    }
                    self.i += 1;
                }
                '#' if self.cur_word.is_none() => self.skip_to_eol(),
                '\'' => {
                    self.i += 1;
                    self.word().quoted = true;
                    loop {
                        let Some(q) = self.peek() else { return Err(LexError::Unbalanced("single quote")) };
                        self.i += 1;
                        if q == '\'' {
                            break;
                        }
                        self.push(q);
                    }
                }
                '"' => {
                    self.i += 1;
                    self.word().quoted = true;
                    loop {
                        let Some(q) = self.peek() else { return Err(LexError::Unbalanced("double quote")) };
                        match q {
                            '"' => {
                                self.i += 1;
                                break;
                            }
                            '\\' => {
                                self.i += 1;
                                if let Some(n) = self.peek() {
                                    self.i += 1;
                                    match n {
                                        '"' | '\\' | '$' | '`' => self.push(n),
                                        '\n' => {}
                                        _ => {
                                            self.push('\\');
                                            self.push(n);
                                        }
                                    }
                                }
                            }
                            '$' => self.posix_dollar()?,
                            '`' => self.posix_backtick()?,
                            _ => {
                                self.i += 1;
                                self.push(q);
                            }
                        }
                    }
                }
                '\\' => {
                    self.i += 1;
                    match self.peek() {
                        Some('\n') => self.i += 1,
                        Some(n) => {
                            self.i += 1;
                            self.push(n);
                            self.word().quoted = true;
                        }
                        None => {}
                    }
                }
                '`' => self.posix_backtick()?,
                '$' => self.posix_dollar()?,
                '<' | '>' if self.peek_at(1) == Some('(') => {
                    // process substitution
                    self.i += 2;
                    let inner = self.capture_balanced('(', ')')?;
                    self.out.nested.push(inner.clone());
                    self.push_str(&format!("{c}({inner})"));
                    self.word().dynamic = true;
                }
                '>' => self.output_redirect(),
                '<' => {
                    self.drop_fd_word();
                    self.i += 1;
                    if self.peek() == Some('<') {
                        self.i += 1;
                        if self.peek() == Some('<') {
                            // here-string: operand is data
                            self.i += 1;
                            self.pending = Pending::Ignore;
                        } else {
                            let strip_tabs = if self.peek() == Some('-') {
                                self.i += 1;
                                true
                            } else {
                                false
                            };
                            while matches!(self.peek(), Some(' ') | Some('\t')) {
                                self.i += 1;
                            }
                            self.pending = Pending::HeredocDelim { strip_tabs };
                        }
                    } else {
                        if self.peek() == Some('&') || self.peek() == Some('>') {
                            self.i += 1;
                        }
                        self.pending = Pending::Ignore;
                    }
                }
                '*' | '?' | '[' => {
                    self.push(c);
                    self.word().glob = true;
                    self.i += 1;
                }
                _ => {
                    self.push(c);
                    self.i += 1;
                }
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // PowerShell
    // ------------------------------------------------------------------

    fn ps_dollar(&mut self) -> Result<(), LexError> {
        match self.peek_at(1) {
            Some('(') => {
                self.i += 2;
                let inner = self.capture_balanced('(', ')')?;
                self.out.nested.push(inner.clone());
                self.push_str(&format!("$({inner})"));
                self.word().dynamic = true;
            }
            Some('{') => {
                self.i += 2;
                let inner = self.capture_balanced('{', '}')?;
                self.push_str(&format!("${{{inner}}}"));
                self.word().dynamic = true;
            }
            Some(c) if c.is_alphanumeric() || c == '_' || c == '?' || c == '$' || c == '^' => {
                self.i += 1;
                let mut name = String::from("$");
                while let Some(n) = self.peek() {
                    if n.is_alphanumeric() || n == '_' || n == ':' || n == '?' || n == '^' || n == '$' {
                        name.push(n);
                        self.i += 1;
                    } else {
                        break;
                    }
                }
                self.push_str(&name);
                self.word().dynamic = true;
            }
            _ => {
                self.i += 1;
                self.push('$');
            }
        }
        Ok(())
    }

    fn run_powershell(&mut self) -> Result<(), LexError> {
        while let Some(c) = self.peek() {
            match c {
                ' ' | '\t' | '\r' => {
                    self.end_word();
                    self.i += 1;
                }
                '\n' | ';' => {
                    self.end_cmd();
                    self.i += 1;
                }
                '|' => {
                    if self.peek_at(1) == Some('|') {
                        self.end_cmd();
                        self.i += 2;
                    } else {
                        self.pipe();
                        self.i += 1;
                    }
                }
                '&' => {
                    if self.peek_at(1) == Some('&') {
                        self.end_cmd();
                        self.i += 2;
                    } else if self.cur_word.is_none() && self.cur_cmd.words.is_empty() {
                        // call operator `& cmd` / `& { … }`
                        self.i += 1;
                    } else {
                        self.end_cmd();
                        self.i += 1;
                    }
                }
                '#' if self.cur_word.is_none() => self.skip_to_eol(),
                '<' if self.peek_at(1) == Some('#') => {
                    self.i += 2;
                    loop {
                        match self.peek() {
                            None => return Err(LexError::Unbalanced("block comment")),
                            Some('#') if self.peek_at(1) == Some('>') => {
                                self.i += 2;
                                break;
                            }
                            Some(_) => self.i += 1,
                        }
                    }
                }
                '\'' | '\u{2018}' | '\u{2019}' => {
                    self.i += 1;
                    self.word().quoted = true;
                    loop {
                        let Some(q) = self.peek() else { return Err(LexError::Unbalanced("single quote")) };
                        self.i += 1;
                        if matches!(q, '\'' | '\u{2018}' | '\u{2019}') {
                            if matches!(self.peek(), Some('\'')) {
                                self.i += 1;
                                self.push('\'');
                                continue;
                            }
                            break;
                        }
                        self.push(q);
                    }
                }
                '"' | '\u{201C}' | '\u{201D}' => {
                    self.i += 1;
                    self.word().quoted = true;
                    loop {
                        let Some(q) = self.peek() else { return Err(LexError::Unbalanced("double quote")) };
                        match q {
                            '"' | '\u{201C}' | '\u{201D}' => {
                                self.i += 1;
                                if self.peek() == Some('"') {
                                    self.i += 1;
                                    self.push('"');
                                    continue;
                                }
                                break;
                            }
                            '`' => {
                                self.i += 1;
                                if let Some(n) = self.peek() {
                                    self.i += 1;
                                    self.push(n);
                                }
                            }
                            '$' => self.ps_dollar()?,
                            _ => {
                                self.i += 1;
                                self.push(q);
                            }
                        }
                    }
                }
                '@' if matches!(self.peek_at(1), Some('"') | Some('\'')) => {
                    // here-string @" … "@
                    let quote = self.peek_at(1).unwrap_or('"');
                    self.i += 2;
                    let mut body = String::new();
                    loop {
                        match self.peek() {
                            None => return Err(LexError::Unbalanced("here-string")),
                            Some('\n') if self.peek_at(1) == Some(quote) && self.peek_at(2) == Some('@') => {
                                self.i += 3;
                                break;
                            }
                            Some(ch) => {
                                body.push(ch);
                                self.i += 1;
                            }
                        }
                    }
                    let dynamic = quote == '"' && body.contains('$');
                    let w = self.word();
                    w.text.push_str(body.trim_start_matches(['\r', '\n']));
                    w.quoted = true;
                    w.dynamic |= dynamic;
                }
                '@' if matches!(self.peek_at(1), Some('(') | Some('{')) => {
                    let open = self.peek_at(1).unwrap_or('(');
                    let close = if open == '(' { ')' } else { '}' };
                    self.i += 2;
                    let inner = self.capture_balanced(open, close)?;
                    self.out.nested.push(inner.clone());
                    self.push_str(&format!("@{open}{inner}{close}"));
                    self.word().dynamic = true;
                }
                '`' => {
                    self.i += 1;
                    match self.peek() {
                        Some('\n') => self.i += 1,
                        Some('\r') if self.peek_at(1) == Some('\n') => self.i += 2,
                        Some(n) => {
                            self.i += 1;
                            self.push(n);
                        }
                        None => {}
                    }
                }
                '$' => self.ps_dollar()?,
                '{' => {
                    self.i += 1;
                    let inner = self.capture_balanced('{', '}')?;
                    self.out.nested.push(inner.clone());
                    self.push_str(&format!("{{{inner}}}"));
                    self.word().dynamic = true;
                }
                '(' => {
                    self.i += 1;
                    let inner = self.capture_balanced('(', ')')?;
                    self.out.nested.push(inner.clone());
                    self.push_str(&format!("({inner})"));
                    self.word().dynamic = true;
                }
                ')' | '}' => return Err(LexError::Unbalanced("closing bracket")),
                '[' if self.cur_word.is_none() => {
                    // type literal `[IO.Directory]::Delete(…)`
                    self.i += 1;
                    let inner = self.capture_balanced('[', ']')?;
                    self.push_str(&format!("[{inner}]"));
                }
                '>' => self.output_redirect(),
                '*' | '?' | '[' => {
                    self.push(c);
                    self.word().glob = true;
                    self.i += 1;
                }
                _ => {
                    self.push(c);
                    self.i += 1;
                }
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // cmd.exe
    // ------------------------------------------------------------------

    fn run_cmd(&mut self) -> Result<(), LexError> {
        while let Some(c) = self.peek() {
            match c {
                ' ' | '\t' | '\r' | ',' | ';' => {
                    self.end_word();
                    self.i += 1;
                }
                '\n' => {
                    self.end_cmd();
                    self.i += 1;
                }
                '&' => {
                    self.end_cmd();
                    self.i += if self.peek_at(1) == Some('&') { 2 } else { 1 };
                }
                '|' => {
                    if self.peek_at(1) == Some('|') {
                        self.end_cmd();
                        self.i += 2;
                    } else {
                        self.pipe();
                        self.i += 1;
                    }
                }
                '"' => {
                    // cmd tolerates an unterminated quote (runs to end of line).
                    self.i += 1;
                    self.word().quoted = true;
                    while let Some(q) = self.peek() {
                        if q == '\n' {
                            break;
                        }
                        self.i += 1;
                        if q == '"' {
                            break;
                        }
                        if q == '%' {
                            self.word().dynamic = true;
                        }
                        self.push(q);
                    }
                }
                '^' => {
                    self.i += 1;
                    match self.peek() {
                        Some('\n') => self.i += 1,
                        Some(n) => {
                            self.i += 1;
                            self.push(n);
                        }
                        None => {}
                    }
                }
                '%' => {
                    self.i += 1;
                    self.push('%');
                    // %VAR% or %%i / %1
                    let start = self.i;
                    let mut j = self.i;
                    while j < self.chars.len() && self.chars[j] != '%' && !self.chars[j].is_whitespace() {
                        j += 1;
                    }
                    if j < self.chars.len() && self.chars[j] == '%' && j > start {
                        let name: String = self.chars[start..=j].iter().collect();
                        self.push_str(&name);
                        self.i = j + 1;
                    }
                    self.word().dynamic = true;
                }
                '!' => {
                    self.push('!');
                    self.i += 1;
                    self.word().dynamic = true;
                }
                '(' if self.cur_word.is_none() => {
                    self.end_cmd();
                    self.i += 1;
                }
                ')' if self.cur_word.is_none() => {
                    self.end_cmd();
                    self.i += 1;
                }
                '@' if self.cur_word.is_none() && self.cur_cmd.words.is_empty() => self.i += 1,
                '>' => self.output_redirect(),
                '<' => {
                    self.drop_fd_word();
                    self.i += 1;
                    self.pending = Pending::Ignore;
                }
                '*' | '?' => {
                    self.push(c);
                    self.word().glob = true;
                    self.i += 1;
                }
                _ => {
                    self.push(c);
                    self.i += 1;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(p: &Parsed, i: usize) -> Vec<String> {
        p.commands[i].words.iter().map(|w| w.text.clone()).collect()
    }

    #[test]
    fn posix_splitting_and_quotes() {
        let p = parse(r#"echo 'a b' "c $HOME" && rm -rf x; ls | grep y"#, Family::Posix).unwrap();
        assert_eq!(p.commands.len(), 4);
        assert_eq!(words(&p, 0), vec!["echo", "a b", "c $HOME"]);
        assert!(p.commands[0].words[2].dynamic);
        assert_eq!(words(&p, 1), vec!["rm", "-rf", "x"]);
        assert!(p.commands[3].piped);
    }

    #[test]
    fn posix_redirects_and_nested() {
        let p = parse("echo $(curl x) > out.txt 2>&1", Family::Posix).unwrap();
        assert_eq!(p.commands[0].redirects[0].text, "out.txt");
        assert_eq!(p.commands[0].redirects.len(), 1);
        assert_eq!(p.nested, vec!["curl x"]);
    }

    #[test]
    fn posix_heredoc() {
        let p = parse("psql <<EOF\nDROP TABLE x;\nEOF\necho done", Family::Posix).unwrap();
        assert_eq!(p.commands.len(), 2);
        assert_eq!(p.commands[0].heredocs[0], "DROP TABLE x;\n");
        assert_eq!(words(&p, 1), vec!["echo", "done"]);
    }

    #[test]
    fn posix_unbalanced() {
        assert!(parse("echo 'oops", Family::Posix).is_err());
        assert!(parse("echo $(ls", Family::Posix).is_err());
    }

    #[test]
    fn powershell_basics() {
        let p = parse("Remove-Item -Recurse -Force .\\dist; iwr https://x | iex", Family::PowerShell).unwrap();
        assert_eq!(words(&p, 0), vec!["Remove-Item", "-Recurse", "-Force", ".\\dist"]);
        assert_eq!(words(&p, 1), vec!["iwr", "https://x"]);
        assert!(p.commands[2].piped);
        let p = parse("& { rm -r x }", Family::PowerShell).unwrap();
        assert_eq!(p.nested, vec![" rm -r x "]);
        let p = parse("Write-Host 'it''s'", Family::PowerShell).unwrap();
        assert_eq!(words(&p, 0)[1], "it's");
    }

    #[test]
    fn cmd_basics() {
        let p = parse("cd build && rd /s /q out & del \"a b.txt\"", Family::Cmd).unwrap();
        assert_eq!(p.commands.len(), 3);
        assert_eq!(words(&p, 1), vec!["rd", "/s", "/q", "out"]);
        assert_eq!(words(&p, 2), vec!["del", "a b.txt"]);
        let p = parse("echo %USERPROFILE% > nul", Family::Cmd).unwrap();
        assert!(p.commands[0].words[1].dynamic);
        assert_eq!(p.commands[0].redirects[0].text, "nul");
    }
}
