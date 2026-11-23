//! Small syntax highlighters for the languages the Wiki renders in the GUI.
//!
//! The frozen v5 design calls out hledger/ledger, Rust and JSON explicitly: dates,
//! accounts, amounts, descriptions and comments for journals; keywords, strings,
//! numbers and comments for code. An unknown language falls back to one plain run
//! per line, so the renderer never has to special-case it.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Plain,
    Date,
    Account,
    Amount,
    Desc,
    Comment,
    Keyword,
    Type,
    Func,
    String,
    Number,
    Bool,
    Key,
    Punct,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub text: String,
    pub class: Class,
}

impl Token {
    fn new(text: impl Into<String>, class: Class) -> Self {
        Self {
            text: text.into(),
            class,
        }
    }
}

/// Highlight one line. Whitespace is part of the tokens so the caller can render the
/// line by concatenating every token without loss.
pub fn line(line: &str, language: &str) -> Vec<Token> {
    match language.trim().to_ascii_lowercase().as_str() {
        "ledger" | "hledger" => ledger(line),
        "rust" | "rs" => rust(line),
        "json" | "jsonc" => json(line),
        _ => vec![Token::new(line, Class::Plain)],
    }
}

fn ledger(line: &str) -> Vec<Token> {
    if line.trim().is_empty() {
        return vec![Token::new(line, Class::Plain)];
    }

    // A full-line comment keeps its indentation.
    let trimmed = line.trim_start();
    if trimmed.starts_with(';') || trimmed.starts_with('#') {
        return vec![Token::new(line, Class::Comment)];
    }

    // A trailing comment after a transaction or posting.
    if let Some(index) = find_comment(line) {
        let (head, tail) = line.split_at(index);
        let mut tokens = ledger_head(head);
        tokens.push(Token::new(tail, Class::Comment));
        return tokens;
    }
    ledger_head(line)
}

fn find_comment(line: &str) -> Option<usize> {
    let mut quoted = false;
    for (index, byte) in line.bytes().enumerate() {
        match byte {
            b'"' => quoted = !quoted,
            b';' if !quoted => return Some(index),
            _ => {}
        }
    }
    None
}

fn ledger_head(head: &str) -> Vec<Token> {
    if head.is_empty() {
        return Vec::new();
    }
    let indent_len = head.len() - head.trim_start_matches([' ', '\t']).len();
    let (indent, rest) = head.split_at(indent_len);
    let mut tokens = Vec::new();
    if !indent.is_empty() {
        tokens.push(Token::new(indent, Class::Plain));
    }
    if rest.is_empty() {
        return tokens;
    }

    if let Some((date, tail)) = split_date(rest) {
        tokens.push(Token::new(date, Class::Date));
        if !tail.is_empty() {
            // Transaction description: `* payee`, `! payee`, or plain text.
            tokens.push(Token::new(tail, Class::Desc));
        }
        return tokens;
    }

    // Posting: account then (optionally) two or more spaces and an amount.
    let split = rest
        .char_indices()
        .collect::<Vec<_>>()
        .windows(2)
        .find_map(|window| {
            let (at, ch) = window[0];
            let (_, next) = window[1];
            ((ch == ' ' || ch == '\t') && (next == ' ' || next == '\t')).then_some(at)
        });
    match split {
        Some(at) => {
            let account = &rest[..at];
            let gap_end = rest[at..]
                .find(|ch: char| ch != ' ' && ch != '\t')
                .map(|offset| at + offset)
                .unwrap_or(rest.len());
            let gap = &rest[at..gap_end];
            let amount = &rest[gap_end..];
            tokens.push(Token::new(account.trim_end(), Class::Account));
            let account_trailing = &account[account.trim_end().len()..];
            if !account_trailing.is_empty() {
                tokens.push(Token::new(account_trailing, Class::Plain));
            }
            tokens.push(Token::new(gap, Class::Plain));
            if !amount.is_empty() {
                tokens.push(Token::new(amount, Class::Amount));
            }
        }
        None => tokens.push(Token::new(rest, Class::Account)),
    }
    tokens
}

fn split_date(rest: &str) -> Option<(&str, &str)> {
    let bytes = rest.as_bytes();
    let digits = |range: std::ops::Range<usize>| {
        range
            .clone()
            .all(|index| bytes.get(index).is_some_and(u8::is_ascii_digit))
    };
    let separator = |index: usize| bytes.get(index).is_some_and(|b| *b == b'-' || *b == b'/');
    if rest.len() >= 10
        && digits(0..4)
        && separator(4)
        && digits(5..7)
        && separator(7)
        && digits(8..10)
    {
        return Some((&rest[..10], &rest[10..]));
    }
    if rest.len() >= 5 && digits(0..2) && separator(2) && digits(3..5) {
        return Some((&rest[..5], &rest[5..]));
    }
    None
}

fn rust(line: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let bytes = line.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let rest = &line[index..];
        if rest.starts_with("//") {
            tokens.push(Token::new(rest, Class::Comment));
            break;
        }
        if rest.starts_with("/*") {
            tokens.push(Token::new(rest, Class::Comment));
            break;
        }
        if rest.starts_with("\"\"\"") {
            // Raw-ish multi-line strings are beyond one line; keep the rest quoted.
            tokens.push(Token::new(rest, Class::String));
            break;
        }
        let byte = bytes[index];
        if byte == b'"' {
            let end = string_end(line, index);
            tokens.push(Token::new(&line[index..end], Class::String));
            index = end;
            continue;
        }
        if byte == b'\'' {
            // A char literal, unless it is a lifetime (`'a`).
            if let Some(end) = char_literal_end(line, index) {
                tokens.push(Token::new(&line[index..end], Class::String));
                index = end;
                continue;
            }
            tokens.push(Token::new(&line[index..index + 1], Class::Punct));
            index += 1;
            continue;
        }
        if byte.is_ascii_digit() {
            let end = take_while(line, index, |ch| {
                ch.is_ascii_alphanumeric() || ch == '.' || ch == '_'
            });
            tokens.push(Token::new(&line[index..end], Class::Number));
            index = end;
            continue;
        }
        if byte.is_ascii_alphabetic() || byte == b'_' {
            let end = take_while(line, index, |ch| ch.is_ascii_alphanumeric() || ch == '_');
            let word = &line[index..end];
            let after = &line[end..];
            let class = if is_rust_keyword(word) {
                Class::Keyword
            } else if matches!(word, "true" | "false") {
                Class::Bool
            } else if after.starts_with('!') {
                Class::Func
            } else if after.starts_with('(') && !word.is_empty() {
                Class::Func
            } else if word.chars().next().is_some_and(char::is_uppercase) {
                Class::Type
            } else {
                Class::Plain
            };
            tokens.push(Token::new(word, class));
            index = end;
            continue;
        }
        if byte.is_ascii_punctuation() || byte == b' ' || byte == b'\t' {
            let end = take_while(line, index, |ch| {
                ch.is_ascii_punctuation() || ch == ' ' || ch == '\t'
            });
            let run = &line[index..end];
            tokens.push(Token::new(run, Class::Punct));
            index = end;
            continue;
        }
        let next = line[index..]
            .chars()
            .next()
            .map(char::len_utf8)
            .unwrap_or(1);
        tokens.push(Token::new(&line[index..index + next], Class::Plain));
        index += next;
    }
    tokens
}

fn is_rust_keyword(word: &str) -> bool {
    matches!(
        word,
        "as" | "async"
            | "await"
            | "break"
            | "const"
            | "continue"
            | "crate"
            | "dyn"
            | "else"
            | "enum"
            | "extern"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "Self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "type"
            | "unsafe"
            | "use"
            | "where"
            | "while"
            | "yield"
    )
}

fn string_end(line: &str, start: usize) -> usize {
    let bytes = line.as_bytes();
    let mut index = start + 1;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 2,
            b'"' => return index + 1,
            _ => index += 1,
        }
    }
    bytes.len()
}

fn char_literal_end(line: &str, start: usize) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut index = start + 1;
    if bytes.get(index) == Some(&b'\\') {
        index += 2;
    } else {
        index += 1;
    }
    while index < bytes.len() && index < start + 8 {
        if bytes[index] == b'\'' {
            return Some(index + 1);
        }
        index += 1;
    }
    None
}

fn take_while(line: &str, start: usize, f: impl Fn(char) -> bool) -> usize {
    let mut end = start;
    for ch in line[start..].chars() {
        if !f(ch) {
            break;
        }
        end += ch.len_utf8();
    }
    end
}

fn json(line: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let bytes = line.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let rest = &line[index..];
        if rest.starts_with("//") {
            tokens.push(Token::new(rest, Class::Comment));
            break;
        }
        let byte = bytes[index];
        if byte == b'"' {
            let end = string_end(line, index);
            let text = &line[index..end];
            let after = &line[end..];
            let class = if after.trim_start().starts_with(':') {
                Class::Key
            } else {
                Class::String
            };
            tokens.push(Token::new(text, class));
            index = end;
            continue;
        }
        if byte.is_ascii_digit()
            || (byte == b'-' && bytes.get(index + 1).is_some_and(u8::is_ascii_digit))
        {
            let end = take_while(line, index, |ch| {
                ch.is_ascii_digit() || matches!(ch, '.' | 'e' | 'E' | '+' | '-')
            });
            tokens.push(Token::new(&line[index..end], Class::Number));
            index = end;
            continue;
        }
        if byte.is_ascii_alphabetic() {
            let end = take_while(line, index, |ch| ch.is_ascii_alphabetic());
            let word = &line[index..end];
            let class = match word {
                "true" | "false" | "null" => Class::Bool,
                _ => Class::Plain,
            };
            tokens.push(Token::new(word, class));
            index = end;
            continue;
        }
        if byte.is_ascii_punctuation() || byte == b' ' || byte == b'\t' {
            let end = take_while(line, index, |ch| {
                ch.is_ascii_punctuation() || ch == ' ' || ch == '\t'
            });
            let run = &line[index..end];
            let class =
                if run.contains('{') || run.contains('}') || run.contains('[') || run.contains(']')
                {
                    Class::Punct
                } else {
                    Class::Punct
                };
            tokens.push(Token::new(run, class));
            index = end;
            continue;
        }
        let next = line[index..]
            .chars()
            .next()
            .map(char::len_utf8)
            .unwrap_or(1);
        tokens.push(Token::new(&line[index..index + next], Class::Plain));
        index += next;
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(tokens: &[Token]) -> String {
        tokens.iter().map(|token| token.text.as_str()).collect()
    }

    #[test]
    fn every_highlighter_keeps_the_line_exactly() {
        for source in [
            "2026-11-16 * 買い物",
            "    支出:食費   1,200 JPY  ; 11月分",
            "fn main() { let s = String::from(\"hello\"); }",
            "  \"workspace\": \"p-home\", // note",
            "未知の言語の行",
        ] {
            for language in ["ledger", "rust", "json", "unknown"] {
                assert_eq!(texts(&line(source, language)), source);
            }
        }
    }

    #[test]
    fn ledger_classes_the_design_parts() {
        let tokens = line("    支出:食費   1,200 JPY", "hledger");
        assert_eq!(tokens[1].class, Class::Account);
        assert_eq!(tokens[3].class, Class::Amount);
        let comment = line("; 11月分", "ledger");
        assert_eq!(comment[0].class, Class::Comment);
    }

    #[test]
    fn unknown_language_is_one_plain_run() {
        let tokens = line("hello", "brainfuck");
        assert_eq!(tokens, vec![Token::new("hello", Class::Plain)]);
    }
}
