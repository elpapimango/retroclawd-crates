//! The MC6850 against Motorola's data sheet (MC6850 "Asynchronous Communications Interface Adapter", DS9493R,
//! sections "Master Reset", "Control Register", "Status Register", "Peripheral/Modem Control").

use mc6850::Acia;

const STATUS: bool = false;
const DATA: bool = true;

fn reset_and(control: u8) -> Acia {
    let mut a = Acia::new();
    a.write(STATUS, 0x03);
    a.write(STATUS, control);
    a
}

/// "Master Reset": CR1-CR0 = 11 clears the status register except CTS and DCD, and IRQ is held high (inactive).
#[test]
fn master_reset_clears_status_and_irq() {
    let mut a = Acia::new();
    a.write(STATUS, 0x96);
    a.rx_byte(0x55);
    assert!(a.irq());
    a.write(STATUS, 0x03);
    assert_eq!(a.read(STATUS), 0x00);
    assert!(!a.irq());
    assert_eq!(a.divide(), 0);
}

/// `$96`: ÷64, 8 data + 1 stop, RTS low with the transmit interrupt disabled, receive interrupt enabled. Out of
/// reset the transmit data register is empty (status `$02`).
#[test]
fn control_96_is_div64_8n1_rx_irq() {
    let mut a = reset_and(0x96);
    assert_eq!(a.divide(), 64);
    assert_eq!(a.frame_bits(), 10);
    assert_eq!(a.read(STATUS), 0x02);
    assert!(!a.irq());
    let b = reset_and(0x95);
    assert_eq!(b.divide(), 16);
    assert_eq!(b.frame_bits(), 10);
    assert_eq!(reset_and(0x94).divide(), 1);
}

/// "Transmit": a write to the TDR clears TDRE ("transmission of a new character has not begun since the last write
/// data command"); TDRE sets again when the character moves to the shift register (`start_tx`, which the host
/// times), and the character leaves the shift register one frame later (`take_tx`).
#[test]
fn transmit_data_register_and_shift_register() {
    let mut a = reset_and(0x96);
    a.write(DATA, 0x80);
    assert_eq!(a.read(STATUS) & 0x02, 0, "TDR full");
    assert!(!a.tx_pending());
    assert!(a.start_tx());
    assert!(a.tx_pending());
    assert_eq!(a.read(STATUS) & 0x02, 0x02);
    a.write(DATA, 0x01);
    assert!(!a.start_tx(), "shift register busy");
    assert_eq!(a.take_tx(), Some(0x80));
    assert!(!a.tx_pending(), "the next character waits for start_tx");
    assert!(a.start_tx());
    assert_eq!(a.take_tx(), Some(0x01));
    assert_eq!(a.take_tx(), None);
}

/// CR6-CR5 = 01: TDRE raises IRQ.
#[test]
fn transmit_interrupt_when_enabled() {
    let a = reset_and(0x36); // ÷64, 8N1, RTS low + transmit interrupt
    assert!(a.irq());
    assert_eq!(a.peek(STATUS) & 0x80, 0x80);
}

/// "Receive Data Register Full": set by a received character, IRQ with CR7; reading the RDR clears both.
#[test]
fn receive_sets_rdrf_and_irq_until_read() {
    let mut a = reset_and(0x96);
    a.rx_byte(0xF1);
    assert_eq!(a.read(STATUS), 0x83);
    assert!(a.irq());
    assert_eq!(a.read(DATA), 0xF1);
    assert_eq!(a.read(STATUS), 0x02);
    assert!(!a.irq());
    let mut quiet = reset_and(0x16); // CR7 = 0
    quiet.rx_byte(1);
    assert!(!quiet.irq());
    assert_eq!(quiet.read(STATUS), 0x03);
}

/// "Receiver Overrun": a second character before the RDR is read is lost; OVRN appears once the valid character
/// has been read, RDRF stays set until the overrun is reset by the next data read.
#[test]
fn overrun_after_reading_the_valid_character() {
    let mut a = reset_and(0x96);
    a.rx_byte(0x11);
    a.rx_byte(0x22);
    assert_eq!(
        a.read(STATUS) & 0x20,
        0,
        "not shown before the valid character is read"
    );
    assert_eq!(a.read(DATA), 0x11);
    assert_eq!(a.read(STATUS) & 0x21, 0x21);
    assert!(a.irq());
    a.read(DATA);
    assert_eq!(a.read(STATUS) & 0x21, 0);
}

/// "Clear-to-Send": high inhibits TDRE and reads high in bit 3; "Data Carrier Detect": high sets bit 2 and IRQ
/// with CR7, latched until status then data are read.
#[test]
fn cts_and_dcd_inputs() {
    let mut a = reset_and(0x96);
    a.set_cts(true);
    assert_eq!(a.read(STATUS) & 0x0A, 0x08);
    a.set_cts(false);
    assert_eq!(a.read(STATUS) & 0x0A, 0x02);
    a.set_dcd(true);
    assert!(a.irq());
    a.set_dcd(false);
    assert_eq!(a.read(STATUS) & 0x04, 0x04, "latched");
    a.read(DATA);
    assert_eq!(a.read(STATUS) & 0x04, 0);
    assert!(!a.irq());
}

/// Word select CR4-CR2: start bit + data + parity + stop bits.
#[test]
fn frame_bits_for_every_word_select() {
    let expected = [11, 11, 10, 10, 11, 10, 11, 11];
    for (ws, bits) in expected.iter().enumerate() {
        let a = reset_and(0x01 | (ws as u8) << 2);
        assert_eq!(a.frame_bits(), *bits, "word select {ws}");
    }
}

/// `peek` has no side effects: RDRF survives it.
#[test]
fn peek_does_not_clear() {
    let mut a = reset_and(0x96);
    a.rx_byte(7);
    assert_eq!(a.peek(DATA), 7);
    assert_eq!(a.peek(STATUS) & 1, 1);
}
