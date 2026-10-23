//! Motorola MC6850 Asynchronous Communications Interface Adapter.
//!
//! Everything here follows Motorola's data sheet (MC6850, DS9493R, 1985): the control register (counter divide
//! CR1-CR0, word select CR4-CR2, transmitter control CR6-CR5, receive interrupt enable CR7), the status register
//! (RDRF, TDRE, DCD, CTS, FE, OVRN, PE, IRQ) and the master reset.
//!
//! The crate has no clock. The host computes a character's time on the line as `frame_bits() × divide()` cycles of
//! the ACIA's transmit/receive clock and calls [`Acia::start_tx`] when the transmitter takes a written character, [`Acia::take_tx`] when the shift register has sent it, and
//! [`Acia::rx_byte`] when one has arrived.
#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

const RDRF: u8 = 0x01;
const TDRE: u8 = 0x02;
const DCD: u8 = 0x04;
const CTS: u8 = 0x08;
const OVRN: u8 = 0x20;
const IRQ: u8 = 0x80;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Acia {
    control: u8,
    /// In master reset (CR1-CR0 = 11), or never initialised since power-up: the data sheet requires a master reset
    /// "after power-on", so the chip starts here.
    in_reset: bool,
    tdr: Option<u8>,
    /// The character being shifted out.
    shift: Option<u8>,
    rdr: u8,
    rdrf: bool,
    /// A character arrived while RDRF was set: the overrun shows once the valid character has been read.
    overrun_pending: bool,
    ovrn: bool,
    cts_in: bool,
    dcd_in: bool,
    /// The DCD status bit, latched high until status then data are read.
    dcd_latch: bool,
    status_read_since_dcd: bool,
}

impl Acia {
    /// Power-up: no master reset has been done yet.
    pub fn new() -> Self {
        Self {
            in_reset: true,
            ..Self::default()
        }
    }

    /// Status (`rs` false) or receive data (`rs` true), with the read side effects.
    pub fn read(&mut self, rs: bool) -> u8 {
        if !rs {
            let s = self.status();
            if self.dcd_latch {
                self.status_read_since_dcd = true;
            }
            return s;
        }
        let byte = self.rdr;
        if self.ovrn {
            // "The Overrun indication is reset after the reading of data from the Receive Data Register".
            self.ovrn = false;
            self.rdrf = false;
        } else if self.overrun_pending {
            self.overrun_pending = false;
            self.ovrn = true;
        } else {
            self.rdrf = false;
        }
        if self.dcd_latch && self.status_read_since_dcd {
            self.dcd_latch = self.dcd_in;
            self.status_read_since_dcd = false;
        }
        byte
    }

    /// The same value as `read`, without side effects.
    pub fn peek(&self, rs: bool) -> u8 {
        if rs { self.rdr } else { self.status() }
    }

    /// Control (`rs` false) or transmit data (`rs` true).
    pub fn write(&mut self, rs: bool, value: u8) {
        if rs {
            if self.in_reset {
                return;
            }
            self.tdr = Some(value);
            return;
        }
        self.control = value;
        if value & 0x03 == 0x03 {
            // "clears the Status Register (except for external conditions on CTS and DCD) and initializes both the
            // receiver and transmitter".
            self.in_reset = true;
            self.tdr = None;
            self.shift = None;
            self.rdrf = false;
            self.overrun_pending = false;
            self.ovrn = false;
            self.dcd_latch = self.dcd_in;
            self.status_read_since_dcd = false;
        } else {
            self.in_reset = false;
        }
    }

    /// The IRQ output is asserted (it is active low on the pin).
    pub fn irq(&self) -> bool {
        if self.in_reset {
            return false;
        }
        let rx = self.control & 0x80 != 0 && (self.rdrf || self.ovrn || self.dcd_latch);
        let tx = self.control & 0x60 == 0x20 && self.tdre();
        rx || tx
    }

    /// The counter divide ratio (1, 16 or 64), or 0 in master reset.
    pub fn divide(&self) -> u32 {
        if self.in_reset {
            return 0;
        }
        match self.control & 0x03 {
            0 => 1,
            1 => 16,
            _ => 64,
        }
    }

    /// Bits per character on the line: start bit, data bits, parity bit and stop bits (word select CR4-CR2).
    pub fn frame_bits(&self) -> u32 {
        match (self.control >> 2) & 0x07 {
            0 | 1 => 1 + 7 + 1 + 2,
            2 | 3 => 1 + 7 + 1 + 1,
            4 => 1 + 8 + 2,
            5 => 1 + 8 + 1,
            _ => 1 + 8 + 1 + 1,
        }
    }

    /// The transmitter takes the TDR's character into the shift register, if it is free (TDRE sets). Returns
    /// whether a character started; the host then calls `take_tx` one frame later.
    pub fn start_tx(&mut self) -> bool {
        if self.in_reset || self.shift.is_some() || self.tdr.is_none() || self.cts_in {
            return false;
        }
        self.shift = self.tdr.take();
        true
    }

    /// The character in the shift register has been sent: hand it over.
    pub fn take_tx(&mut self) -> Option<u8> {
        self.shift.take()
    }

    /// A character is in the shift register, being sent.
    pub fn tx_pending(&self) -> bool {
        self.shift.is_some()
    }

    /// A complete character arrived on the receive line.
    pub fn rx_byte(&mut self, byte: u8) {
        if self.in_reset || self.dcd_in {
            return;
        }
        if self.rdrf {
            self.overrun_pending = true;
            return;
        }
        self.rdr = byte;
        self.rdrf = true;
    }

    /// The CTS input: high inhibits TDRE.
    pub fn set_cts(&mut self, high: bool) {
        self.cts_in = high;
    }

    /// The DCD input: a rise sets the latched DCD status bit (and IRQ with CR7).
    pub fn set_dcd(&mut self, high: bool) {
        if high && !self.dcd_in {
            self.dcd_latch = true;
            self.status_read_since_dcd = false;
        }
        self.dcd_in = high;
    }

    fn tdre(&self) -> bool {
        !self.in_reset && self.tdr.is_none() && !self.cts_in
    }

    fn status(&self) -> u8 {
        let mut s = 0;
        if self.rdrf && !self.dcd_in {
            s |= RDRF;
        }
        if self.tdre() {
            s |= TDRE;
        }
        if self.dcd_latch {
            s |= DCD;
        }
        if self.cts_in {
            s |= CTS;
        }
        if self.ovrn {
            s |= OVRN;
        }
        if self.irq() {
            s |= IRQ;
        }
        s
    }
}
