//! The reader: text to data.

use crate::value::Value;
use crate::{Error, Result};

const MAX_DEPTH: usize = 400;

pub struct Reader<'a> {
    chars: std::iter::Peekable<std::str::CharIndices<'a>>,
    src: &'a str,
}

fn is_delim(c: char) -> bool {
    c.is_whitespace() || matches!(c, '(' | ')' | '"' | ';' | '\'')
}

impl<'a> Reader<'a> {
    pub fn new(src: &'a str) -> Self {
        Reader { chars: src.char_indices().peekable(), src }
    }

    fn skip_ws(&mut self) -> Result<()> {
        while let Some(&(_, c)) = self.chars.peek() {
            if c.is_whitespace() {
                self.chars.next();
            } else if c == ';' {
                for (_, c) in self.chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            } else if c == '#' && self.src.get(self.pos()..).is_some_and(|s| s.starts_with("#|")) {
                self.chars.next();
                self.chars.next();
                let mut depth = 1;
                let mut prev = ' ';
                loop {
                    let Some((_, c)) = self.chars.next() else { return Err(Error::msg("unterminated #| comment")) };
                    match (prev, c) {
                        ('|', '#') => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                            prev = ' ';
                            continue;
                        }
                        ('#', '|') => {
                            depth += 1;
                            prev = ' ';
                            continue;
                        }
                        _ => {}
                    }
                    prev = c;
                }
            } else {
                break;
            }
        }
        Ok(())
    }

    fn pos(&mut self) -> usize {
        self.chars.peek().map_or(self.src.len(), |&(i, _)| i)
    }

    /// The next datum, or `None` at the end of the text.
    pub fn read(&mut self) -> Result<Option<Value>> {
        self.skip_ws()?;
        if self.chars.peek().is_none() {
            return Ok(None);
        }
        self.datum(0).map(Some)
    }

    fn datum(&mut self, depth: usize) -> Result<Value> {
        if depth > MAX_DEPTH {
            return Err(Error::msg("expression nested too deeply"));
        }
        self.skip_ws()?;
        let Some(&(start, c)) = self.chars.peek() else { return Err(Error::msg("unexpected end of input")) };
        match c {
            '(' | '[' => {
                self.chars.next();
                self.list(depth, if c == '(' { ')' } else { ']' })
            }
            ')' | ']' => {
                self.chars.next();
                Err(Error::msg("unexpected )"))
            }
            '\'' => {
                self.chars.next();
                self.quoted("quote", depth)
            }
            '`' => {
                self.chars.next();
                self.quoted("quasiquote", depth)
            }
            ',' => {
                self.chars.next();
                if self.chars.peek().is_some_and(|&(_, c)| c == '@') {
                    self.chars.next();
                    self.quoted("unquote-splicing", depth)
                } else {
                    self.quoted("unquote", depth)
                }
            }
            '"' => {
                self.chars.next();
                self.string()
            }
            '#' => self.hash(start, depth),
            _ => Ok(self.atom(start)),
        }
    }

    fn quoted(&mut self, name: &str, depth: usize) -> Result<Value> {
        let d = self.datum(depth + 1)?;
        Ok(Value::list([Value::sym(name), d]))
    }

    fn list(&mut self, depth: usize, close: char) -> Result<Value> {
        let mut items = Vec::new();
        let mut tail = Value::Nil;
        loop {
            self.skip_ws()?;
            match self.chars.peek() {
                None => return Err(Error::msg("missing )")),
                Some(&(_, c)) if c == close || c == ')' || c == ']' => {
                    self.chars.next();
                    break;
                }
                Some(&(i, '.')) if self.src.get(i + 1..).and_then(|s| s.chars().next()).is_none_or(is_delim) => {
                    self.chars.next();
                    tail = self.datum(depth + 1)?;
                    self.skip_ws()?;
                    match self.chars.next() {
                        Some((_, ')' | ']')) => break,
                        _ => return Err(Error::msg("bad dotted list")),
                    }
                }
                Some(_) => items.push(self.datum(depth + 1)?),
            }
        }
        Ok(items.into_iter().rev().fold(tail, |t, v| Value::cons(v, t)))
    }

    fn string(&mut self) -> Result<Value> {
        let mut s = String::new();
        loop {
            let Some((_, c)) = self.chars.next() else { return Err(Error::msg("unterminated string")) };
            match c {
                '"' => return Ok(Value::string(s)),
                '\\' => match self.chars.next() {
                    Some((_, 'n')) => s.push('\n'),
                    Some((_, 't')) => s.push('\t'),
                    Some((_, 'r')) => s.push('\r'),
                    Some((_, '0')) => s.push('\0'),
                    Some((_, c)) => s.push(c),
                    None => return Err(Error::msg("unterminated string")),
                },
                c => s.push(c),
            }
        }
    }

    fn hash(&mut self, start: usize, depth: usize) -> Result<Value> {
        self.chars.next();
        match self.chars.peek().map(|&(_, c)| c) {
            Some('(') => {
                self.chars.next();
                let l = self.list(depth, ')')?;
                Ok(Value::vector(l.to_vec()?))
            }
            Some('\\') => {
                self.chars.next();
                let Some((i, first)) = self.chars.next() else { return Err(Error::msg("bad character")) };
                let mut end = i + first.len_utf8();
                while let Some(&(j, c)) = self.chars.peek() {
                    if is_delim(c) {
                        break;
                    }
                    end = j + c.len_utf8();
                    self.chars.next();
                }
                let name = self.src.get(i..end).unwrap_or_default();
                Ok(Value::Char(match name {
                    "space" => ' ',
                    "newline" | "linefeed" => '\n',
                    "tab" => '\t',
                    "return" => '\r',
                    "nul" | "null" => '\0',
                    n if n.chars().count() == 1 => first,
                    n => return Err(Error::msg(format!("unknown character #\\{n}"))),
                }))
            }
            _ => {
                let a = self.atom(start);
                match a.as_sym() {
                    Some("#t" | "#true") => Ok(Value::Bool(true)),
                    Some("#f" | "#false") => Ok(Value::Bool(false)),
                    Some(other) => Err(Error::msg(format!("unknown syntax {other}"))),
                    None => Err(Error::msg("bad # syntax")),
                }
            }
        }
    }

    /// A number or symbol starting at byte `start`.
    fn atom(&mut self, start: usize) -> Value {
        let mut end = start;
        while let Some(&(j, c)) = self.chars.peek() {
            if is_delim(c) || c == ']' {
                break;
            }
            end = j + c.len_utf8();
            self.chars.next();
        }
        let text = self.src.get(start..end).unwrap_or_default();
        parse_number(text).unwrap_or_else(|| Value::Sym(text.into()))
    }
}

/// Integer or decimal literal; `None` when `s` is not a number.
pub fn parse_number(s: &str) -> Option<Value> {
    let body = s.strip_prefix(['+', '-']).unwrap_or(s);
    if body.is_empty() || !body.chars().next().is_some_and(|c| c.is_ascii_digit() || c == '.') {
        return None;
    }
    if let Ok(i) = s.parse::<i64>() {
        return Some(Value::Int(i));
    }
    if let Some(hex) = s.strip_prefix("0x") {
        return i64::from_str_radix(hex, 16).ok().map(Value::Int);
    }
    s.parse::<f64>().ok().filter(|f| f.is_finite()).map(Value::Float)
}
