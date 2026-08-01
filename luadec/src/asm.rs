//! Disassembly, which is what a function reads as when the reading of it does not resolve.

use std::fmt::Write;

use crate::chunk::{Constant, Function, Instruction, Opcode, Operand};

use crate::expr::{number, quoted};

/// One function's instructions, one line each, indented from `indent` tabs.
pub fn listing(held: &Function, indent: usize, out: &mut Vec<String>) {
    let tabs = "\t".repeat(indent);
    for (pc, &instruction) in held.code().iter().enumerate() {
        let mut line = format!("{tabs}{:>5}  {:<9}", pc + 1, instruction.opcode().name());
        let operands = operands(held, instruction);
        if !operands.is_empty() {
            let _ = write!(line, "  {operands}");
        }
        if let Some(note) = note(held, instruction) {
            let width = line.chars().count();
            let _ = write!(
                line,
                "{}; {note}",
                " ".repeat(46usize.saturating_sub(width))
            );
        }
        out.push(line);
    }
}

fn operands(held: &Function, instruction: Instruction) -> String {
    let (a, b, c) = (instruction.a(), instruction.b(), instruction.c());
    let rk = |value: u16| match Operand::from(value) {
        Operand::Register(register) => format!("R{register}"),
        Operand::Constant(at) => format!("K{at}"),
    };
    let _ = held;
    match instruction.opcode() {
        Opcode::Move | Opcode::Not | Opcode::Unm | Opcode::Len | Opcode::TestSet => {
            format!("R{a}, R{b}")
        }
        Opcode::LoadK | Opcode::GetGlobal | Opcode::SetGlobal => {
            format!("R{a}, K{}", instruction.bx())
        }
        Opcode::LoadBool => format!("R{a}, {b}, {c}"),
        Opcode::LoadNil => format!("R{a}, R{b}"),
        Opcode::GetUpval | Opcode::SetUpval => format!("R{a}, U{b}"),
        Opcode::GetTable | Opcode::Self_ => format!("R{a}, R{b}, {}", rk(c)),
        Opcode::SetTable
        | Opcode::Add
        | Opcode::Sub
        | Opcode::Mul
        | Opcode::Div
        | Opcode::Mod
        | Opcode::Pow => format!("R{a}, {}, {}", rk(b), rk(c)),
        Opcode::NewTable | Opcode::Call | Opcode::TailCall | Opcode::SetList => {
            format!("R{a}, {b}, {c}")
        }
        Opcode::Concat => format!("R{a}, R{b}, R{c}"),
        Opcode::Jmp | Opcode::ForLoop | Opcode::ForPrep => format!("{:+}", instruction.sbx()),
        Opcode::Eq | Opcode::Lt | Opcode::Le => format!("{a}, {}, {}", rk(b), rk(c)),
        Opcode::Test | Opcode::TForLoop => format!("R{a}, {c}"),
        Opcode::Return | Opcode::Vararg => format!("R{a}, {b}"),
        Opcode::Close => format!("R{a}"),
        Opcode::Closure => format!("R{a}, F{}", instruction.bx()),
        Opcode::Unknown(held) => format!("{held:#04x}"),
    }
}

/// What the instruction's constants say, so a listing reads without counting the pool by hand.
fn note(held: &Function, instruction: Instruction) -> Option<String> {
    let text = |at: usize| held.constants().get(at).map(constant);
    match instruction.opcode() {
        Opcode::LoadK | Opcode::GetGlobal | Opcode::SetGlobal => text(instruction.bx() as usize),
        Opcode::GetTable
        | Opcode::Self_
        | Opcode::SetTable
        | Opcode::Add
        | Opcode::Sub
        | Opcode::Mul
        | Opcode::Div
        | Opcode::Mod
        | Opcode::Pow
        | Opcode::Eq
        | Opcode::Lt
        | Opcode::Le => {
            let named = |value: u16| match Operand::from(value) {
                Operand::Constant(at) => text(usize::from(at)),
                Operand::Register(_) => None,
            };
            match (named(instruction.b()), named(instruction.c())) {
                (None, None) => None,
                (left, right) => Some(format!(
                    "{} {}",
                    left.unwrap_or_else(|| "-".to_owned()),
                    right.unwrap_or_else(|| "-".to_owned())
                )),
            }
        }
        _ => None,
    }
}

fn constant(held: &Constant) -> String {
    match held {
        Constant::Nil => "nil".to_owned(),
        Constant::Boolean(held) => held.to_string(),
        Constant::Number(held) => number(*held),
        Constant::String(held) => quoted(held),
    }
}
