//! Disassembly of x86 and x86-64 code: the functions whose addresses the
//! strings and symbols point to, decoded to Intel-syntax instructions, with
//! the call targets and branches that outline the control flow.

use iced_x86::{Decoder, DecoderOptions, Formatter, Instruction, IntelFormatter};
use object::{Object, ObjectSection, ObjectSymbol};
use std::collections::BTreeMap;

/// One decoded instruction: its address and its text.
pub struct Line {
    pub ip: u64,
    pub text: String,
}

/// Disassembles `bytes` (loaded at `rip`), at most `max` instructions.
/// Stops at a return that is not followed by more code.
pub fn disassemble(bytes: &[u8], rip: u64, bitness: u32, max: usize) -> Vec<Line> {
    let mut decoder = Decoder::with_ip(bitness, bytes, rip, DecoderOptions::NONE);
    let mut formatter = IntelFormatter::new();
    formatter.options_mut().set_first_operand_char_index(8);
    let mut out = Vec::new();
    let mut insn = Instruction::default();
    let mut text = String::new();
    while decoder.can_decode() && out.len() < max {
        decoder.decode_out(&mut insn);
        if insn.is_invalid() {
            break;
        }
        text.clear();
        formatter.format(&insn, &mut text);
        let is_ret = text.starts_with("ret");
        out.push(Line { ip: insn.ip(), text: text.clone() });
        if is_ret {
            break;
        }
    }
    out
}

/// Which bitness and the section covering `addr`, from the file.
struct Loaded<'a> {
    bitness: u32,
    /// `(section data, its virtual address)`.
    sections: Vec<(&'a [u8], u64)>,
}

impl Loaded<'_> {
    /// The bytes at virtual address `addr`.
    fn at(&self, addr: u64) -> Option<&[u8]> {
        for (data, base) in &self.sections {
            if addr >= *base && (addr - base) < data.len() as u64 {
                return Some(&data[(addr - base) as usize..]);
            }
        }
        None
    }
}

fn load<'a>(file: &'a object::File<'a>) -> Loaded<'a> {
    let bitness = if file.is_64() { 64 } else { 32 };
    let sections = file
        .sections()
        .filter(|s| s.kind() == object::SectionKind::Text || matches!(s.name(), Ok(".text" | "__text")))
        .filter_map(|s| Some((s.data().ok()?, s.address())))
        .collect();
    Loaded { bitness, sections }
}

/// Function addresses named like the question, from the symbol table.
fn targets(file: &object::File, terms: &[String]) -> BTreeMap<u64, String> {
    let mut out = BTreeMap::new();
    for sym in file.symbols() {
        if !sym.is_definition() {
            continue;
        }
        let Ok(name) = sym.name() else { continue };
        let lower = name.to_lowercase();
        if sym.address() != 0 && (terms.is_empty() || terms.iter().any(|t| lower.contains(t))) {
            out.insert(sym.address(), name.to_string());
            if out.len() >= 6 {
                break;
            }
        }
    }
    out
}

/// Disassembles the functions related to `question`. Empty when there are
/// no usable symbols (a stripped binary) or the architecture is not x86.
pub fn functions(file: &object::File, question: &str) -> String {
    if !matches!(file.architecture(), object::Architecture::X86_64 | object::Architecture::I386) {
        return "Desmontagem: arquitetura não é x86; use o REA externo (Ghidra/IDA).".to_string();
    }
    let loaded = load(file);
    if loaded.sections.is_empty() {
        return String::new();
    }
    let terms = crate::terms(question);
    let targets = targets(file, &terms);
    if targets.is_empty() {
        return "Desmontagem: binário sem símbolos para as palavras da pergunta (provavelmente stripped).".to_string();
    }
    let mut r = String::from("Disassembly of functions matching the question (Intel syntax):\n");
    for (addr, name) in targets.iter().take(4) {
        let Some(bytes) = loaded.at(*addr) else { continue };
        r.push_str(&format!("\n{name} @ 0x{addr:x}:\n"));
        let mut calls = Vec::new();
        for line in disassemble(bytes, *addr, loaded.bitness, 80) {
            r.push_str(&format!("  0x{:x}: {}\n", line.ip, line.text));
            if line.text.starts_with("call") || line.text.starts_with("jmp") {
                calls.push(line.text);
            }
        }
        if !calls.is_empty() {
            r.push_str(&format!("  → chamadas/saltos: {}\n", calls.iter().take(12).cloned().collect::<Vec<_>>().join("; ")));
        }
    }
    r
}
