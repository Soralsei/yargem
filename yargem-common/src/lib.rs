//! Shared instruction-set types. Single source of truth so codegen and backend
//! cannot drift. Types only — no serde, no parsing, no dependencies.

use std::fmt::Display;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reg8 {
    A,
    B,
    C,
    D,
    E,
    F,
    H,
    L,
    W,
    Z, // fetch latches
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reg16 {
    AF,
    BC,
    DE,
    HL,
    SP,
    PC,
    WZ,
}

impl From<Reg16> for &str {
    fn from(value: Reg16) -> Self {
        match value {
            Reg16::AF => "AF",
            Reg16::BC => "BC",
            Reg16::DE => "DE",
            Reg16::HL => "HL",
            Reg16::SP => "SP",
            Reg16::PC => "PC",
            Reg16::WZ => "WZ",
        }
    }
}

impl Display for Reg16 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label: &str = (*self).into();
        write!(f, "{}", label)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Condition {
    Unconditional,
    NotZero,
    Zero,
    NotCarry,
    Carry,
}

/// One operand in the yaml's vocabulary. The disasm table holds it as data;
/// codegen lowers it to execution steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operand {
    Reg8(Reg8),   // a..l
    Reg16(Reg16), // bc..sp
    Imm8,         // d8, a8
    Rel8,         // r8
    Imm16,        // d16, a16
    Mem(Reg16),   // (bc) (de) (hl)
    MemImm16,     // (a16)
    HighC,        // (0xff00+c)
    HighImm8,     // (0xff00+a8)
    Cond(Condition),
    Num(u8), // bit index, rst vector, dummy
}

/// Cycle cost for the disasm table.
#[derive(Debug, Clone, Copy)]
pub enum Cycles {
    Unconditional(usize),
    Conditional { taken: usize, not_taken: usize },
}

/// A decoded instruction, indexable by opcode. Fully const — no execute field.
#[derive(Debug, Clone, Copy)]
pub struct Instruction {
    pub operator: &'static str,
    pub operands: &'static [Operand],
    pub size: usize,
    pub cycles: Cycles,
}
