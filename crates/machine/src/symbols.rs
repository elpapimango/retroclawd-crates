//! Symbol tables for disassembly views: address -> name, built-in per machine (ROM entry points,
//! hardware registers) and loadable from a user's symbol file (assembler map, VICE labels, ...).
//!
//! CPU-agnostic on purpose: [`SymbolTable::annotate`] works on the *text* a disassembler produced, so
//! the same code serves every CPU. It looks for addresses written the way our disassemblers write
//! them -- `$1234`/`$12` (6502) and `0x1234` (Z80) -- and appends `  ; NAME` for the first one it
//! knows. 16-bit addresses (four digits) match in both styles; two-digit `$12` zero-page addresses
//! match only for the 6502 style and not behind a `#` (an immediate value is not an address), and Z80
//! `0x12` byte values never match.

use std::collections::BTreeMap;

/// Address -> name, later insertions replace earlier ones (so a user's file overrides the built-ins).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SymbolTable {
    by_addr: BTreeMap<u16, String>,
}

impl SymbolTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// A table from a static list such as a machine's ROM entry points.
    pub fn from_entries(entries: &[(u16, &str)]) -> Self {
        let mut table = Self::new();
        for (addr, name) in entries {
            table.insert(*addr, name);
        }
        table
    }

    pub fn insert(&mut self, addr: u16, name: &str) {
        self.by_addr.insert(addr, name.to_string());
    }

    /// Adds every entry of `other`, replacing names at the same address.
    pub fn merge(&mut self, other: &SymbolTable) {
        for (addr, name) in &other.by_addr {
            self.by_addr.insert(*addr, name.clone());
        }
    }

    pub fn name(&self, addr: u16) -> Option<&str> {
        self.by_addr.get(&addr).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.by_addr.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_addr.is_empty()
    }

    /// The address of the symbol called `name` (ignoring case), if there is one. Two addresses with the same name give the lower
    /// one.
    pub fn address(&self, name: &str) -> Option<u16> {
        self.by_addr
            .iter()
            .find(|(_, n)| n.eq_ignore_ascii_case(name))
            .map(|(addr, _)| *addr)
    }

    /// `text` (one line of assembly) with every symbol name in its operands replaced by its address written as `prefix` + four hex
    /// digits (`"$"` for the 6502 assembler, `"0x"` for the Z80's): the input-side counterpart of [`annotate`](Self::annotate), so
    /// `JSR CHROUT` assembles. The first word (the mnemonic), register and condition names (`A`, `HL`, `NZ`, ...), text after a
    /// `;` and anything already written as a number are left alone, as is a name the table does not know.
    pub fn substitute(&self, text: &str, prefix: &str) -> String {
        if self.is_empty() {
            return text.to_string();
        }
        let (code, comment) = match text.find(';') {
            Some(at) => text.split_at(at),
            None => (text, ""),
        };
        let mut out = String::with_capacity(text.len());
        let mut chars = code.char_indices().peekable();
        let mut words = 0;
        while let Some((start, c)) = chars.next() {
            let starts_word = c.is_ascii_alphabetic() || c == '_';
            // A number such as `$1A` or `0x1A` or `12h` is one token that merely starts with a digit or `$`.
            if !starts_word {
                out.push(c);
                if c.is_ascii_digit() || c == '$' {
                    while let Some(&(_, next)) = chars.peek() {
                        if next.is_ascii_alphanumeric() {
                            out.push(next);
                            chars.next();
                        } else {
                            break;
                        }
                    }
                }
                continue;
            }
            let mut end = start + c.len_utf8();
            while let Some(&(at, next)) = chars.peek() {
                if next.is_ascii_alphanumeric() || next == '_' || next == '.' {
                    end = at + next.len_utf8();
                    chars.next();
                } else {
                    break;
                }
            }
            let word = &code[start..end];
            words += 1;
            let known = (words > 1 && !is_register_or_condition(word))
                .then(|| self.address(word))
                .flatten();
            match known {
                Some(addr) => out.push_str(&format!("{prefix}{addr:04X}")),
                None => out.push_str(word),
            }
        }
        out.push_str(comment);
        out
    }

    /// Parses a symbol file. One symbol per line, `;`, `#` and `//` start comments, blank lines are ignored:
    ///
    /// * `NAME = $1234`, `NAME EQU 0x1234`, `NAME equ 1234h`, `NAME = 4660` (assembler listings; a value
    ///   with `$`, `0x` or an `h` suffix is hex, a bare number after `=`/`EQU` is decimal)
    /// * `NAME 1234`, `NAME $1234`, `NAME 001234 RLA` (map files: a bare number is hex; extra columns ignored)
    /// * `al C:1234 .NAME`, `al 1234 .NAME` (VICE monitor labels)
    ///
    /// An unreadable line is an error naming the line, never silently skipped.
    pub fn parse(text: &str) -> Result<SymbolTable, String> {
        let mut table = SymbolTable::new();
        for (index, raw) in text.lines().enumerate() {
            let line = strip_comment(raw).trim();
            if line.is_empty() {
                continue;
            }
            let (name, addr) = parse_line(line)
                .map_err(|e| format!("symbol file line {}: {e}: \"{raw}\"", index + 1))?;
            table.insert(addr, &name);
        }
        Ok(table)
    }

    /// Appends `  ; NAME` for the first known address in `text`; `text` is returned unchanged when there
    /// is none or it already carries a comment.
    pub fn annotate(&self, text: &str) -> String {
        if self.is_empty() || text.contains(';') {
            return text.to_string();
        }
        match self.find_in(text) {
            Some(name) => format!("{text}  ; {name}"),
            None => text.to_string(),
        }
    }

    fn find_in(&self, text: &str) -> Option<&str> {
        let bytes = text.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let (digits_at, style) = if bytes[i] == b'$' {
                (i + 1, Style::Dollar)
            } else if bytes[i] == b'0' && bytes.get(i + 1).is_some_and(|c| *c == b'x' || *c == b'X')
            {
                (i + 2, Style::ZeroX)
            } else {
                i += 1;
                continue;
            };
            let digits = bytes[digits_at.min(bytes.len())..]
                .iter()
                .take_while(|c| c.is_ascii_hexdigit())
                .count();
            let after_hash = i > 0 && bytes[i - 1] == b'#';
            let usable = match (style, digits) {
                (_, 4) => true,
                (Style::Dollar, 2) => !after_hash,
                _ => false,
            };
            if usable
                && let Ok(addr) = u16::from_str_radix(&text[digits_at..digits_at + digits], 16)
                && let Some(name) = self.name(addr)
            {
                return Some(name);
            }
            i = digits_at + digits.max(1);
        }
        None
    }
}

/// Names an operand can be that are not symbols: the Z80's and 6502's registers and the Z80's condition codes.
fn is_register_or_condition(word: &str) -> bool {
    const RESERVED: &[&str] = &[
        "A", "B", "C", "D", "E", "H", "L", "F", "I", "R", "X", "Y", "AF", "BC", "DE", "HL", "SP",
        "IX", "IY", "PC", "IXH", "IXL", "IYH", "IYL", "AF'", "NZ", "Z", "NC", "PO", "PE", "P", "M",
    ];
    RESERVED.iter().any(|r| r.eq_ignore_ascii_case(word))
}

#[derive(Clone, Copy)]
enum Style {
    Dollar,
    ZeroX,
}

fn strip_comment(line: &str) -> &str {
    let mut end = line.len();
    for marker in [";", "#", "//"] {
        if let Some(at) = line.find(marker) {
            end = end.min(at);
        }
    }
    &line[..end]
}

/// A number with `$`/`0x` (hex), a trailing `h` (hex) or, if `bare_hex`, plain digits as hex.
fn number(token: &str, bare_hex: bool) -> Result<u16, String> {
    let t = token.trim();
    let (digits, radix) = if let Some(h) = t.strip_prefix('$') {
        (h, 16)
    } else if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        (h, 16)
    } else if let Some(h) = t.strip_suffix('h').or_else(|| t.strip_suffix('H')) {
        (h, 16)
    } else if bare_hex {
        (t, 16)
    } else {
        (t, 10)
    };
    let value =
        u32::from_str_radix(digits, radix).map_err(|_| format!("bad address \"{token}\""))?;
    // map files write six digits ("001234"): fine as long as it fits in 16 bits
    u16::try_from(value).map_err(|_| format!("address \"{token}\" does not fit in 16 bits"))
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '@' | '$'))
}

fn parse_line(line: &str) -> Result<(String, u16), String> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    // VICE: al [C:]ADDR .NAME
    if tokens.first().is_some_and(|t| t.eq_ignore_ascii_case("al")) && tokens.len() >= 3 {
        let addr = tokens[1].rsplit(':').next().unwrap_or(tokens[1]);
        let name = tokens[2].trim_start_matches('.');
        if !valid_name(name) {
            return Err(format!("bad name \"{name}\""));
        }
        return Ok((name.to_string(), number(addr, true)?));
    }
    // NAME = VALUE  /  NAME EQU VALUE
    let assignment =
        if tokens.len() >= 3 && (tokens[1] == "=" || tokens[1].eq_ignore_ascii_case("equ")) {
            Some((tokens[0], tokens[2]))
        } else if let Some((n, v)) = line.split_once('=') {
            Some((n.trim(), v.trim()))
        } else {
            None
        };
    if let Some((name, value)) = assignment {
        let name = name.trim_end_matches(':');
        if !valid_name(name) {
            return Err(format!("bad name \"{name}\""));
        }
        return Ok((name.to_string(), number(value, false)?));
    }
    // NAME ADDR [more columns]
    if tokens.len() >= 2 {
        let name = tokens[0].trim_end_matches(':');
        if !valid_name(name) {
            return Err(format!("bad name \"{name}\""));
        }
        return Ok((name.to_string(), number(tokens[1], true)?));
    }
    Err("expected NAME = ADDRESS".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> SymbolTable {
        SymbolTable::from_entries(&[
            (0xFFD2, "CHROUT"),
            (0xD020, "BORDER"),
            (0x00C6, "NDX"),
            (0x0556, "LD_BYTES"),
        ])
    }

    #[test]
    fn annotates_six_five_o_two_and_z80_address_styles() {
        let t = table();
        assert_eq!(t.annotate("JSR $FFD2"), "JSR $FFD2  ; CHROUT");
        assert_eq!(t.annotate("STA $D020"), "STA $D020  ; BORDER");
        assert_eq!(t.annotate("LDA $C6"), "LDA $C6  ; NDX");
        assert_eq!(t.annotate("STA $D020,X"), "STA $D020,X  ; BORDER");
        assert_eq!(t.annotate("JMP ($FFD2)"), "JMP ($FFD2)  ; CHROUT");
        assert_eq!(t.annotate("CALL 0x0556"), "CALL 0x0556  ; LD_BYTES");
        assert_eq!(t.annotate("CALL NZ,0x0556"), "CALL NZ,0x0556  ; LD_BYTES");
    }

    #[test]
    fn leaves_values_that_are_not_addresses_alone() {
        let t = table();
        assert_eq!(t.annotate("LDA #$C6"), "LDA #$C6"); // an immediate, not the zero-page cell
        assert_eq!(t.annotate("LD A,0xC6"), "LD A,0xC6"); // Z80 byte value
        assert_eq!(t.annotate("LDA $1234"), "LDA $1234"); // unknown address
        assert_eq!(t.annotate("RTS"), "RTS");
        assert_eq!(t.annotate("JSR $FFD2  ; mine"), "JSR $FFD2  ; mine"); // already commented
        assert_eq!(SymbolTable::new().annotate("JSR $FFD2"), "JSR $FFD2");
    }

    #[test]
    fn parses_assembler_map_and_vice_formats() {
        let text = "; comment
CHROUT = $FFD2
BORDER equ 0xD020
NDX = 198        ; decimal after =
LDB EQU 0556h
GETIN  FFE4
ld65label 001000 RLA
al C:c000 .start
al d800 .colour
";
        let t = SymbolTable::parse(text).unwrap();
        assert_eq!(t.name(0xFFD2), Some("CHROUT"));
        assert_eq!(t.name(0xD020), Some("BORDER"));
        assert_eq!(t.name(198), Some("NDX"));
        assert_eq!(t.name(0x0556), Some("LDB"));
        assert_eq!(t.name(0xFFE4), Some("GETIN"));
        assert_eq!(t.name(0x1000), Some("ld65label"));
        assert_eq!(t.name(0xC000), Some("start"));
        assert_eq!(t.name(0xD800), Some("colour"));
        assert_eq!(t.len(), 8);
    }

    #[test]
    fn a_bad_line_is_an_error_with_its_line_number() {
        let err = SymbolTable::parse("OK = $10\nthis is not a symbol line\n").unwrap_err();
        assert!(err.contains("line 2"), "{err}");
        assert!(
            SymbolTable::parse("X = $12345")
                .unwrap_err()
                .contains("16 bits")
        );
        assert!(
            SymbolTable::parse("1BAD = $10")
                .unwrap_err()
                .contains("bad name")
        );
    }

    #[test]
    fn later_entries_replace_earlier_ones() {
        let mut t = table();
        t.merge(&SymbolTable::from_entries(&[(0xFFD2, "PRINT_CHAR")]));
        assert_eq!(t.annotate("JSR $FFD2"), "JSR $FFD2  ; PRINT_CHAR");
    }

    #[test]
    fn substitute_turns_names_into_addresses_for_either_assembler() {
        let table = SymbolTable::from_entries(&[
            (0xFFD2, "CHROUT"),
            (0x0556, "LD_BYTES"),
            (0xD020, "BORDER"),
        ]);
        assert_eq!(table.substitute("JSR CHROUT", "$"), "JSR $FFD2");
        assert_eq!(table.substitute("jsr chrout", "$"), "jsr $FFD2");
        assert_eq!(table.substitute("CALL LD_BYTES", "0x"), "CALL 0x0556");
        assert_eq!(
            table.substitute("LDA #$05", "$"),
            "LDA #$05",
            "numbers are left alone"
        );
        assert_eq!(table.substitute("STA BORDER", "$"), "STA $D020");
        assert_eq!(table.substitute("LD A,(BORDER)", "0x"), "LD A,(0xD020)");
        assert_eq!(
            table.substitute("JSR NOWHERE", "$"),
            "JSR NOWHERE",
            "unknown names stay"
        );
        assert_eq!(
            table.substitute("JSR CHROUT ; CHROUT", "$"),
            "JSR $FFD2 ; CHROUT",
            "comments stay"
        );
        assert_eq!(table.address("border"), Some(0xD020));
        // The mnemonic and registers are never symbols, even when the table has such a name.
        let odd = SymbolTable::from_entries(&[(0x1000, "NOP"), (0x2000, "A"), (0x3000, "HL")]);
        assert_eq!(odd.substitute("NOP", "$"), "NOP");
        assert_eq!(odd.substitute("LD A,HL", "0x"), "LD A,HL");
        assert_eq!(SymbolTable::new().substitute("JSR X", "$"), "JSR X");
    }
}
