use crate::Bus;
use crate::cpu::{Cpu, FLAG_C, FLAG_F3, FLAG_F5, FLAG_H, FLAG_N, FLAG_PV, FLAG_S, FLAG_Z};

mod base_arith16;
mod base_control;
mod base_incdec8;
mod base_loads;
mod base_stack_misc;
mod cb;
mod ed;
mod ed_block;
mod indexed_cb;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum IndexReg {
    Ix,
    Iy,
}

pub(crate) type Handler<B> = fn(&mut Cpu, &mut B) -> u8;
pub(crate) type IndexedCbHandler<B> = fn(&mut Cpu, &mut B, d: i8) -> u8;

/// Dispatch tables are generic over the bus type (`B: Bus`) rather than
/// built once against `&mut dyn Bus` -- this lets `Cpu::step` take `&mut B`
/// directly, so every `bus.read`/`bus.write` call in the hot path can be
/// inlined instead of going through a vtable indirect call on every single
/// memory access. Each of these is still built exactly once per concrete
/// `B` (not once per `step()` call): the `const { ... }` block forces
/// compile-time evaluation of the `const fn` table-builders below, which
/// the compiler promotes to a `'static` value -- same zero-runtime-cost
/// shape as the old top-level `const` tables, just monomorphized per bus
/// type instead of fixed to one array.
pub(crate) fn base_table<B: Bus + 'static>() -> &'static [Handler<B>; 256] {
    const { &build_base_table() }
}
pub(crate) fn cb_table<B: Bus + 'static>() -> &'static [Handler<B>; 256] {
    const { &cb::install([unimplemented; 256]) }
}
pub(crate) fn ed_table<B: Bus + 'static>() -> &'static [Handler<B>; 256] {
    const { &ed_block::install(ed::install([ed::undefined_ed; 256])) }
}
pub(crate) fn indexed_cb_table<B: Bus + 'static>() -> &'static [IndexedCbHandler<B>; 256] {
    const { &indexed_cb::install([unimplemented_indexed_cb; 256]) }
}

const fn build_base_table<B: Bus>() -> [Handler<B>; 256] {
    let mut table: [Handler<B>; 256] = [unimplemented; 256];
    table[0x00] = nop;

    let mut opcode = 0x40;
    while opcode <= 0x7F {
        table[opcode] = ld_r_r;
        opcode += 1;
    }
    table[0x76] = unimplemented; // HALT, not LD (HL),(HL) -- filled in by base_stack_misc

    let mut opcode = 0x80;
    while opcode <= 0xBF {
        table[opcode] = alu_a_r;
        opcode += 1;
    }

    let table = base_loads::install(table);
    let table = base_incdec8::install(table);
    let table = base_arith16::install(table);
    let table = base_control::install(table);

    base_stack_misc::install(table)
}

pub(crate) fn unimplemented<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let opcode = bus.read(cpu.pc.wrapping_sub(1));
    panic!("unimplemented opcode 0x{opcode:02X}");
}

fn unimplemented_indexed_cb<B: Bus>(cpu: &mut Cpu, bus: &mut B, _d: i8) -> u8 {
    let opcode = bus.read(cpu.pc.wrapping_sub(1));
    panic!("unimplemented indexed CB opcode 0x{opcode:02X}");
}

fn nop<B: Bus>(_cpu: &mut Cpu, _bus: &mut B) -> u8 {
    4
}

/// Fetches the displacement byte following a DD/FD-prefixed opcode
/// (advancing `pc` past it) and returns `base + d` as used by `(IX+d)`/`(IY+d)`
/// addressing. Also charges the 8 extra T-states (3 for the displacement
/// fetch, 5 for the address calculation) that plain `(HL)` addressing
/// doesn't incur, via `cpu.pending_cycles` -- this total is independent of
/// (i.e. added on top of) any contention delay `mem_read` itself charges for
/// the displacement byte fetch.
pub(crate) fn indexed_addr<B: Bus>(cpu: &mut Cpu, bus: &mut B, base: u16) -> u16 {
    let d = cpu.mem_read(bus, cpu.pc) as i8;
    cpu.pc = cpu.pc.wrapping_add(1);
    cpu.pending_cycles += 8;
    // The address-calculation portion (5 of the 8 T-states above) touches no
    // bus address at all, so it can't be contended -- but it still consumes
    // real time, so the bus's own frame-position tracker (used by later
    // contention lookups, e.g. the actual `(IX+d)` access this address is
    // for) needs to advance past it explicitly, unlike `mem_read`'s 3 T-state
    // base cost above, which already advanced it as part of that call.
    bus.advance_tstates(5);
    base.wrapping_add(d as i16 as u16)
}

/// Reads one of the 8 standard Z80 register-field targets (3-bit encoding:
/// B,C,D,E,H,L,(HL),A). H/L become IXH/IXL/IYH/IYL and (HL) becomes
/// (IX+d)/(IY+d) when `cpu.active_index` is set, so every caller of this
/// (LD r,r', ALU A,r, INC/DEC r, CB rotate/BIT/RES/SET) gets DD/FD
/// substitution for free.
pub(crate) fn reg_get<B: Bus>(cpu: &mut Cpu, bus: &mut B, idx: u8) -> u8 {
    match idx {
        0 => cpu.b(),
        1 => cpu.c(),
        2 => cpu.d(),
        3 => cpu.e(),
        4 => match cpu.active_index {
            None => cpu.h(),
            Some(IndexReg::Ix) => cpu.ixh(),
            Some(IndexReg::Iy) => cpu.iyh(),
        },
        5 => match cpu.active_index {
            None => cpu.l(),
            Some(IndexReg::Ix) => cpu.ixl(),
            Some(IndexReg::Iy) => cpu.iyl(),
        },
        6 => {
            let addr = match cpu.active_index {
                None => cpu.hl,
                Some(IndexReg::Ix) => indexed_addr(cpu, bus, cpu.ix),
                Some(IndexReg::Iy) => indexed_addr(cpu, bus, cpu.iy),
            };
            cpu.mem_read(bus, addr)
        }
        7 => cpu.a(),
        _ => unreachable!("register index out of range: {idx}"),
    }
}

pub(crate) fn reg_set<B: Bus>(cpu: &mut Cpu, bus: &mut B, idx: u8, val: u8) {
    match idx {
        0 => cpu.set_b(val),
        1 => cpu.set_c(val),
        2 => cpu.set_d(val),
        3 => cpu.set_e(val),
        4 => match cpu.active_index {
            None => cpu.set_h(val),
            Some(IndexReg::Ix) => cpu.set_ixh(val),
            Some(IndexReg::Iy) => cpu.set_iyh(val),
        },
        5 => match cpu.active_index {
            None => cpu.set_l(val),
            Some(IndexReg::Ix) => cpu.set_ixl(val),
            Some(IndexReg::Iy) => cpu.set_iyl(val),
        },
        6 => {
            let addr = match cpu.active_index {
                None => cpu.hl,
                Some(IndexReg::Ix) => indexed_addr(cpu, bus, cpu.ix),
                Some(IndexReg::Iy) => indexed_addr(cpu, bus, cpu.iy),
            };
            cpu.mem_write(bus, addr, val);
        }
        7 => cpu.set_a(val),
        _ => unreachable!("register index out of range: {idx}"),
    }
}

/// Reads B/C/D/E/H/L/A (never IXH/IXL/IYH/IYL) regardless of `active_index`.
/// There's no such opcode as "LD IXH,(IX+d)" -- when an LD r,r' operand is
/// (HL)/(IX+d)/(IY+d), the OTHER operand's H/L register-field values always
/// mean the real H/L, even under a DD/FD prefix.
fn real_reg_get(cpu: &Cpu, idx: u8) -> u8 {
    match idx {
        0 => cpu.b(),
        1 => cpu.c(),
        2 => cpu.d(),
        3 => cpu.e(),
        4 => cpu.h(),
        5 => cpu.l(),
        7 => cpu.a(),
        _ => unreachable!("real_reg_get called with indirect index: {idx}"),
    }
}

fn real_reg_set(cpu: &mut Cpu, idx: u8, val: u8) {
    match idx {
        0 => cpu.set_b(val),
        1 => cpu.set_c(val),
        2 => cpu.set_d(val),
        3 => cpu.set_e(val),
        4 => cpu.set_h(val),
        5 => cpu.set_l(val),
        7 => cpu.set_a(val),
        _ => unreachable!("real_reg_set called with indirect index: {idx}"),
    }
}

fn ld_r_r<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let opcode = bus.read(cpu.pc.wrapping_sub(1));
    let dest = (opcode >> 3) & 0x07;
    let src = opcode & 0x07;

    // Substitution to IXH/IXL/IYH/IYL only happens when NEITHER operand is
    // the (HL)-role memory operand -- see real_reg_get's doc comment.
    let involves_indirect = dest == 6 || src == 6;

    let val = if involves_indirect && src != 6 {
        real_reg_get(cpu, src)
    } else {
        reg_get(cpu, bus, src)
    };

    if involves_indirect && dest != 6 {
        real_reg_set(cpu, dest, val);
    } else {
        reg_set(cpu, bus, dest, val);
    }

    if dest == 6 || src == 6 { 7 } else { 4 }
}

pub(crate) fn parity(val: u8) -> bool {
    val.count_ones().is_multiple_of(2)
}

/// Packs S/Z/H/PV/N/C into an F-register byte (bits 3/5 left as 0).
pub(crate) fn flags_byte(s: bool, z: bool, h: bool, pv: bool, n: bool, c: bool) -> u8 {
    let mut f = 0u8;
    if s {
        f |= FLAG_S;
    }
    if z {
        f |= FLAG_Z;
    }
    if h {
        f |= FLAG_H;
    }
    if pv {
        f |= FLAG_PV;
    }
    if n {
        f |= FLAG_N;
    }
    if c {
        f |= FLAG_C;
    }
    f
}

/// Applies `flags` (from `flags_byte`) plus the undocumented F3/F5 bits
/// copied from `f3f5_source`'s bits 3/5. Every ALU op copies these from
/// its own result, except CP, which copies them from the operand.
pub(crate) fn apply_alu_flags(cpu: &mut Cpu, flags: u8, f3f5_source: u8) {
    cpu.set_f(flags | (f3f5_source & (FLAG_F3 | FLAG_F5)));
}

fn add_a(cpu: &mut Cpu, val: u8, carry_in: u8) {
    let a = cpu.a();
    let sum = a as u16 + val as u16 + carry_in as u16;
    let result = sum as u8;
    let carry = sum > 0xFF;
    let half = (a & 0x0F) + (val & 0x0F) + carry_in > 0x0F;
    let overflow = (a ^ val) & 0x80 == 0 && (a ^ result) & 0x80 != 0;
    let flags = flags_byte(
        result & 0x80 != 0,
        result == 0,
        half,
        overflow,
        false,
        carry,
    );
    apply_alu_flags(cpu, flags, result);
    cpu.set_a(result);
}

/// Returns (result, sign, zero, half-borrow, overflow, carry/borrow) for `a - val - carry_in`.
fn sub_core(a: u8, val: u8, carry_in: u8) -> (u8, bool, bool, bool, bool, bool) {
    let diff = a as i16 - val as i16 - carry_in as i16;
    let result = diff as u8;
    let carry = diff < 0;
    let half = ((a & 0x0F) as i16 - (val & 0x0F) as i16 - carry_in as i16) < 0;
    let overflow = (a ^ val) & 0x80 != 0 && (a ^ result) & 0x80 != 0;
    (
        result,
        result & 0x80 != 0,
        result == 0,
        half,
        overflow,
        carry,
    )
}

fn sub_a(cpu: &mut Cpu, val: u8, carry_in: u8) {
    let (result, s, z, h, pv, c) = sub_core(cpu.a(), val, carry_in);
    let flags = flags_byte(s, z, h, pv, true, c);
    apply_alu_flags(cpu, flags, result);
    cpu.set_a(result);
}

fn cp_a(cpu: &mut Cpu, val: u8) {
    let (_result, s, z, h, pv, c) = sub_core(cpu.a(), val, 0);
    let flags = flags_byte(s, z, h, pv, true, c);
    // Undocumented: CP copies F3/F5 from the operand, not the result.
    apply_alu_flags(cpu, flags, val);
    // A is left unchanged
}

fn and_a(cpu: &mut Cpu, val: u8) {
    let result = cpu.a() & val;
    let flags = flags_byte(
        result & 0x80 != 0,
        result == 0,
        true,
        parity(result),
        false,
        false,
    );
    apply_alu_flags(cpu, flags, result);
    cpu.set_a(result);
}

fn xor_a(cpu: &mut Cpu, val: u8) {
    let result = cpu.a() ^ val;
    let flags = flags_byte(
        result & 0x80 != 0,
        result == 0,
        false,
        parity(result),
        false,
        false,
    );
    apply_alu_flags(cpu, flags, result);
    cpu.set_a(result);
}

fn or_a(cpu: &mut Cpu, val: u8) {
    let result = cpu.a() | val;
    let flags = flags_byte(
        result & 0x80 != 0,
        result == 0,
        false,
        parity(result),
        false,
        false,
    );
    apply_alu_flags(cpu, flags, result);
    cpu.set_a(result);
}

fn alu_a_r<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> u8 {
    let opcode = bus.read(cpu.pc.wrapping_sub(1));
    let op = (opcode >> 3) & 0x07;
    let src = opcode & 0x07;

    let val = reg_get(cpu, bus, src);
    let carry_in = u8::from(cpu.f() & FLAG_C != 0);

    match op {
        0 => add_a(cpu, val, 0),
        1 => add_a(cpu, val, carry_in),
        2 => sub_a(cpu, val, 0),
        3 => sub_a(cpu, val, carry_in),
        4 => and_a(cpu, val),
        5 => xor_a(cpu, val),
        6 => or_a(cpu, val),
        7 => cp_a(cpu, val),
        _ => unreachable!("ALU op out of range: {op}"),
    }

    if src == 6 { 7 } else { 4 }
}
