//! Motorola MC68000 CPU core.
//!
//! Sources: *M68000 8-/16-/32-Bit Microprocessors User's Manual*, ninth edition (Motorola, 1993; M68000UM/AD),
//! cited as "UM" with a section number, and the *M68000 Family Programmer's Reference Manual* (M68000PM/AD),
//! cited as "PRM". Instruction behaviour and cycle counts are checked against SingleStepTests/680x0
//! (`tests/singlestep.rs`, fetched at test time).
#![forbid(unsafe_code)]

mod bus;
mod cpu;
mod ea;
mod exception;
pub mod opcodes;
mod ops;

pub use bus::{BusFault, FunctionCode, IackResponse, M68kBus, Size};
pub use cpu::{Cpu, RunState};
