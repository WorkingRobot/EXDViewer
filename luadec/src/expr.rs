//! Expression and statement trees, and how they read back as source.
//!
//! Printing is precedence-aware rather than fully bracketed: a reading that brackets everything
//! compiles, but nobody can follow it, and the point of the crate is source somebody reads.

use std::fmt::Write;

/// Where an expression sits in Lua's precedence table. A subexpression binding looser than the slot
/// it lands in is what needs brackets.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Or,
    And,
    Compare,
    Concat,
    Additive,
    Multiplicative,
    Unary,
    Power,
    Atom,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Binary {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Concat,
    Eq,
    Ne,
    Lt,
    Le,
    And,
    Or,
}

impl Binary {
    fn text(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Sub => "-",
            Self::Mul => "*",
            Self::Div => "/",
            Self::Mod => "%",
            Self::Pow => "^",
            Self::Concat => "..",
            Self::Eq => "==",
            Self::Ne => "~=",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::And => "and",
            Self::Or => "or",
        }
    }

    fn level(self) -> Level {
        match self {
            Self::Or => Level::Or,
            Self::And => Level::And,
            Self::Eq | Self::Ne | Self::Lt | Self::Le => Level::Compare,
            Self::Concat => Level::Concat,
            Self::Add | Self::Sub => Level::Additive,
            Self::Mul | Self::Div | Self::Mod => Level::Multiplicative,
            Self::Pow => Level::Power,
        }
    }

    /// Whether the operator groups to the right, which decides which side may share its level.
    fn right(self) -> bool {
        matches!(self, Self::Concat | Self::Pow)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Unary {
    Not,
    Minus,
    Length,
}

/// A function the reading recovered, ready to print as a `function` expression.
#[derive(Clone)]
pub struct Closure {
    /// Names of the fixed parameters.
    pub parameters: Vec<String>,
    /// Whether the parameter list ends in `...`.
    pub vararg: bool,
    pub body: Vec<Stat>,
}

#[derive(Clone)]
pub enum Expr {
    Nil,
    Bool(bool),
    Number(f64),
    Str(Vec<u8>),
    Vararg,
    Name(String),
    /// `t[k]`, printed as `t.k` where the key is a name.
    Index(Box<Expr>, Box<Expr>),
    Call(Box<Expr>, Vec<Expr>),
    /// `object:name(...)`.
    Method(Box<Expr>, String, Vec<Expr>),
    Binary(Binary, Box<Expr>, Box<Expr>),
    Unary(Unary, Box<Expr>),
    Table {
        array: Vec<Expr>,
        hash: Vec<(Expr, Expr)>,
    },
    Function(Box<Closure>),
}

impl Expr {
    fn level(&self) -> Level {
        match self {
            Self::Binary(op, ..) => op.level(),
            // A literal below zero prints as a negation, and a power binds tighter than one.
            Self::Unary(..) | Self::Number(f64::MIN..0.0) => Level::Unary,
            _ => Level::Atom,
        }
    }

    /// Whether the expression may stand to the left of a `.`, `[` or `(` without brackets.
    fn is_prefix(&self) -> bool {
        matches!(
            self,
            Self::Name(_) | Self::Index(..) | Self::Call(..) | Self::Method(..)
        )
    }

    /// The expression with its truth reversed, folded into the operator where one exists so a reading
    /// says `a ~= b` rather than `not (a == b)`.
    pub fn negate(self) -> Self {
        match self {
            Self::Binary(Binary::Eq, left, right) => Self::Binary(Binary::Ne, left, right),
            Self::Binary(Binary::Ne, left, right) => Self::Binary(Binary::Eq, left, right),
            Self::Unary(Unary::Not, held) => *held,
            Self::Bool(held) => Self::Bool(!held),
            held => Self::Unary(Unary::Not, Box::new(held)),
        }
    }

    fn write(&self, out: &mut String, at: Level, indent: usize, lines: &mut Vec<String>) {
        let level = self.level();
        let brackets = level < at;
        if brackets {
            out.push('(');
        }
        self.write_bare(out, indent, lines);
        if brackets {
            out.push(')');
        }
    }

    /// The expression as a prefix, bracketed where it cannot carry a suffix on its own.
    fn write_prefix(&self, out: &mut String, indent: usize, lines: &mut Vec<String>) {
        match self.is_prefix() {
            true => self.write_bare(out, indent, lines),
            false => {
                out.push('(');
                self.write_bare(out, indent, lines);
                out.push(')');
            }
        }
    }

    fn write_bare(&self, out: &mut String, indent: usize, lines: &mut Vec<String>) {
        match self {
            Self::Nil => out.push_str("nil"),
            Self::Bool(held) => out.push_str(if *held { "true" } else { "false" }),
            Self::Number(held) => out.push_str(&number(*held)),
            Self::Str(held) => out.push_str(&quoted(held)),
            Self::Vararg => out.push_str("..."),
            Self::Name(held) => out.push_str(held),

            Self::Index(table, key) => {
                table.write_prefix(out, indent, lines);
                match key.as_ref() {
                    Self::Str(name) if is_name(name) => {
                        out.push('.');
                        out.push_str(&String::from_utf8_lossy(name));
                    }
                    key => {
                        out.push('[');
                        key.write(out, Level::Or, indent, lines);
                        out.push(']');
                    }
                }
            }

            Self::Call(target, arguments) => {
                target.write_prefix(out, indent, lines);
                write_arguments(out, arguments, indent, lines);
            }

            Self::Method(object, name, arguments) => {
                object.write_prefix(out, indent, lines);
                out.push(':');
                out.push_str(name);
                write_arguments(out, arguments, indent, lines);
            }

            Self::Binary(op, left, right) => {
                let level = op.level();
                // The side the operator groups towards may share its level; the other side may not,
                // or `a - (b - c)` reads back as `a - b - c`.
                let (left_at, right_at) = match op.right() {
                    true => (next(level), level),
                    false => (level, next(level)),
                };
                left.write(out, left_at, indent, lines);
                let _ = write!(out, " {} ", op.text());
                right.write(out, right_at, indent, lines);
            }

            Self::Unary(op, held) => {
                out.push_str(match op {
                    Unary::Not => "not ",
                    Unary::Minus => "-",
                    Unary::Length => "#",
                });
                // A power binds tighter than a unary on its left, so `-x^2` is `-(x^2)`; the operand
                // of a unary therefore sits at the power's level.
                held.write(out, Level::Power, indent, lines);
            }

            Self::Table { array, hash } => write_table(out, array, hash, indent, lines),

            Self::Function(closure) => write_closure(out, closure, indent, lines),
        }
    }
}

fn next(level: Level) -> Level {
    match level {
        Level::Or => Level::And,
        Level::And => Level::Compare,
        Level::Compare => Level::Concat,
        Level::Concat => Level::Additive,
        Level::Additive => Level::Multiplicative,
        Level::Multiplicative => Level::Unary,
        Level::Unary | Level::Power | Level::Atom => Level::Atom,
    }
}

fn write_arguments(out: &mut String, arguments: &[Expr], indent: usize, lines: &mut Vec<String>) {
    out.push('(');
    write_list(out, arguments, indent, lines);
    out.push(')');
}

fn write_list(out: &mut String, items: &[Expr], indent: usize, lines: &mut Vec<String>) {
    for (at, item) in items.iter().enumerate() {
        if at > 0 {
            out.push_str(", ");
        }
        item.write(out, Level::Or, indent, lines);
    }
}

fn write_table(
    out: &mut String,
    array: &[Expr],
    hash: &[(Expr, Expr)],
    indent: usize,
    lines: &mut Vec<String>,
) {
    if array.is_empty() && hash.is_empty() {
        out.push_str("{}");
        return;
    }
    out.push_str("{ ");
    let mut first = true;
    for item in array {
        if !first {
            out.push_str(", ");
        }
        first = false;
        item.write(out, Level::Or, indent, lines);
    }
    for (key, value) in hash {
        if !first {
            out.push_str(", ");
        }
        first = false;
        match key {
            Expr::Str(name) if is_name(name) => {
                out.push_str(&String::from_utf8_lossy(name));
                out.push_str(" = ");
            }
            key => {
                out.push('[');
                key.write(out, Level::Or, indent, lines);
                out.push_str("] = ");
            }
        }
        value.write(out, Level::Or, indent, lines);
    }
    out.push_str(" }");
}

/// A closure prints across lines, so the head is flushed and the body emitted before the tail joins
/// whatever the expression was building.
fn write_closure(out: &mut String, closure: &Closure, indent: usize, lines: &mut Vec<String>) {
    out.push_str("function(");
    out.push_str(&closure.parameters.join(", "));
    if closure.vararg {
        if !closure.parameters.is_empty() {
            out.push_str(", ");
        }
        out.push_str("...");
    }
    out.push(')');
    lines.push(std::mem::take(out));
    write_block(lines, &closure.body, indent + 1);
    out.push_str(&"\t".repeat(indent));
    out.push_str("end");
}

/// What an assignment writes to.
#[derive(Clone)]
pub enum Target {
    Name(String),
    Index(Expr, Expr),
}

impl Target {
    fn write(&self, out: &mut String, indent: usize, lines: &mut Vec<String>) {
        match self {
            Self::Name(name) => out.push_str(name),
            Self::Index(table, key) => {
                Expr::Index(Box::new(table.clone()), Box::new(key.clone()))
                    .write_bare(out, indent, lines);
            }
        }
    }
}

#[derive(Clone)]
pub enum Stat {
    /// `local a, b = ...`, with no values where the declaration only reserves the names.
    Local(Vec<String>, Vec<Expr>),
    Assign(Vec<Target>, Vec<Expr>),
    /// A call standing on its own.
    Call(Expr),
    Return(Vec<Expr>),
    Break,
    Do(Vec<Stat>),
    /// The arms of an `if`/`elseif` chain, and its `else` where it has one.
    If(Vec<(Expr, Vec<Stat>)>, Option<Vec<Stat>>),
    While(Expr, Vec<Stat>),
    Repeat(Vec<Stat>, Expr),
    NumericFor {
        name: String,
        start: Expr,
        limit: Expr,
        step: Expr,
        body: Vec<Stat>,
    },
    GenericFor {
        names: Vec<String>,
        values: Vec<Expr>,
        body: Vec<Stat>,
    },
    /// A line the reading could not put into source, kept so nothing is silently dropped.
    Raw(String),
}

/// Print a block, one entry per line, indented from `indent` tabs.
pub fn write_block(lines: &mut Vec<String>, block: &[Stat], indent: usize) {
    for (at, stat) in block.iter().enumerate() {
        // Lua wants `break` and `return` to end the block they are in. One the reading found in the
        // middle of a block, which the compiler is free to leave there, takes a block of its own.
        let last = at + 1 == block.len();
        match stat {
            Stat::Break if !last => lines.push(format!("{}do break end", "\t".repeat(indent))),
            Stat::Return(values) if !last && values.is_empty() => {
                lines.push(format!("{}do return end", "\t".repeat(indent)));
            }
            Stat::Return(_) if !last => {
                let mut held = Vec::new();
                write_stat(&mut held, stat, 0);
                lines.push(format!(
                    "{}do {} end",
                    "\t".repeat(indent),
                    held.join(" ").trim()
                ));
            }
            _ => write_stat(lines, stat, indent),
        }
    }
}

fn write_stat(lines: &mut Vec<String>, stat: &Stat, indent: usize) {
    let tabs = "\t".repeat(indent);
    let mut out = tabs.clone();
    match stat {
        Stat::Local(names, values) => {
            out.push_str("local ");
            out.push_str(&names.join(", "));
            if !values.is_empty() {
                out.push_str(" = ");
                write_list(&mut out, values, indent, lines);
            }
        }

        Stat::Assign(targets, values) => {
            for (at, target) in targets.iter().enumerate() {
                if at > 0 {
                    out.push_str(", ");
                }
                target.write(&mut out, indent, lines);
            }
            out.push_str(" = ");
            write_list(&mut out, values, indent, lines);
        }

        Stat::Call(call) => call.write_bare(&mut out, indent, lines),

        Stat::Return(values) => {
            out.push_str("return");
            if !values.is_empty() {
                out.push(' ');
                write_list(&mut out, values, indent, lines);
            }
        }

        Stat::Break => out.push_str("break"),

        Stat::Do(body) => {
            out.push_str("do");
            lines.push(out);
            write_block(lines, body, indent + 1);
            lines.push(format!("{tabs}end"));
            return;
        }

        Stat::If(arms, otherwise) => {
            for (at, (condition, body)) in arms.iter().enumerate() {
                let mut head = tabs.clone();
                head.push_str(if at == 0 { "if " } else { "elseif " });
                condition.write(&mut head, Level::Or, indent, lines);
                head.push_str(" then");
                lines.push(head);
                write_block(lines, body, indent + 1);
            }
            if let Some(body) = otherwise {
                lines.push(format!("{tabs}else"));
                write_block(lines, body, indent + 1);
            }
            lines.push(format!("{tabs}end"));
            return;
        }

        Stat::While(condition, body) => {
            out.push_str("while ");
            condition.write(&mut out, Level::Or, indent, lines);
            out.push_str(" do");
            lines.push(out);
            write_block(lines, body, indent + 1);
            lines.push(format!("{tabs}end"));
            return;
        }

        Stat::Repeat(body, condition) => {
            out.push_str("repeat");
            lines.push(out);
            write_block(lines, body, indent + 1);
            let mut tail = format!("{tabs}until ");
            condition.write(&mut tail, Level::Or, indent, lines);
            lines.push(tail);
            return;
        }

        Stat::NumericFor {
            name,
            start,
            limit,
            step,
            body,
        } => {
            let _ = write!(out, "for {name} = ");
            start.write(&mut out, Level::Or, indent, lines);
            out.push_str(", ");
            limit.write(&mut out, Level::Or, indent, lines);
            // A step of one is what a `for` without one means, so saying it adds nothing.
            if !matches!(step, Expr::Number(held) if *held == 1.0) {
                out.push_str(", ");
                step.write(&mut out, Level::Or, indent, lines);
            }
            out.push_str(" do");
            lines.push(out);
            write_block(lines, body, indent + 1);
            lines.push(format!("{tabs}end"));
            return;
        }

        Stat::GenericFor {
            names,
            values,
            body,
        } => {
            let _ = write!(out, "for {} in ", names.join(", "));
            write_list(&mut out, values, indent, lines);
            out.push_str(" do");
            lines.push(out);
            write_block(lines, body, indent + 1);
            lines.push(format!("{tabs}end"));
            return;
        }

        Stat::Raw(text) => out.push_str(text),
    }
    lines.push(out);
}

/// Whether the bytes read as a name, which is what lets a key print as `t.k`.
pub fn is_name(bytes: &[u8]) -> bool {
    const KEYWORDS: [&str; 21] = [
        "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "if", "in",
        "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
    ];
    let leads = |byte: u8| byte.is_ascii_alphabetic() || byte == b'_';
    match bytes.split_first() {
        Some((first, rest))
            if leads(*first)
                && rest
                    .iter()
                    .all(|byte| leads(*byte) || byte.is_ascii_digit()) =>
        {
            !KEYWORDS.contains(&&*String::from_utf8_lossy(bytes))
        }
        _ => false,
    }
}

/// The character the bytes open with, where they open with a whole one.
fn character(bytes: &[u8]) -> Option<char> {
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) => std::str::from_utf8(bytes.get(..error.valid_up_to())?).ok()?,
    };
    text.chars().next()
}

/// A number as a literal the lexer reads back as the same value.
pub fn number(value: f64) -> String {
    match value {
        _ if value.is_nan() => "(0/0)".to_owned(),
        f64::INFINITY => "(1/0)".to_owned(),
        f64::NEG_INFINITY => "(-1/0)".to_owned(),
        // Rust prints the shortest decimal that reads back as the same double, which is what the
        // lexer's own strtod then gives; a negative literal is a negation, so it needs brackets
        // wherever it lands under a suffix.
        _ => format!("{value}"),
    }
}

/// A byte string as a quoted literal. Text that is already UTF-8 is left as it reads, so the
/// game's Japanese stays legible, and everything else escapes to stay inside source.
pub fn quoted(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() + 2);
    out.push('"');
    let mut rest = bytes;
    while let Some((byte, tail)) = rest.split_first() {
        match byte {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            0x20..=0x7E => out.push(char::from(*byte)),
            _ => {
                // A multi-byte sequence prints as itself where it is valid text, and as an escape
                // where it is not, so every chunk reads back as the bytes it came from.
                match character(rest) {
                    Some(held) if !held.is_control() => {
                        out.push(held);
                        rest = &rest[held.len_utf8()..];
                        continue;
                    }
                    // A digit after a numeric escape would be read into it, so it is padded.
                    _ if tail.first().is_some_and(u8::is_ascii_digit) => {
                        let _ = write!(out, "\\{byte:03}");
                    }
                    _ => {
                        let _ = write!(out, "\\{byte}");
                    }
                }
            }
        }
        rest = tail;
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(expr: &Expr) -> String {
        let mut out = String::new();
        let mut lines = Vec::new();
        expr.write(&mut out, Level::Or, 0, &mut lines);
        out
    }

    fn binary(op: Binary, left: Expr, right: Expr) -> Expr {
        Expr::Binary(op, Box::new(left), Box::new(right))
    }

    fn name(text: &str) -> Expr {
        Expr::Name(text.to_owned())
    }

    #[test]
    fn a_looser_operand_takes_brackets() {
        let sum = binary(Binary::Add, name("a"), name("b"));
        assert_eq!(
            read(&binary(Binary::Mul, sum.clone(), name("c"))),
            "(a + b) * c"
        );
        assert_eq!(read(&binary(Binary::Add, sum, name("c"))), "a + b + c");
    }

    /// Both operators group to the right, so the side that may share a level is the far one.
    #[test]
    fn grouping_decides_which_side_may_share_a_level() {
        let right = binary(Binary::Sub, name("b"), name("c"));
        assert_eq!(read(&binary(Binary::Sub, name("a"), right)), "a - (b - c)");

        let joined = binary(Binary::Concat, name("b"), name("c"));
        assert_eq!(
            read(&binary(Binary::Concat, name("a"), joined)),
            "a .. b .. c"
        );

        let power = binary(Binary::Pow, name("b"), name("c"));
        assert_eq!(read(&binary(Binary::Pow, power, name("a"))), "(b ^ c) ^ a");
    }

    /// A power binds tighter than the minus on its left, so an unbracketed operand would move.
    #[test]
    fn a_unary_keeps_a_power_together() {
        let power = binary(Binary::Pow, name("x"), Expr::Number(2.0));
        assert_eq!(read(&Expr::Unary(Unary::Minus, Box::new(power))), "-x ^ 2");
        let sum = binary(Binary::Add, name("a"), name("b"));
        assert_eq!(read(&Expr::Unary(Unary::Not, Box::new(sum))), "not (a + b)");
    }

    #[test]
    fn a_key_that_reads_as_a_name_prints_as_a_field() {
        let table = name("t");
        let field = Expr::Index(Box::new(table.clone()), Box::new(Expr::Str(b"k".to_vec())));
        assert_eq!(read(&field), "t.k");

        let keyword = Expr::Index(
            Box::new(table.clone()),
            Box::new(Expr::Str(b"end".to_vec())),
        );
        assert_eq!(read(&keyword), r#"t["end"]"#);

        let digits = Expr::Index(Box::new(table), Box::new(Expr::Str(b"1a".to_vec())));
        assert_eq!(read(&digits), r#"t["1a"]"#);
    }

    /// Only a prefix may carry a suffix, so anything else needs brackets to be called or indexed.
    #[test]
    fn a_suffix_brackets_what_cannot_carry_it() {
        let text = Expr::Str(b"a".to_vec());
        let call = Expr::Method(Box::new(text), "len".to_owned(), Vec::new());
        assert_eq!(read(&call), r#"("a"):len()"#);

        let called = Expr::Call(Box::new(name("f")), vec![name("x")]);
        let chained = Expr::Index(Box::new(called), Box::new(Expr::Str(b"k".to_vec())));
        assert_eq!(read(&chained), "f(x).k");
    }

    #[test]
    fn negation_folds_into_the_operator_it_can() {
        let equal = binary(Binary::Eq, name("a"), name("b"));
        assert_eq!(read(&equal.clone().negate()), "a ~= b");
        assert_eq!(read(&equal.clone().negate().negate()), "a == b");
        let less = binary(Binary::Lt, name("a"), name("b"));
        assert_eq!(read(&less.negate()), "not (a < b)");
    }

    #[test]
    fn text_escapes_only_what_source_cannot_hold() {
        assert_eq!(quoted(b"plain"), r#""plain""#);
        assert_eq!(quoted(b"a\"b\\c"), r#""a\"b\\c""#);
        assert_eq!(quoted(b"line\n"), r#""line\n""#);
        assert_eq!(quoted("\u{3042}".as_bytes()), "\"\u{3042}\"");
        assert_eq!(quoted(&[0xFF]), r#""\255""#);
    }

    /// A digit after a short escape would be read into it, so the escape is padded to three.
    #[test]
    fn an_escape_before_a_digit_is_padded() {
        assert_eq!(quoted(&[0x01, b'1']), r#""\0011""#);
        assert_eq!(quoted(&[0x01, b'a']), "\"\\1a\"");
    }

    #[test]
    fn a_number_reads_back_as_itself() {
        assert_eq!(number(1.0), "1");
        assert_eq!(number(0.5), "0.5");
        assert_eq!(number(-3.0), "-3");
        assert_eq!(number(f64::INFINITY), "(1/0)");
        assert_eq!(number(1e300).parse::<f64>(), Ok(1e300));
    }
}
