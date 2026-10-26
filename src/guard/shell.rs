//! Just enough of a POSIX shell parser to see what a command line runs.
//!
//! The old policy matched the first word only, so `ls; rm -rf /` passed an
//! allowlist that held `ls`. This splits a line into the simple commands it is
//! made of (`;` `&&` `||` `|` `&` newline, parentheses), pulls out what runs
//! inside `$(…)`, backticks and `<(…)`, and records every redirect, so a rule
//! can be applied to each part.
//!
//! It does not expand anything. A word that only a running shell could know
//! (`$CMD`, `${x}`) stays as written, and the rules treat it as unknown.

/// One redirect, e.g. `> /etc/hosts` is `{ op: ">", target: "/etc/hosts" }`.
#[derive(Debug, Clone, PartialEq)]
pub struct Redirect {
    pub op: String,
    pub target: String,
}

impl Redirect {
    /// Whether this writes to a file (as opposed to reading one, or pointing
    /// one descriptor at another such as `2>&1`).
    pub fn writes_file(&self) -> bool {
        let writes = self.op.contains('>') && !self.op.starts_with('<');
        if !writes || self.op.ends_with('&') {
            return false;
        }
        !matches!(
            self.target.as_str(),
            "/dev/null" | "/dev/stdout" | "/dev/stderr" | "/dev/tty"
        )
    }
}

/// A command line taken apart.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Parsed {
    /// Each simple command as its words, quotes removed.
    pub segments: Vec<Vec<String>>,
    /// Per word of `segments`: `false` when the shell would change it before
    /// running — a variable, a substitution, `$'…'`, a glob or brace pattern
    /// outside quotes. Such a word can turn into `-delete` or `/etc/shadow`.
    pub literal: Vec<Vec<bool>>,
    /// The text inside every `$(…)`, backtick pair, `<(…)` and `>(…)`.
    pub substitutions: Vec<String>,
    pub redirects: Vec<Redirect>,
    /// A `<<` here-document: its body is on the following lines and was not read.
    pub heredoc: bool,
    /// A shell function definition (`f() { … }`), which is how a fork bomb starts.
    pub defines_function: bool,
    /// A `#` comment: text that never runs but that a model reviewing the line reads.
    pub has_comment: bool,
    /// Why the line could not be read to the end (an unclosed quote, …).
    pub error: Option<String>,
}

/// Collapse runs of whitespace between words, so `ls   -la` and `ls -la` are
/// one command, but leave quoted text exactly as written: `'a  b'` is not `'a b'`.
pub fn normalize(command: &str) -> String {
    let mut out = String::new();
    let mut quote: Option<char> = None;
    let mut pending_space = false;
    let mut chars = command.trim().chars();
    while let Some(c) = chars.next() {
        if quote.is_none() && c.is_whitespace() {
            pending_space = true;
            continue;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        out.push(c);
        match (quote, c) {
            (None, '\\') | (Some('"'), '\\') => {
                if let Some(n) = chars.next() {
                    out.push(n);
                }
            }
            (None, '\'' | '"') => quote = Some(c),
            (Some(q), _) if c == q => quote = None,
            _ => {}
        }
    }
    out
}

pub fn parse(command: &str) -> Parsed {
    let mut p = Parser {
        chars: command.chars().collect(),
        pos: 0,
        out: Parsed::default(),
        words: Vec::new(),
        word: None,
        flags: Vec::new(),
        dynamic: false,
    };
    p.run();
    p.out
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
    out: Parsed,
    words: Vec<String>,
    /// The word being read; `None` between words (so `""` is still a word).
    word: Option<String>,
    /// `literal` flags of `words`.
    flags: Vec<bool>,
    /// The word being read will be expanded by the shell.
    dynamic: bool,
}

impl Parser {
    fn peek(&self, ahead: usize) -> Option<char> {
        self.chars.get(self.pos + ahead).copied()
    }

    fn push_char(&mut self, c: char) {
        self.word.get_or_insert_with(String::new).push(c);
    }

    /// A character the shell expands, read outside single quotes.
    fn push_dynamic(&mut self, c: char) {
        self.dynamic = true;
        self.push_char(c);
    }

    fn end_word(&mut self) {
        if let Some(w) = self.word.take() {
            self.words.push(w);
            self.flags.push(!self.dynamic);
        }
        self.dynamic = false;
    }

    fn end_segment(&mut self) {
        self.end_word();
        if !self.words.is_empty() {
            if self.words.first().map(String::as_str) == Some("function") {
                self.out.defines_function = true;
            }
            self.out.segments.push(std::mem::take(&mut self.words));
            self.out.literal.push(std::mem::take(&mut self.flags));
        }
    }

    fn fail(&mut self, why: &str) {
        if self.out.error.is_none() {
            self.out.error = Some(why.to_string());
        }
    }

    fn run(&mut self) {
        while let Some(c) = self.peek(0) {
            match c {
                ' ' | '\t' => {
                    self.end_word();
                    self.pos += 1;
                }
                '\n' | ';' | '(' | ')' | '{' | '}'
                    if !(matches!(c, '{' | '}') && self.word.is_some()) =>
                {
                    // `{`/`}` group only when they stand alone; `${x}` and
                    // `a{b,c}` are part of a word.
                    if c == '(' && self.word.is_some() && self.peek(1) == Some(')') {
                        // `name()` — a function definition.
                        if self.words.is_empty() {
                            self.out.defines_function = true;
                        }
                        self.push_char('(');
                        self.push_char(')');
                        self.end_word();
                        self.pos += 2;
                        continue;
                    }
                    self.end_segment();
                    self.pos += 1;
                }
                '&' => {
                    if self.peek(1) == Some('>') {
                        // `&>file` / `&>>file`: stdout and stderr to a file.
                        self.end_word();
                        self.redirect();
                    } else {
                        self.end_segment();
                        self.pos += if self.peek(1) == Some('&') { 2 } else { 1 };
                    }
                }
                '|' => {
                    self.end_segment();
                    self.pos += if matches!(self.peek(1), Some('|') | Some('&')) { 2 } else { 1 };
                }
                '>' | '<' => {
                    if matches!(self.peek(1), Some('(')) {
                        // Process substitution `<(cmd)` / `>(cmd)`.
                        self.pos += 2;
                        let inner = self.until_close_paren();
                        self.out.substitutions.push(inner);
                        self.push_dynamic('_');
                        continue;
                    }
                    // A word of digits right before is the descriptor (`2>`).
                    if self.word.as_deref().is_some_and(|w| w.chars().all(|d| d.is_ascii_digit())) {
                        self.word = None;
                        self.dynamic = false;
                    } else {
                        self.end_word();
                    }
                    self.redirect();
                }
                '#' if self.word.is_none() => {
                    // A comment runs to the end of the line.
                    self.out.has_comment = true;
                    while let Some(c) = self.peek(0) {
                        if c == '\n' {
                            break;
                        }
                        self.pos += 1;
                    }
                }
                '\\' => {
                    if let Some(next) = self.peek(1) {
                        if next != '\n' {
                            self.push_char(next);
                        }
                        self.pos += 2;
                    } else {
                        self.pos += 1;
                    }
                }
                '\'' => {
                    self.pos += 1;
                    let start = self.pos;
                    while self.peek(0).is_some_and(|c| c != '\'') {
                        self.pos += 1;
                    }
                    if self.peek(0).is_none() {
                        self.fail("unclosed single quote");
                    }
                    let s: String = self.chars[start..self.pos].iter().collect();
                    self.word.get_or_insert_with(String::new).push_str(&s);
                    self.pos += 1;
                }
                '"' => {
                    self.pos += 1;
                    self.word.get_or_insert_with(String::new);
                    loop {
                        match self.peek(0) {
                            None => {
                                self.fail("unclosed double quote");
                                break;
                            }
                            Some('"') => {
                                self.pos += 1;
                                break;
                            }
                            Some('\\') => {
                                if let Some(n) = self.peek(1) {
                                    if !matches!(n, '"' | '\\' | '$' | '`') {
                                        self.push_char('\\');
                                    }
                                    self.push_char(n);
                                }
                                self.pos += 2;
                            }
                            Some('$') if self.peek(1) == Some('(') => self.dollar(),
                            Some('`') => self.backtick(),
                            Some('$') => {
                                // `"$X"` is still expanded.
                                self.push_dynamic('$');
                                self.pos += 1;
                            }
                            Some(c) => {
                                self.push_char(c);
                                self.pos += 1;
                            }
                        }
                    }
                }
                '$' if self.peek(1) == Some('(') => self.dollar(),
                '`' => self.backtick(),
                // A variable, `$'…'`, and glob or brace patterns: the shell
                // rewrites the word before the program sees it.
                '$' | '*' | '?' | '[' | '{' | '}' => {
                    self.push_dynamic(c);
                    self.pos += 1;
                }
                _ => {
                    self.push_char(c);
                    self.pos += 1;
                }
            }
        }
        self.end_segment();
    }

    /// At `$(`: arithmetic `$((…))` is kept as text, a command is recorded.
    fn dollar(&mut self) {
        if self.peek(2) == Some('(') {
            self.pos += 3;
            let inner = self.until_close_paren();
            // The second `)` of `))`.
            if self.peek(0) == Some(')') {
                self.pos += 1;
            }
            self.dynamic = true;
            self.word
                .get_or_insert_with(String::new)
                .push_str(&format!("$(({inner}))"));
            return;
        }
        self.pos += 2;
        let inner = self.until_close_paren();
        self.out.substitutions.push(inner);
        // The word's value is unknown until the shell runs it.
        self.push_dynamic('$');
    }

    fn backtick(&mut self) {
        self.pos += 1;
        let start = self.pos;
        while self.peek(0).is_some_and(|c| c != '`') {
            if self.peek(0) == Some('\\') {
                self.pos += 1;
            }
            self.pos += 1;
        }
        if self.peek(0).is_none() {
            self.fail("unclosed backtick");
        }
        let end = self.pos.min(self.chars.len());
        self.out
            .substitutions
            .push(self.chars[start..end].iter().collect());
        self.pos += 1;
        self.push_dynamic('$');
    }

    /// Read to the `)` that closes one already consumed, honouring quotes and
    /// nesting. Leaves `pos` after it.
    fn until_close_paren(&mut self) -> String {
        let start = self.pos;
        let mut depth = 1;
        let mut quote: Option<char> = None;
        while let Some(c) = self.peek(0) {
            match (quote, c) {
                (Some(q), _) if c == q => quote = None,
                (Some('"'), '\\') => self.pos += 1,
                (Some(_), _) => {}
                (None, '\'' | '"') => quote = Some(c),
                (None, '\\') => self.pos += 1,
                (None, '(') => depth += 1,
                (None, ')') => {
                    depth -= 1;
                    if depth == 0 {
                        let inner: String = self.chars[start..self.pos].iter().collect();
                        self.pos += 1;
                        return inner;
                    }
                }
                _ => {}
            }
            self.pos += 1;
        }
        self.fail("unclosed parenthesis");
        self.chars[start.min(self.chars.len())..].iter().collect()
    }

    /// At `<` / `>` (or `&>`): read the operator and its target word.
    fn redirect(&mut self) {
        let mut op = String::new();
        while let Some(c) = self.peek(0) {
            if matches!(c, '<' | '>' | '&' | '|') && op.len() < 3 {
                // `>|` forces an overwrite; a `|` after `<`/`>>` is a pipe.
                if c == '|' && op != ">" {
                    break;
                }
                op.push(c);
                self.pos += 1;
            } else {
                break;
            }
        }
        if op.starts_with("<<") && op != "<<<" {
            self.out.heredoc = true;
        }
        while matches!(self.peek(0), Some(' ') | Some('\t')) {
            self.pos += 1;
        }
        // `2>&1`, `>&2`: the target is a descriptor written right after `&`.
        if op.ends_with('&') {
            let mut fd = String::new();
            while let Some(c) = self.peek(0) {
                if c.is_ascii_digit() || c == '-' {
                    fd.push(c);
                    self.pos += 1;
                } else {
                    break;
                }
            }
            if !fd.is_empty() {
                self.out.redirects.push(Redirect { op, target: fd });
                return;
            }
            // `>& file` (csh style) writes the file.
            op.pop();
        }
        // The target is the next word, read with the normal quoting rules.
        let saved_words = std::mem::take(&mut self.words);
        let saved_flags = std::mem::take(&mut self.flags);
        let saved_word = self.word.take();
        let saved_dynamic = std::mem::replace(&mut self.dynamic, false);
        self.read_one_word();
        let target = self.word.take().unwrap_or_default();
        self.words = saved_words;
        self.flags = saved_flags;
        self.word = saved_word;
        self.dynamic = saved_dynamic;
        self.out.redirects.push(Redirect { op, target });
    }

    fn read_one_word(&mut self) {
        while let Some(c) = self.peek(0) {
            match c {
                ' ' | '\t' | '\n' | ';' | '|' | '&' | '(' | ')' | '<' | '>' => break,
                '\\' => {
                    if let Some(n) = self.peek(1) {
                        self.push_char(n);
                    }
                    self.pos += 2;
                }
                '\'' | '"' => {
                    let q = c;
                    self.pos += 1;
                    self.word.get_or_insert_with(String::new);
                    while let Some(c) = self.peek(0) {
                        self.pos += 1;
                        if c == q {
                            break;
                        }
                        self.push_char(c);
                    }
                }
                '$' if self.peek(1) == Some('(') => self.dollar(),
                '`' => self.backtick(),
                _ => {
                    self.push_char(c);
                    self.pos += 1;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segs(cmd: &str) -> Vec<Vec<String>> {
        parse(cmd).segments
    }

    fn words(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn operators_split_commands() {
        assert_eq!(
            segs("ls -la; rm -rf / && echo ok || true | wc -l & sleep 1"),
            vec![
                words(&["ls", "-la"]),
                words(&["rm", "-rf", "/"]),
                words(&["echo", "ok"]),
                words(&["true"]),
                words(&["wc", "-l"]),
                words(&["sleep", "1"]),
            ]
        );
        assert_eq!(segs("df\nreboot"), vec![words(&["df"]), words(&["reboot"])]);
        assert_eq!(segs("(cd /x && rm y)"), vec![words(&["cd", "/x"]), words(&["rm", "y"])]);
    }

    #[test]
    fn quotes_keep_operators_as_text() {
        assert_eq!(
            segs("grep -E 'a|b;c' \"x && y\" file"),
            vec![words(&["grep", "-E", "a|b;c", "x && y", "file"])]
        );
        assert_eq!(segs(r"echo a\;b"), vec![words(&["echo", "a;b"])]);
        assert_eq!(segs("echo ''"), vec![words(&["echo", ""])]);
    }

    #[test]
    fn substitutions_are_collected() {
        let p = parse("echo $(rm -rf /tmp/x) `reboot` \"$(id -u)\" $((1+2))");
        assert_eq!(p.substitutions, vec!["rm -rf /tmp/x", "reboot", "id -u"]);
        assert_eq!(p.segments[0][4], "$((1+2))");
        let p = parse("diff <(ls a) <(ls b)");
        assert_eq!(p.substitutions, vec!["ls a", "ls b"]);
        let p = parse("echo $(echo $(whoami))");
        assert_eq!(p.substitutions, vec!["echo $(whoami)"]);
    }

    #[test]
    fn redirects_are_recorded_with_their_target() {
        let p = parse("echo x > /etc/hosts 2>&1");
        assert_eq!(p.segments, vec![words(&["echo", "x"])]);
        assert!(p.redirects[0].writes_file());
        assert_eq!(p.redirects[0].target, "/etc/hosts");
        assert!(!p.redirects[1].writes_file());

        let p = parse("cat a.log 2>/dev/null >> '/var/log/my file'");
        assert!(!p.redirects[0].writes_file());
        assert_eq!(p.redirects[1].target, "/var/log/my file");
        assert!(p.redirects[1].writes_file());

        let p = parse("sort < in.txt");
        assert!(!p.redirects[0].writes_file());
        assert!(parse("ls &>out.txt").redirects[0].writes_file());
        assert!(parse("ls >| out").redirects[0].writes_file());
        assert!(parse("echo hi>out").redirects[0].writes_file());
    }

    #[test]
    fn heredoc_function_and_errors() {
        assert!(parse("cat <<EOF > /etc/x\nhi\nEOF").heredoc);
        assert!(!parse("grep x <<< 'abc'").heredoc);
        assert!(parse(":(){ :|:& };:").defines_function);
        assert!(parse("function f { rm -rf /; }").defines_function);
        assert!(parse("echo 'oops").error.is_some());
        assert!(parse("echo $(ls").error.is_some());
    }

    #[test]
    fn comments_and_braces() {
        assert_eq!(segs("ls # rm -rf /"), vec![words(&["ls"])]);
        assert!(parse("ls # rm -rf /").has_comment);
        assert!(!parse("echo a#b '#x'").has_comment);
        assert_eq!(segs("echo ${HOME} a{b,c}"), vec![words(&["echo", "${HOME}", "a{b,c}"])]);
    }

    #[test]
    fn words_the_shell_rewrites_are_marked() {
        let lit = |cmd: &str| parse(cmd).literal;
        assert_eq!(lit("find / $X"), vec![vec![true, true, false]]);
        assert_eq!(lit("find / \"$X\" '$Y'"), vec![vec![true, true, false, true]]);
        assert_eq!(lit("cat $'/etc/shado\\x77'"), vec![vec![true, false]]);
        assert_eq!(lit("cat /etc/sha* '/x/*' \\*"), vec![vec![true, false, true, true]]);
        assert_eq!(lit("echo $(id) `id` a{b,c}"), vec![vec![true, false, false, false]]);
        assert_eq!(lit("echo x > $F; ls"), vec![vec![true, true], vec![true]]);
    }

    #[test]
    fn normalize_keeps_quoted_text() {
        assert_eq!(normalize("  ls   -la \t /tmp "), "ls -la /tmp");
        assert_eq!(normalize("grep  'a  b'   f"), "grep 'a  b' f");
        assert_eq!(normalize("echo \"x  \\\"  y\"  z"), "echo \"x  \\\"  y\" z");
        assert_ne!(normalize("rm -rf '/srv/a b'"), normalize("rm -rf '/srv/a  b'"));
    }
}
