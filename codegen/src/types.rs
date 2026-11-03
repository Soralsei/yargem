use clap::Parser;
use serde::{Deserialize, Deserializer};
use std::path::PathBuf;

// Shared ISA types come from `common`; re-exported so `use crate::types::*`
// still reaches them. Parsing yaml into them lives here, not in `common`.
pub use yargem_common::{Condition, Cycles, Operand, Reg16, Reg8};

#[derive(Debug, Deserialize)]
pub struct ConditionalTime {
    // Order matters: the yaml lists taken first.
    pub taken: usize,
    pub not_taken: usize,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Time {
    One(usize),
    Two(ConditionalTime),
}

impl Time {
    /// M-cycles, unconditional path (taken for conditionals).
    pub fn mcycles(&self) -> usize {
        match self {
            Time::One(t) => t / 4,
            Time::Two(c) => c.taken / 4,
        }
    }
    pub fn is_conditional(&self) -> bool {
        matches!(self, Time::Two(_))
    }
}

/// One yaml row. Codegen-only: `common::Instruction` is the runtime disasm
/// entry and a different shape.
#[derive(Debug, Deserialize)]
pub struct Instruction {
    pub code: u16,
    pub operator: String,
    #[serde(deserialize_with = "de_operands")]
    pub operands: Vec<Operand>,
    pub bits: usize,
    pub size: usize,
    pub time: Time,
    pub z: String,
    pub n: String,
    pub h: String,
    pub c: String,
}

impl Instruction {
    pub fn prefixed(&self) -> bool {
        self.code & 0xFF00 == 0xCB00
    }
    pub fn byte(&self) -> u8 {
        (self.code & 0xFF) as u8
    }
    /// Last operand: the ALU source / the RMW target / the jump target.
    pub fn last(&self) -> Option<&Operand> {
        self.operands.last()
    }
    /// First `Num` operand: the bit index (bit/res/set) or rst vector.
    pub fn index(&self) -> Option<u8> {
        self.operands.iter().find_map(|o| match o {
            Operand::Num(n) => Some(*n),
            _ => None,
        })
    }
    pub fn condition(&self) -> Option<Condition> {
        self.operands.iter().find_map(|o| match o {
            Operand::Cond(c) => Some(*c),
            _ => None,
        })
    }
}

// --- yaml -> common::Operand (the serde seam, codegen-local) ----------------

#[derive(Deserialize)]
#[serde(untagged)]
enum RawOperand {
    Int(u8),
    Str(String),
}

fn de_operands<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Operand>, D::Error> {
    let raw = Vec::<RawOperand>::deserialize(d)?;
    raw.into_iter()
        .map(|r| match r {
            RawOperand::Int(n) => Ok(Operand::Num(n)),
            RawOperand::Str(s) => parse_operand(&s),
        })
        .collect::<Result<_, String>>()
        .map_err(serde::de::Error::custom)
}

fn parse_operand(v: &str) -> Result<Operand, String> {
    if let Some(inner) = v.strip_prefix('(').and_then(|x| x.strip_suffix(')')) {
        if let Some(base) = inner.strip_prefix("0xff00+") {
            return match base {
                "c" => Ok(Operand::HighC),
                "a8" => Ok(Operand::HighImm8),
                _ => Err(format!("unknown high base '{base}'")),
            };
        }
        return match inner {
            "a16" => Ok(Operand::MemImm16),
            _ => Ok(Operand::Mem(parse_reg16(inner)?)),
        };
    }
    match v {
        "d8" | "a8" => return Ok(Operand::Imm8),
        "r8" => return Ok(Operand::Rel8),
        "d16" | "a16" => return Ok(Operand::Imm16),
        _ => {}
    }
    match v.to_lowercase().as_str() {
        "nz" => return Ok(Operand::Cond(Condition::NotZero)),
        "z" => return Ok(Operand::Cond(Condition::Zero)),
        "nc" => return Ok(Operand::Cond(Condition::NotCarry)),
        "cf" => return Ok(Operand::Cond(Condition::Carry)),
        _ => {}
    }
    if let Ok(r) = parse_reg8(v) {
        return Ok(Operand::Reg8(r));
    }
    if let Ok(r) = parse_reg16(v) {
        return Ok(Operand::Reg16(r));
    }
    if let Ok(n) = v.parse::<u8>() {
        return Ok(Operand::Num(n)); // quoted dummy, e.g. stop's "0"
    }
    Err(format!("unknown operand '{v}'"))
}

fn parse_reg8(v: &str) -> Result<Reg8, String> {
    Ok(match v.to_lowercase().as_str() {
        "a" => Reg8::A,
        "b" => Reg8::B,
        "c" => Reg8::C,
        "d" => Reg8::D,
        "e" => Reg8::E,
        "h" => Reg8::H,
        "l" => Reg8::L,
        _ => return Err(format!("unknown r8 '{v}'")),
    })
}

fn parse_reg16(v: &str) -> Result<Reg16, String> {
    Ok(match v.to_lowercase().as_str() {
        "af" => Reg16::AF,
        "bc" => Reg16::BC,
        "de" => Reg16::DE,
        "hl" => Reg16::HL,
        "sp" => Reg16::SP,
        "pc" => Reg16::PC,
        _ => return Err(format!("unknown r16 '{v}'")),
    })
}

#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
pub struct GenerateArgs {
    #[arg(long)]
    pub oplist: PathBuf,
    #[arg(long = "out")]
    pub output: PathBuf,
}
