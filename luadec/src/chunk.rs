//! The bytecode itself, as `luaU_dump` writes it.
//!
//! The layout is fixed by the header rather than by the version alone, so a chunk built for another
//! word size or byte order is refused instead of misread.

use std::fmt;

/// Version byte of Lua 5.1, the only one the reader takes.
pub const VERSION_51: u8 = 0x51;

/// Nesting the reader will follow before it calls a chunk malformed. Lua's own parser stops a long
/// way before this, so only a crafted file reaches it.
const MAX_DEPTH: usize = 200;

/// Smallest a function can encode to, used to bound how many one count may claim.
const FUNCTION_SIZE: usize = 40;

/// Why a chunk could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error(String);

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// A compiled Lua chunk.
#[derive(Debug, Clone)]
pub struct Chunk {
    header: Header,
    main: Function,
}

impl Chunk {
    /// Read a chunk from the bytes of a whole file.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let mut walk = Walk { bytes, at: 0 };
        let header = walk.header()?;
        let main = walk.function(0)?;
        match walk.at == bytes.len() {
            true => Ok(Self { header, main }),
            false => Err(walk.invalid(format!(
                "the chunk ends at {:#x}, where the file is {:#x} bytes",
                walk.at,
                bytes.len()
            ))),
        }
    }

    /// The layout the chunk was written for.
    pub fn header(&self) -> Header {
        self.header
    }

    /// The chunk's outermost function, which the loader calls to run it.
    pub fn main(&self) -> &Function {
        &self.main
    }

    /// How many functions the chunk holds, counting the outermost one.
    pub fn function_count(&self) -> usize {
        fn count(held: &Function) -> usize {
            1 + held.functions.iter().map(count).sum::<usize>()
        }
        count(&self.main)
    }
}

/// The widths and byte order a chunk was written with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// `0x51` for Lua 5.1.
    pub version: u8,
    /// `0` for the layout the reference implementation writes.
    pub format: u8,
    /// Zero where numbers and integers are big-endian.
    pub little_endian: u8,
    pub size_int: u8,
    pub size_size: u8,
    pub size_instruction: u8,
    /// Width of `lua_Number`, eight for the `double` a stock build uses.
    pub size_number: u8,
    /// Non-zero where `lua_Number` is an integer type.
    pub integral: u8,
}

/// One function of a chunk, holding its own code, constants and nested functions.
///
/// A stripped chunk keeps [`line_defined`](Self::line_defined) and
/// [`last_line_defined`](Self::last_line_defined) but drops the source name, per-instruction lines,
/// local names and upvalue names, leaving [`lines`](Self::lines), [`locals`](Self::locals) and
/// [`upvalue_names`](Self::upvalue_names) empty.
#[derive(Debug, Clone)]
pub struct Function {
    source: Option<Vec<u8>>,
    line_defined: u32,
    last_line_defined: u32,
    upvalues: u8,
    parameters: u8,
    varargs: u8,
    max_stack: u8,
    code: Vec<Instruction>,
    constants: Vec<Constant>,
    functions: Vec<Function>,
    lines: Vec<u32>,
    locals: Vec<Local>,
    upvalue_names: Vec<Vec<u8>>,
}

impl Function {
    /// Name of the file the function was compiled from, which a stripped chunk drops.
    pub fn source(&self) -> Option<&[u8]> {
        self.source.as_deref()
    }

    /// First line of the function in the source it was compiled from.
    pub fn line_defined(&self) -> u32 {
        self.line_defined
    }

    /// Last line of the function in the source it was compiled from.
    pub fn last_line_defined(&self) -> u32 {
        self.last_line_defined
    }

    /// How many upvalues the function closes over.
    pub fn upvalues(&self) -> u8 {
        self.upvalues
    }

    /// Fixed parameters, not counting the implicit `arg` a [`Self::has_arg`] function takes.
    pub fn parameters(&self) -> u8 {
        self.parameters
    }

    /// Registers the function needs, which is where its temporaries stop.
    pub fn max_stack(&self) -> u8 {
        self.max_stack
    }

    /// The function's instructions.
    pub fn code(&self) -> &[Instruction] {
        &self.code
    }

    /// Values the function's instructions index rather than hold.
    pub fn constants(&self) -> &[Constant] {
        &self.constants
    }

    /// Functions [`Opcode::Closure`] builds a closure from, indexed by its `Bx`.
    pub fn functions(&self) -> &[Function] {
        &self.functions
    }

    /// Source line of each instruction, empty in a stripped chunk.
    pub fn lines(&self) -> &[u32] {
        &self.lines
    }

    /// Locals the function declared, empty in a stripped chunk.
    pub fn locals(&self) -> &[Local] {
        &self.locals
    }

    /// Name of each upvalue, empty in a stripped chunk.
    pub fn upvalue_names(&self) -> &[Vec<u8>] {
        &self.upvalue_names
    }

    /// Whether the function takes `...`.
    pub fn is_vararg(&self) -> bool {
        self.varargs & 2 != 0
    }

    /// Whether the register past the fixed parameters holds the 5.0 `arg` table.
    pub fn has_arg(&self) -> bool {
        self.varargs & 1 != 0
    }

    /// Whether the function reads `arg`, which is what makes the caller build the table.
    pub fn needs_arg(&self) -> bool {
        self.varargs & 4 != 0
    }
}

/// A value a function's instructions index.
#[derive(Debug, Clone, PartialEq)]
pub enum Constant {
    Nil,
    Boolean(bool),
    Number(f64),
    /// Lua strings are byte strings, so text is left in whatever encoding it was written in.
    String(Vec<u8>),
}

/// A local and the instructions it is in scope over.
#[derive(Debug, Clone)]
pub struct Local {
    name: Vec<u8>,
    start_pc: u32,
    end_pc: u32,
}

impl Local {
    /// The name the source gave it.
    pub fn name(&self) -> &[u8] {
        &self.name
    }

    /// First instruction the local is in scope over.
    pub fn start_pc(&self) -> u32 {
        self.start_pc
    }

    /// First instruction past the local's scope.
    pub fn end_pc(&self) -> u32 {
        self.end_pc
    }
}

/// One instruction, in the packed form the VM reads.
///
/// Lua 5.1 lays a word out as `B:9 C:9 A:8 opcode:6` from the top, with `Bx` taking the two nine-bit
/// fields together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Instruction(u32);

impl Instruction {
    /// The word as it sits in the file.
    pub fn raw(self) -> u32 {
        self.0
    }

    /// What the instruction does.
    pub fn opcode(self) -> Opcode {
        Opcode::from(self.0 as u8 & 0x3F)
    }

    /// The `A` field, which is a register for every instruction that has one.
    pub fn a(self) -> u8 {
        (self.0 >> 6) as u8
    }

    /// The `B` field.
    pub fn b(self) -> u16 {
        ((self.0 >> 23) & 0x1FF) as u16
    }

    /// The `C` field.
    pub fn c(self) -> u16 {
        ((self.0 >> 14) & 0x1FF) as u16
    }

    /// The `B` and `C` fields together, as instructions taking a constant or function index use them.
    pub fn bx(self) -> u32 {
        self.0 >> 14
    }

    /// [`Self::bx`] biased so a jump can reach backwards.
    pub fn sbx(self) -> i32 {
        self.bx() as i32 - 131071
    }
}

/// What an `RK` operand names, which is a register below `256` and a constant at or above it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operand {
    Register(u8),
    Constant(u16),
}

impl From<u16> for Operand {
    fn from(value: u16) -> Self {
        match value & 0x100 {
            0 => Self::Register(value as u8),
            _ => Self::Constant(value & 0xFF),
        }
    }
}

/// What an [`Instruction`] does.
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opcode {
    Move,
    LoadK,
    LoadBool,
    LoadNil,
    GetUpval,
    GetGlobal,
    GetTable,
    SetGlobal,
    SetUpval,
    SetTable,
    NewTable,
    Self_,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Unm,
    Not,
    Len,
    Concat,
    Jmp,
    Eq,
    Lt,
    Le,
    Test,
    TestSet,
    Call,
    TailCall,
    Return,
    ForLoop,
    ForPrep,
    TForLoop,
    SetList,
    Close,
    Closure,
    Vararg,
    /// A number no Lua 5.1 opcode has.
    Unknown(u8),
}

/// Every opcode in the order the VM numbers them, which is also the order `luac -l` names them in.
const OPCODES: [(Opcode, &str); 38] = [
    (Opcode::Move, "MOVE"),
    (Opcode::LoadK, "LOADK"),
    (Opcode::LoadBool, "LOADBOOL"),
    (Opcode::LoadNil, "LOADNIL"),
    (Opcode::GetUpval, "GETUPVAL"),
    (Opcode::GetGlobal, "GETGLOBAL"),
    (Opcode::GetTable, "GETTABLE"),
    (Opcode::SetGlobal, "SETGLOBAL"),
    (Opcode::SetUpval, "SETUPVAL"),
    (Opcode::SetTable, "SETTABLE"),
    (Opcode::NewTable, "NEWTABLE"),
    (Opcode::Self_, "SELF"),
    (Opcode::Add, "ADD"),
    (Opcode::Sub, "SUB"),
    (Opcode::Mul, "MUL"),
    (Opcode::Div, "DIV"),
    (Opcode::Mod, "MOD"),
    (Opcode::Pow, "POW"),
    (Opcode::Unm, "UNM"),
    (Opcode::Not, "NOT"),
    (Opcode::Len, "LEN"),
    (Opcode::Concat, "CONCAT"),
    (Opcode::Jmp, "JMP"),
    (Opcode::Eq, "EQ"),
    (Opcode::Lt, "LT"),
    (Opcode::Le, "LE"),
    (Opcode::Test, "TEST"),
    (Opcode::TestSet, "TESTSET"),
    (Opcode::Call, "CALL"),
    (Opcode::TailCall, "TAILCALL"),
    (Opcode::Return, "RETURN"),
    (Opcode::ForLoop, "FORLOOP"),
    (Opcode::ForPrep, "FORPREP"),
    (Opcode::TForLoop, "TFORLOOP"),
    (Opcode::SetList, "SETLIST"),
    (Opcode::Close, "CLOSE"),
    (Opcode::Closure, "CLOSURE"),
    (Opcode::Vararg, "VARARG"),
];

impl From<u8> for Opcode {
    fn from(value: u8) -> Self {
        match OPCODES.get(usize::from(value)) {
            Some((opcode, _)) => *opcode,
            None => Self::Unknown(value),
        }
    }
}

impl Opcode {
    /// The name `luac -l` prints.
    pub fn name(self) -> &'static str {
        OPCODES
            .iter()
            .find_map(|(opcode, name)| (*opcode == self).then_some(*name))
            .unwrap_or("UNKNOWN")
    }
}

/// A walk over a chunk, carrying where the last read finished.
struct Walk<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Walk<'a> {
    fn invalid(&self, reason: impl Into<String>) -> Error {
        Error(reason.into())
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let taken = self
            .bytes
            .get(self.at..)
            .and_then(|rest| rest.get(..count))
            .ok_or_else(|| {
                self.invalid(format!(
                    "{count} bytes at {:#x} run past the end of the file",
                    self.at
                ))
            })?;
        self.at += count;
        Ok(taken)
    }

    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn word(&mut self) -> Result<u32> {
        let bytes: [u8; 4] = self
            .take(4)?
            .try_into()
            .map_err(|_| self.invalid("short word"))?;
        Ok(u32::from_le_bytes(bytes))
    }

    fn number(&mut self) -> Result<f64> {
        let bytes: [u8; 8] = self
            .take(8)?
            .try_into()
            .map_err(|_| self.invalid("short number"))?;
        Ok(f64::from_le_bytes(bytes))
    }

    /// A count of records at least `size` bytes each, checked against the bytes left so a malformed
    /// one cannot be allocated for.
    fn count(&mut self, size: usize, what: &str) -> Result<usize> {
        let count = self.word()? as usize;
        let left = self.bytes.len().saturating_sub(self.at);
        match count.checked_mul(size).is_some_and(|need| need <= left) {
            true => Ok(count),
            false => Err(self.invalid(format!(
                "{count} {what} at {:#x} do not fit in the {left} bytes left",
                self.at
            ))),
        }
    }

    /// A length-prefixed string, whose length counts a terminator that is not kept.
    fn text(&mut self) -> Result<Option<Vec<u8>>> {
        match self.count(1, "string bytes")? {
            0 => Ok(None),
            size => Ok(Some(self.take(size)?[..size - 1].to_vec())),
        }
    }

    fn header(&mut self) -> Result<Header> {
        if self.take(4)? != b"\x1bLua" {
            return Err(self.invalid("the file does not open with a Lua signature"));
        }
        let header = Header {
            version: self.byte()?,
            format: self.byte()?,
            little_endian: self.byte()?,
            size_int: self.byte()?,
            size_size: self.byte()?,
            size_instruction: self.byte()?,
            size_number: self.byte()?,
            integral: self.byte()?,
        };

        // Every width decides how the rest of the file is laid out, so a chunk built for another
        // one cannot be read as this one.
        let unsupported = |what: &str, held: u8| {
            Err(Error(format!(
                "{what} is {held}, which the reader does not take"
            )))
        };
        match header {
            Header { version, .. } if version != VERSION_51 => unsupported("the version", version),
            Header { format, .. } if format != 0 => unsupported("the format", format),
            Header {
                little_endian: 0, ..
            } => unsupported("the endianness", 0),
            Header { size_int, .. } if size_int != 4 => unsupported("sizeof(int)", size_int),
            Header { size_size, .. } if size_size != 4 => unsupported("sizeof(size_t)", size_size),
            Header {
                size_instruction, ..
            } if size_instruction != 4 => unsupported("sizeof(Instruction)", size_instruction),
            Header { size_number, .. } if size_number != 8 => {
                unsupported("sizeof(lua_Number)", size_number)
            }
            Header { integral, .. } if integral != 0 => unsupported("the number kind", integral),
            _ => Ok(header),
        }
    }

    fn function(&mut self, depth: usize) -> Result<Function> {
        if depth > MAX_DEPTH {
            return Err(self.invalid(format!("functions nest more than {MAX_DEPTH} deep")));
        }

        let source = self.text()?;
        let line_defined = self.word()?;
        let last_line_defined = self.word()?;
        let upvalues = self.byte()?;
        let parameters = self.byte()?;
        let varargs = self.byte()?;
        let max_stack = self.byte()?;

        let count = self.count(4, "instructions")?;
        let mut code = Vec::with_capacity(count);
        for _ in 0..count {
            code.push(Instruction(self.word()?));
        }

        let count = self.count(1, "constants")?;
        let mut constants = Vec::with_capacity(count);
        for _ in 0..count {
            constants.push(match self.byte()? {
                0 => Constant::Nil,
                1 => Constant::Boolean(self.byte()? != 0),
                3 => Constant::Number(self.number()?),
                4 => Constant::String(self.text()?.unwrap_or_default()),
                other => return Err(self.invalid(format!("constant type {other}"))),
            });
        }

        let count = self.count(FUNCTION_SIZE, "functions")?;
        let mut functions = Vec::with_capacity(count);
        for _ in 0..count {
            functions.push(self.function(depth + 1)?);
        }

        let count = self.count(4, "line entries")?;
        let mut lines = Vec::with_capacity(count);
        for _ in 0..count {
            lines.push(self.word()?);
        }

        let count = self.count(12, "locals")?;
        let mut locals = Vec::with_capacity(count);
        for _ in 0..count {
            locals.push(Local {
                name: self.text()?.unwrap_or_default(),
                start_pc: self.word()?,
                end_pc: self.word()?,
            });
        }

        let count = self.count(4, "upvalue names")?;
        let mut upvalue_names = Vec::with_capacity(count);
        for _ in 0..count {
            upvalue_names.push(self.text()?.unwrap_or_default());
        }

        Ok(Function {
            source,
            line_defined,
            last_line_defined,
            upvalues,
            parameters,
            varargs,
            max_stack,
            code,
            constants,
            functions,
            lines,
            locals,
            upvalue_names,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: [u8; 12] = [0x1B, b'L', b'u', b'a', 0x51, 0, 1, 4, 4, 4, 8, 0];

    fn text(bytes: &mut Vec<u8>, value: &[u8]) {
        bytes.extend(u32::try_from(value.len() + 1).unwrap().to_le_bytes());
        bytes.extend(value);
        bytes.push(0);
    }

    fn function(parameters: u8, varargs: u8, code: &[u32], constants: &[Constant]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend(0u32.to_le_bytes());
        bytes.extend(1u32.to_le_bytes());
        bytes.extend(2u32.to_le_bytes());
        bytes.extend([0, parameters, varargs, 3]);
        bytes.extend(u32::try_from(code.len()).unwrap().to_le_bytes());
        for word in code {
            bytes.extend(word.to_le_bytes());
        }
        bytes.extend(u32::try_from(constants.len()).unwrap().to_le_bytes());
        for constant in constants {
            match constant {
                Constant::Nil => bytes.push(0),
                Constant::Boolean(held) => bytes.extend([1, u8::from(*held)]),
                Constant::Number(held) => {
                    bytes.push(3);
                    bytes.extend(held.to_le_bytes());
                }
                Constant::String(held) => {
                    bytes.push(4);
                    text(&mut bytes, held);
                }
            }
        }
        bytes.extend([0; 16]);
        bytes
    }

    fn chunk(main: Vec<u8>) -> Vec<u8> {
        let mut bytes = HEADER.to_vec();
        bytes.extend(main);
        bytes
    }

    #[test]
    fn empty() {
        assert!(Chunk::parse(&[]).is_err());
    }

    #[test]
    fn truncated_code() {
        let mut bytes = chunk(function(0, 2, &[0x0000_0024, 0x0080_401C], &[]));
        bytes.truncate(bytes.len() - 2);
        assert!(Chunk::parse(&bytes).is_err());
    }

    /// A chunk built for another layout cannot be read as this one, so the widths are checked rather
    /// than assumed.
    #[test]
    fn a_chunk_of_another_layout_is_refused() {
        for (at, byte) in [
            (4, 0x52),
            (5, 1),
            (6, 0),
            (7, 8),
            (8, 8),
            (9, 8),
            (10, 4),
            (11, 1),
        ] {
            let mut bytes = chunk(function(0, 2, &[], &[]));
            bytes[at] = byte;
            assert!(
                Chunk::parse(&bytes).is_err(),
                "header byte {at} went unchecked"
            );
        }
    }

    #[test]
    fn trailing_bytes_are_refused() {
        let mut bytes = chunk(function(0, 2, &[], &[]));
        bytes.push(0);
        assert!(Chunk::parse(&bytes).is_err());
    }

    /// A count is checked against the bytes left before it is allocated for, so a malformed one is
    /// an error rather than a reservation the size of the number it happens to hold.
    #[test]
    fn an_oversized_count_is_refused() {
        let mut bytes = chunk(function(0, 2, &[0x0000_0024], &[]));
        let at = HEADER.len() + 16;
        bytes[at..at + 4].copy_from_slice(&0xFFFF_FFF0u32.to_le_bytes());
        assert!(Chunk::parse(&bytes).is_err());
    }

    #[test]
    fn a_stripped_chunk_keeps_only_the_lines_a_function_spans() {
        let constants = [
            Constant::String(b"print".to_vec()),
            Constant::Number(1.5),
            Constant::Boolean(true),
            Constant::Nil,
        ];
        let bytes = chunk(function(2, 3, &[0x0080_001E], &constants));
        let file = Chunk::parse(&bytes).unwrap();
        let main = file.main();
        assert_eq!(file.header().version, VERSION_51);
        assert_eq!((main.line_defined(), main.last_line_defined()), (1, 2));
        assert_eq!(main.max_stack(), 3);
        assert!(main.source().is_none());
        assert!(main.lines().is_empty());
        assert!(main.locals().is_empty());
        assert_eq!(main.constants(), &constants);
        assert_eq!(main.parameters(), 2);
        assert!(main.is_vararg() && main.has_arg() && !main.needs_arg());
    }

    /// The nine-bit fields sit above the eight-bit one, so a register wide enough to reach `B` is
    /// what tells a misread layout from a correct one.
    #[test]
    fn a_word_unpacks_into_its_fields() {
        let held = Instruction(0x8180_4009);
        assert_eq!(held.opcode(), Opcode::SetTable);
        assert_eq!((held.a(), held.b(), held.c()), (0, 259, 1));
        assert_eq!(Operand::from(held.b()), Operand::Constant(3));
        assert_eq!(Operand::from(held.c()), Operand::Register(1));

        let call = Instruction(0x0100_401C);
        assert_eq!(call.opcode(), Opcode::Call);
        assert_eq!((call.a(), call.b(), call.c()), (0, 2, 1));

        let loadk = Instruction(0x0000_4041);
        assert_eq!(loadk.opcode(), Opcode::LoadK);
        assert_eq!((loadk.a(), loadk.bx()), (1, 1));
    }

    /// A jump is biased rather than signed, so the halfway point is where it stands still.
    #[test]
    fn a_jump_reaches_both_ways() {
        assert_eq!(Instruction(131071 << 14 | 22).sbx(), 0);
        assert_eq!(Instruction(131072 << 14 | 22).sbx(), 1);
        assert_eq!(Instruction(131070 << 14 | 22).sbx(), -1);
    }

    #[test]
    fn every_opcode_is_named_by_its_number() {
        assert_eq!(Opcode::from(0), Opcode::Move);
        assert_eq!(Opcode::from(37), Opcode::Vararg);
        assert_eq!(Opcode::from(38), Opcode::Unknown(38));
        assert_eq!(Opcode::Closure.name(), "CLOSURE");
        assert_eq!(Opcode::Unknown(60).name(), "UNKNOWN");
    }
}
