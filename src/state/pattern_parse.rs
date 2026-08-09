//! Tokenizer and recursive-descent parser for the `graph-pattern` grammar.
//!
//! A child module of [`super`] so the evaluator there reads as one page. Every
//! failure path returns a message naming what was expected and what was found:
//! a policy author's typo must be a loud parse error, never a selector that
//! quietly matches nothing.

use crate::state::graph::AttrValue;

use super::{Clause, CmpOp, Filter, Pattern, Pred, Term};

/// One lexical token.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Token {
    /// `?name`
    Var(String),
    /// A bare name — including one carrying a `prefix:` that Yupana does not expand.
    Name(String),
    /// A quoted string, a number, or `true`/`false`.
    Lit(AttrValue),
    /// `a`, the kind predicate.
    KindPred,
    /// `;`
    Semi,
    /// `.`
    Dot,
    /// `|`
    Pipe,
    /// `,`
    Comma,
    /// A comparison operator.
    Op(CmpOp),
}

/// Characters that may appear in a bare name or a variable. `:` is included and
/// NOT treated as a prefix separator — see the module docs on [`super`].
fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | ':' | '/' | '.')
}

/// Split `source` into tokens.
pub(super) fn tokenize(source: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = source.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            c if c.is_whitespace() => i += 1,
            ';' => {
                out.push(Token::Semi);
                i += 1;
            }
            '|' => {
                out.push(Token::Pipe);
                i += 1;
            }
            ',' => {
                out.push(Token::Comma);
                i += 1;
            }
            '=' => {
                out.push(Token::Op(CmpOp::Eq));
                i += 1;
            }
            '!' | '<' | '>' => {
                let followed_by_eq = chars.get(i + 1) == Some(&'=');
                let op = match (c, followed_by_eq) {
                    ('!', true) => CmpOp::Ne,
                    ('!', false) => return Err("`!` must be `!=`".to_string()),
                    ('<', true) => CmpOp::Le,
                    ('<', false) => CmpOp::Lt,
                    ('>', true) => CmpOp::Ge,
                    (_, _) => CmpOp::Gt,
                };
                out.push(Token::Op(op));
                i += if followed_by_eq { 2 } else { 1 };
            }
            '"' => {
                let (value, next) = read_quoted(&chars, i)?;
                out.push(Token::Lit(AttrValue::Str(value)));
                i = next;
            }
            '?' => {
                let start = i + 1;
                let end = scan_name(&chars, start);
                if end == start {
                    return Err("`?` must be followed by a variable name".to_string());
                }
                out.push(Token::Var(chars[start..end].iter().collect()));
                i = end;
            }
            c if c.is_ascii_digit()
                || (c == '-' && chars.get(i + 1).is_some_and(char::is_ascii_digit)) =>
            {
                let (value, next) = read_number(&chars, i)?;
                out.push(Token::Lit(AttrValue::Num(value)));
                i = next;
            }
            c if is_name_char(c) => {
                // A lone `.` is the clause separator, never the start of a name.
                if c == '.' {
                    out.push(Token::Dot);
                    i += 1;
                    continue;
                }
                let end = scan_name(&chars, i);
                let word: String = chars[i..end].iter().collect();
                out.push(match word.as_str() {
                    "a" => Token::KindPred,
                    "true" => Token::Lit(AttrValue::Bool(true)),
                    "false" => Token::Lit(AttrValue::Bool(false)),
                    _ => Token::Name(word),
                });
                i = end;
            }
            other => return Err(format!("unexpected character `{other}`")),
        }
    }
    Ok(out)
}

/// Scan a bare name starting at `start`, stopping before a trailing `.` so a
/// clause-final `.` separates rather than joining the next clause's subject.
fn scan_name(chars: &[char], start: usize) -> usize {
    let mut end = start;
    while end < chars.len() && is_name_char(chars[end]) {
        end += 1;
    }
    while end > start && chars[end - 1] == '.' {
        end -= 1;
    }
    end
}

/// Read a `"…"` string, honouring `\"` and `\\`.
fn read_quoted(chars: &[char], open: usize) -> Result<(String, usize), String> {
    let mut out = String::new();
    let mut i = open + 1;
    while i < chars.len() {
        match chars[i] {
            '\\' if i + 1 < chars.len() => {
                out.push(chars[i + 1]);
                i += 2;
            }
            '"' => return Ok((out, i + 1)),
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    Err("unterminated string literal".to_string())
}

/// Read a decimal number, integer or fractional.
fn read_number(chars: &[char], start: usize) -> Result<(f64, usize), String> {
    let mut end = start;
    if chars[end] == '-' {
        end += 1;
    }
    while end < chars.len() && (chars[end].is_ascii_digit() || chars[end] == '.') {
        end += 1;
    }
    // A trailing `.` is the clause separator, not a decimal point.
    while end > start && chars[end - 1] == '.' {
        end -= 1;
    }
    let text: String = chars[start..end].iter().collect();
    text.parse::<f64>()
        .map(|n| (n, end))
        .map_err(|_| format!("`{text}` is not a number"))
}

/// Parse a token stream into a [`Pattern`].
pub(super) fn parse_tokens(tokens: &[Token]) -> Result<Pattern, String> {
    let split = tokens.iter().position(|t| t == &Token::Pipe);
    let (clause_tokens, filter_tokens) = match split {
        Some(at) => (&tokens[..at], &tokens[at + 1..]),
        None => (tokens, &tokens[tokens.len()..]),
    };
    let clauses = parse_clauses(clause_tokens)?;
    if clauses.is_empty() {
        return Err("a pattern needs at least one clause".to_string());
    }
    let filters = parse_filters(filter_tokens)?;
    Ok(Pattern { clauses, filters })
}

fn parse_clauses(tokens: &[Token]) -> Result<Vec<Clause>, String> {
    let mut clauses = Vec::new();
    for chunk in tokens.split(|t| t == &Token::Dot) {
        if chunk.is_empty() {
            continue;
        }
        clauses.push(parse_clause(chunk)?);
    }
    Ok(clauses)
}

fn parse_clause(tokens: &[Token]) -> Result<Clause, String> {
    let Some(Token::Var(subject)) = tokens.first() else {
        return Err(format!(
            "a clause must start with a `?variable`, found {}",
            describe(tokens.first())
        ));
    };
    let mut pairs = Vec::new();
    for chunk in tokens[1..].split(|t| t == &Token::Semi) {
        if chunk.is_empty() {
            return Err(format!("clause `?{subject}` has an empty `;` section"));
        }
        if chunk.len() != 2 {
            return Err(format!(
                "clause `?{subject}` expects `predicate object` pairs, found {} token(s)",
                chunk.len()
            ));
        }
        let pred = match &chunk[0] {
            Token::KindPred => Pred::Kind,
            Token::Name(n) => Pred::Named(n.clone()),
            other => {
                return Err(format!(
                    "`{}` is not a predicate — expected `a` or a name",
                    describe(Some(other))
                ))
            }
        };
        let term = match &chunk[1] {
            Token::Var(v) => Term::Var(v.clone()),
            Token::Lit(v) => Term::Lit(v.clone()),
            Token::Name(n) => Term::Name(n.clone()),
            Token::KindPred => Term::Name("a".to_string()),
            other => return Err(format!("`{}` is not an object term", describe(Some(other)))),
        };
        pairs.push((pred, term));
    }
    Ok(Clause {
        subject: subject.clone(),
        pairs,
    })
}

fn parse_filters(tokens: &[Token]) -> Result<Vec<Filter>, String> {
    let mut filters = Vec::new();
    for chunk in tokens.split(|t| t == &Token::Comma) {
        if chunk.is_empty() {
            continue;
        }
        let [Token::Var(var), Token::Op(op), Token::Lit(value)] = chunk else {
            return Err(format!(
                "a filter must be `?var OP literal`, found {} token(s)",
                chunk.len()
            ));
        };
        filters.push(Filter {
            var: var.clone(),
            op: *op,
            value: value.clone(),
        });
    }
    Ok(filters)
}

/// A token rendered for an error message.
fn describe(token: Option<&Token>) -> String {
    match token {
        None => "end of pattern".to_string(),
        Some(Token::Var(v)) => format!("?{v}"),
        Some(Token::Name(n)) => n.clone(),
        Some(Token::Lit(v)) => v.render(),
        Some(Token::KindPred) => "a".to_string(),
        Some(Token::Semi) => ";".to_string(),
        Some(Token::Dot) => ".".to_string(),
        Some(Token::Pipe) => "|".to_string(),
        Some(Token::Comma) => ",".to_string(),
        Some(Token::Op(_)) => "a comparison operator".to_string(),
    }
}
