//! Recovers Lua source from the bytecode a stripped Lua 5.1 chunk holds.
//!
//! The names a chunk was written with are gone, so locals read as `v0`, `v1` and parameters as `a0`,
//! `a1`; everything else is the source it was compiled from. A function the reading cannot resolve
//! is left as its own disassembly rather than dropped, so [`Decompiled::disassembled`] says how much
//! of a chunk is source and how much is not.

pub mod chunk;

mod asm;
mod expr;
mod read;

use crate::chunk::{Function, Opcode};

pub use chunk::Chunk;
pub use expr::{Closure, Expr, Stat};

/// A chunk as source, and how much of it the reading resolved.
pub struct Decompiled {
    /// The source, one entry per line, with no trailing newlines.
    pub lines: Vec<String>,
    /// Statements the chunk holds. A chunk compiled from an empty file has none.
    pub statements: usize,
    /// Functions recovered as source.
    pub functions: usize,
    /// Functions left as disassembly because the reading did not resolve them.
    pub disassembled: usize,
}

/// Decompile a chunk to Lua.
pub fn decompile(chunk: &Chunk) -> Decompiled {
    let mut counts = read::Counts::default();
    let mut lines = Vec::new();
    let mut statements = 0;

    match units(chunk) {
        Some(units) => {
            for (at, unit) in units.iter().enumerate() {
                if at > 0 {
                    lines.push(String::new());
                }
                if units.len() > 1 {
                    lines.push(format!("-- unit {} of {}", at + 1, units.len()));
                }
                let held = read::closure(unit, Vec::new(), 0, &mut counts);
                statements += held.body.len();
                expr::write_block(&mut lines, &held.body, 0);
            }
        }
        None => {
            let held = read::closure(chunk.main(), Vec::new(), 0, &mut counts);
            statements += held.body.len();
            expr::write_block(&mut lines, &held.body, 0);
        }
    }

    Decompiled {
        lines,
        statements,
        functions: counts.read,
        disassembled: counts.raw,
    }
}

/// One function as source: the statements it holds, and the parameters it takes.
pub struct Source {
    /// Names of the fixed parameters.
    pub parameters: Vec<String>,
    /// Whether the parameter list ends in `...`.
    pub vararg: bool,
    /// The body, one entry per line, with no trailing newlines.
    pub lines: Vec<String>,
}

/// Read one function on its own, or nothing where any part of it stayed disassembly.
///
/// This is what checking a reading against the compiler that wrote the bytecode needs: a function
/// only round-trips if the whole of it came back as source.
pub fn source(held: &Function) -> Option<Source> {
    let mut counts = read::Counts::default();
    let closure = read::closure(held, Vec::new(), 0, &mut counts);
    if counts.raw > 0 {
        return None;
    }
    let mut lines = Vec::new();
    expr::write_block(&mut lines, &closure.body, 0);
    Some(Source {
        parameters: closure.parameters,
        vararg: closure.vararg,
        lines,
    })
}

/// Disassemble a chunk, every function in the order the file holds them.
pub fn disassemble(chunk: &Chunk) -> Vec<String> {
    let mut lines = Vec::new();
    listing(chunk.main(), "main", &mut lines);
    lines
}

fn listing(held: &Function, name: &str, lines: &mut Vec<String>) {
    if !lines.is_empty() {
        lines.push(String::new());
    }
    lines.push(format!(
        "-- {name}: {} params{}, {} slots, {} upvalues, {} constants, lines {}-{}",
        held.parameters(),
        if held.is_vararg() { " and ..." } else { "" },
        held.max_stack(),
        held.upvalues(),
        held.constants().len(),
        held.line_defined(),
        held.last_line_defined(),
    ));
    asm::listing(held, 0, lines);
    for (at, nested) in held.functions().iter().enumerate() {
        listing(nested, &format!("{name}.{at}"), lines);
    }
}

/// The source units a chunk holds, where it holds more than one.
///
/// The game links several compiled files into one chunk behind a wrapper that closes over each and
/// calls it in turn. The wrapper is not something anybody wrote, so a reading skips it and shows
/// what was linked.
pub fn units(chunk: &Chunk) -> Option<&[Function]> {
    let main = chunk.main();
    let code = main.code();
    let units = main.functions();
    if !main.constants().is_empty() || units.is_empty() || code.len() != units.len() * 2 + 1 {
        return None;
    }
    // A closure of each unit in turn, called and dropped, then the return the compiler always writes.
    for (at, pair) in code.chunks_exact(2).take(units.len()).enumerate() {
        let closes = pair[0].opcode() == Opcode::Closure && pair[0].bx() as usize == at;
        let calls = pair[1].opcode() == Opcode::Call && pair[1].b() == 1 && pair[1].c() == 1;
        if !closes || !calls || units[at].upvalues() != 0 {
            return None;
        }
    }
    match code.last().map(|held| held.opcode()) {
        Some(Opcode::Return) => Some(units),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a chunk the way `luaU_dump` writes one, so a test says what bytes mean rather than what
    /// some other reader made of them.
    struct Builder {
        bytes: Vec<u8>,
    }

    impl Builder {
        fn new() -> Self {
            Self {
                bytes: vec![0x1B, b'L', b'u', b'a', 0x51, 0, 1, 4, 4, 4, 8, 0],
            }
        }

        fn word(&mut self, value: u32) -> &mut Self {
            self.bytes.extend(value.to_le_bytes());
            self
        }

        fn text(&mut self, value: &[u8]) -> &mut Self {
            self.word(u32::try_from(value.len() + 1).unwrap());
            self.bytes.extend(value);
            self.bytes.push(0);
            self
        }
    }

    /// The pieces of one function, laid out in the order the dump writes them.
    #[derive(Default)]
    struct Proto {
        parameters: u8,
        varargs: u8,
        stack: u8,
        code: Vec<u32>,
        constants: Vec<Value>,
        nested: Vec<Proto>,
    }

    enum Value {
        Number(f64),
        Text(&'static str),
        Bool(bool),
    }

    impl Proto {
        fn write(&self, into: &mut Builder) {
            into.word(0).word(0).word(0);
            into.bytes
                .extend([0, self.parameters, self.varargs, self.stack.max(2)]);
            into.word(u32::try_from(self.code.len()).unwrap());
            for word in &self.code {
                into.word(*word);
            }
            into.word(u32::try_from(self.constants.len()).unwrap());
            for constant in &self.constants {
                match constant {
                    Value::Number(held) => {
                        into.bytes.push(3);
                        into.bytes.extend(held.to_le_bytes());
                    }
                    Value::Text(held) => {
                        into.bytes.push(4);
                        into.text(held.as_bytes());
                    }
                    Value::Bool(held) => into.bytes.extend([1, u8::from(*held)]),
                }
            }
            into.word(u32::try_from(self.nested.len()).unwrap());
            for nested in &self.nested {
                nested.write(into);
            }
            into.word(0).word(0).word(0);
        }
    }

    fn chunk(main: Proto) -> Chunk {
        let mut builder = Builder::new();
        main.write(&mut builder);
        Chunk::parse(&builder.bytes).expect("the builder writes a chunk the reader takes")
    }

    fn abc(opcode: u32, a: u32, b: u32, c: u32) -> u32 {
        opcode | a << 6 | c << 14 | b << 23
    }

    fn abx(opcode: u32, a: u32, bx: u32) -> u32 {
        opcode | a << 6 | bx << 14
    }

    fn asbx(opcode: u32, a: u32, sbx: i32) -> u32 {
        abx(opcode, a, (sbx + 131071) as u32)
    }

    const GETGLOBAL: u32 = 5;
    const LOADK: u32 = 1;
    const CALL: u32 = 28;
    const RETURN: u32 = 30;
    const EQ: u32 = 23;
    const JMP: u32 = 22;
    const LOADBOOL: u32 = 2;
    const SELF: u32 = 11;
    const SETTABLE: u32 = 9;
    const CLOSURE: u32 = 36;
    const FORPREP: u32 = 32;
    const FORLOOP: u32 = 31;
    const MOVE: u32 = 0;
    const TEST: u32 = 26;
    const SETGLOBAL: u32 = 7;
    const NEWTABLE: u32 = 10;
    const SETLIST: u32 = 34;

    fn source(main: Proto) -> String {
        decompile(&chunk(main)).lines.join("\n")
    }

    #[test]
    fn a_call_of_a_global_reads_as_one() {
        let held = Proto {
            code: vec![
                abx(GETGLOBAL, 0, 0),
                abx(LOADK, 1, 1),
                abc(CALL, 0, 2, 1),
                abc(RETURN, 0, 1, 0),
            ],
            constants: vec![Value::Text("print"), Value::Text("hi")],
            ..Proto::default()
        };
        assert_eq!(source(held), "print(\"hi\")");
    }

    /// A key that reads as a name prints as a field, and the object of a method lookup is not
    /// repeated as its first argument.
    #[test]
    fn a_method_reads_as_one() {
        let held = Proto {
            code: vec![
                abx(GETGLOBAL, 0, 0),
                abc(SELF, 0, 0, 0x101),
                abc(CALL, 0, 2, 1),
                abc(RETURN, 0, 1, 0),
            ],
            constants: vec![Value::Text("t"), Value::Text("m")],
            stack: 3,
            ..Proto::default()
        };
        assert_eq!(source(held), "t:m()");
    }

    /// The load reached by falling through is the false one, so a comparison standing as a value is
    /// the condition the other way up.
    #[test]
    fn a_comparison_standing_as_a_value_keeps_its_sense() {
        let held = Proto {
            parameters: 1,
            code: vec![
                abc(EQ, 1, 0, 0x100),
                asbx(JMP, 0, 1),
                abc(LOADBOOL, 1, 0, 1),
                abc(LOADBOOL, 1, 1, 0),
                abc(RETURN, 1, 2, 0),
            ],
            constants: vec![Value::Number(1.0)],
            stack: 2,
            ..Proto::default()
        };
        assert_eq!(source(held), "return a0 == 1");
    }

    #[test]
    fn a_conditional_reads_as_an_if() {
        let held = Proto {
            parameters: 1,
            code: vec![
                abc(EQ, 0, 0, 0x100),
                asbx(JMP, 0, 2),
                abx(GETGLOBAL, 1, 1),
                abc(CALL, 1, 1, 1),
                abc(RETURN, 0, 1, 0),
            ],
            constants: vec![Value::Number(1.0), Value::Text("f")],
            stack: 2,
            ..Proto::default()
        };
        assert_eq!(source(held), "if a0 == 1 then\n\tf()\nend");
    }

    /// Two conditionals leaving for the same place are one condition, not one `if` inside another.
    #[test]
    fn conditionals_sharing_an_exit_read_as_and() {
        let held = Proto {
            parameters: 2,
            code: vec![
                abc(EQ, 0, 0, 0x100),
                asbx(JMP, 0, 4),
                abc(EQ, 0, 1, 0x101),
                asbx(JMP, 0, 2),
                abx(GETGLOBAL, 2, 2),
                abc(CALL, 2, 1, 1),
                abc(RETURN, 0, 1, 0),
            ],
            constants: vec![Value::Number(1.0), Value::Number(2.0), Value::Text("f")],
            stack: 3,
            ..Proto::default()
        };
        assert_eq!(source(held), "if a0 == 1 and a1 == 2 then\n\tf()\nend");
    }

    /// A conditional leaving for the body rather than past it is the left of an `or`.
    #[test]
    fn a_conditional_leaving_for_the_body_reads_as_or() {
        let held = Proto {
            parameters: 2,
            code: vec![
                abc(EQ, 1, 0, 0x100),
                asbx(JMP, 0, 2),
                abc(EQ, 0, 1, 0x101),
                asbx(JMP, 0, 2),
                abx(GETGLOBAL, 2, 2),
                abc(CALL, 2, 1, 1),
                abc(RETURN, 0, 1, 0),
            ],
            constants: vec![Value::Number(1.0), Value::Number(2.0), Value::Text("f")],
            stack: 3,
            ..Proto::default()
        };
        assert_eq!(source(held), "if a0 == 1 or a1 == 2 then\n\tf()\nend");
    }

    #[test]
    fn a_jump_over_the_body_reads_as_an_else() {
        let held = Proto {
            parameters: 1,
            code: vec![
                abc(EQ, 0, 0, 0x100),
                asbx(JMP, 0, 3),
                abx(GETGLOBAL, 1, 1),
                abc(CALL, 1, 1, 1),
                asbx(JMP, 0, 2),
                abx(GETGLOBAL, 1, 2),
                abc(CALL, 1, 1, 1),
                abc(RETURN, 0, 1, 0),
            ],
            constants: vec![Value::Number(1.0), Value::Text("f"), Value::Text("g")],
            stack: 2,
            ..Proto::default()
        };
        assert_eq!(source(held), "if a0 == 1 then\n\tf()\nelse\n\tg()\nend");
    }

    #[test]
    fn a_for_prep_and_its_loop_read_as_a_numeric_for() {
        let held = Proto {
            code: vec![
                abx(LOADK, 0, 0),
                abx(LOADK, 1, 1),
                abx(LOADK, 2, 0),
                asbx(FORPREP, 0, 3),
                abx(GETGLOBAL, 4, 2),
                abc(MOVE, 5, 3, 0),
                abc(CALL, 4, 2, 1),
                asbx(FORLOOP, 0, -4),
                abc(RETURN, 0, 1, 0),
            ],
            constants: vec![Value::Number(1.0), Value::Number(3.0), Value::Text("f")],
            stack: 7,
            ..Proto::default()
        };
        assert_eq!(source(held), "for v3 = 1, 3 do\n\tf(v3)\nend");
    }

    /// The wrapper the game links units behind is not something anybody wrote, so it is not shown.
    #[test]
    fn a_linked_chunk_shows_its_units_rather_than_its_wrapper() {
        let unit = |name: &'static str| Proto {
            varargs: 2,
            code: vec![
                abx(GETGLOBAL, 0, 0),
                abx(LOADK, 1, 1),
                abc(CALL, 0, 2, 1),
                abc(RETURN, 0, 1, 0),
            ],
            constants: vec![Value::Text("print"), Value::Text(name)],
            ..Proto::default()
        };
        let held = Proto {
            code: vec![
                abx(CLOSURE, 0, 0),
                abc(CALL, 0, 1, 1),
                abx(CLOSURE, 0, 1),
                abc(CALL, 0, 1, 1),
                abc(RETURN, 0, 1, 0),
            ],
            nested: vec![unit("one"), unit("two")],
            ..Proto::default()
        };
        let chunk = chunk(held);
        assert_eq!(units(&chunk).map(<[Function]>::len), Some(2));
        let read = source(Proto {
            code: vec![
                abx(CLOSURE, 0, 0),
                abc(CALL, 0, 1, 1),
                abx(CLOSURE, 0, 1),
                abc(CALL, 0, 1, 1),
                abc(RETURN, 0, 1, 0),
            ],
            nested: vec![unit("one"), unit("two")],
            ..Proto::default()
        });
        assert!(read.contains("-- unit 1 of 2"), "{read}");
        assert!(read.contains(r#"print("one")"#), "{read}");
        assert!(read.contains(r#"print("two")"#), "{read}");
        assert!(!read.contains("function"), "{read}");
    }

    /// A chunk holding one unit has no wrapper to skip.
    #[test]
    fn a_chunk_of_one_unit_reads_on_its_own() {
        let held = Proto {
            code: vec![
                abx(GETGLOBAL, 0, 0),
                abc(CALL, 0, 1, 1),
                abc(RETURN, 0, 1, 0),
            ],
            constants: vec![Value::Text("f")],
            varargs: 2,
            ..Proto::default()
        };
        assert!(
            units(&chunk(Proto {
                code: vec![
                    abx(GETGLOBAL, 0, 0),
                    abc(CALL, 0, 1, 1),
                    abc(RETURN, 0, 1, 0)
                ],
                constants: vec![Value::Text("f")],
                varargs: 2,
                ..Proto::default()
            }))
            .is_none()
        );
        assert_eq!(source(held), "f()");
    }

    /// A jump the compiler collapsed into another lands where that one goes, which is how the exit
    /// of a loop reaches back past its own jump home.
    #[test]
    fn a_loop_whose_arms_jump_home_reads_as_one() {
        let held = Proto {
            code: vec![
                abc(LOADBOOL, 0, 1, 0),
                abc(TEST, 0, 0, 0),
                asbx(JMP, 0, 7),
                abx(GETGLOBAL, 1, 0),
                abc(CALL, 1, 1, 2),
                abc(TEST, 1, 0, 0),
                asbx(JMP, 0, -6),
                abx(GETGLOBAL, 2, 1),
                abc(CALL, 2, 1, 1),
                asbx(JMP, 0, -9),
                abc(RETURN, 0, 1, 0),
            ],
            constants: vec![Value::Text("f"), Value::Text("g")],
            stack: 3,
            ..Proto::default()
        };
        assert_eq!(
            source(held),
            "local v0 = true\nwhile v0 do\n\tlocal v1 = f()\n\tif v1 then\n\t\tg()\n\tend\nend"
        );
    }

    /// A method lookup reserves its two registers before it works the key out, so a function holding
    /// more constants than an operand can name puts the key two registers above the lookup.
    #[test]
    fn a_method_named_from_a_register_still_reads_as_one() {
        let mut constants = vec![Value::Text("t")];
        constants.extend((1..256).map(|held| Value::Number(f64::from(held))));
        constants.push(Value::Text("m"));
        let held = Proto {
            code: vec![
                abx(GETGLOBAL, 3, 0),
                abx(LOADK, 5, 256),
                abc(SELF, 3, 3, 5),
                abc(CALL, 3, 2, 1),
                abc(RETURN, 0, 1, 0),
            ],
            constants,
            stack: 6,
            ..Proto::default()
        };
        assert_eq!(source(held), "t:m()");
    }

    /// A function the reading cannot resolve is kept as its own disassembly rather than dropped.
    #[test]
    fn a_function_that_does_not_resolve_is_kept_as_disassembly() {
        let held = Proto {
            code: vec![abc(TEST, 0, 0, 0), abc(RETURN, 0, 1, 0)],
            stack: 2,
            ..Proto::default()
        };
        let chunk = chunk(held);
        let read = decompile(&chunk);
        assert_eq!((read.functions, read.disassembled), (0, 1));
        let text = read.lines.join("\n");
        assert!(text.contains("TEST"), "{text}");
        assert!(
            text.lines().all(|line| line.trim_start().starts_with("--")),
            "{text}"
        );
    }

    /// A short circuit leaving its value where it tested for it is written as a plain test rather
    /// than one that stores, and a run of them is one expression however many the compiler wrote.
    #[test]
    fn a_run_of_tests_on_one_register_reads_as_an_or() {
        let held = Proto {
            code: vec![
                abx(GETGLOBAL, 0, 0),
                abc(CALL, 0, 1, 2),
                abc(TEST, 0, 0, 1),
                asbx(JMP, 0, 6),
                abx(GETGLOBAL, 0, 1),
                abc(CALL, 0, 1, 2),
                abc(TEST, 0, 0, 1),
                asbx(JMP, 0, 2),
                abx(GETGLOBAL, 0, 2),
                abc(CALL, 0, 1, 2),
                abc(RETURN, 0, 2, 0),
                abc(RETURN, 0, 1, 0),
            ],
            constants: vec![Value::Text("f"), Value::Text("g"), Value::Text("h")],
            stack: 2,
            ..Proto::default()
        };
        assert_eq!(source(held), "return f() or g() or h()");
    }

    /// A list set on a table that is an item of another leaves that other one still waiting, so
    /// neither of them outlived the statement they are both part of.
    #[test]
    fn a_table_built_inside_another_reads_as_one_expression() {
        let held = Proto {
            code: vec![
                abc(NEWTABLE, 0, 2, 0),
                abc(NEWTABLE, 1, 1, 0),
                abx(LOADK, 2, 0),
                abc(SETLIST, 1, 1, 1),
                abc(NEWTABLE, 2, 1, 0),
                abx(LOADK, 3, 1),
                abc(SETLIST, 2, 1, 1),
                abc(SETLIST, 0, 2, 1),
                abx(SETGLOBAL, 0, 2),
                abc(RETURN, 0, 1, 0),
            ],
            constants: vec![Value::Number(1.0), Value::Number(2.0), Value::Text("t")],
            stack: 4,
            ..Proto::default()
        };
        assert_eq!(source(held), "t = { { 1 }, { 2 } }");
    }

    #[test]
    fn a_setter_on_a_global_reads_as_a_field() {
        let held = Proto {
            code: vec![
                abx(GETGLOBAL, 0, 0),
                abx(LOADK, 1, 2),
                abc(SETTABLE, 0, 0x101, 1),
                abc(RETURN, 0, 1, 0),
            ],
            constants: vec![Value::Text("t"), Value::Text("k"), Value::Bool(true)],
            ..Proto::default()
        };
        assert_eq!(source(held), "t.k = true");
    }
}
