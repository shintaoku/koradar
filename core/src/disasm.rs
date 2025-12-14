use crate::Address;
use anyhow::{anyhow, Result};
use capstone::prelude::*;

pub struct Disassembler {
    cs: Capstone,
}

// Capstone is not thread-safe, so we cannot implement Sync for it.
// However, TraceDB wraps everything in RwLock/DashMap, and we will only use
// Disassembler behind a Mutex or careful access control if needed.
// For TraceDB which is Send+Sync, we might need to wrap Disassembler in a Mutex.
unsafe impl Send for Disassembler {}

impl Disassembler {
    pub fn new() -> Result<Self> {
        let cs = Capstone::new()
            .x86()
            .mode(arch::x86::ArchMode::Mode64)
            .syntax(arch::x86::ArchSyntax::Intel)
            .detail(true)
            .build()
            .map_err(|e| anyhow!("Failed to initialize Capstone: {}", e))?;

        Ok(Self { cs })
    }

    pub fn disassemble(&self, bytes: &[u8], address: Address) -> Result<String> {
        let insns = self
            .cs
            .disasm_all(bytes, address)
            .map_err(|e| anyhow!("Disassembly failed: {}", e))?;

        if let Some(insn) = insns.first() {
            let mnemonic = insn.mnemonic().unwrap_or("???");
            let op_str = insn.op_str().unwrap_or("");
            Ok(format!("{} {}", mnemonic, op_str))
        } else {
            Ok(String::from("???"))
        }
    }

    pub fn get_read_registers(&self, bytes: &[u8], address: Address) -> Result<Vec<usize>> {
        let insns = self
            .cs
            .disasm_all(bytes, address)
            .map_err(|e| anyhow!("Disassembly failed: {}", e))?;

        if let Some(insn) = insns.first() {
             let details = self.cs.insn_detail(insn)
                .map_err(|e| anyhow!("Failed to get details: {}", e))?;
             let regs = details.regs_read();
             let mut read_regs = Vec::new();
             
             for r in regs {
                 if let Some(idx) = map_capstone_reg(r.0) {
                     read_regs.push(idx);
                 }
             }
             
             // Also check explicit operands for memory base/index
             let arch_detail = details.arch_detail();
             if let capstone::arch::ArchDetail::X86Detail(x86) = arch_detail {
                 for op in x86.operands() {
                     match op.op_type {
                         capstone::arch::x86::X86OperandType::Mem(m) => {
                             if let Some(idx) = map_capstone_reg(m.base().0) { read_regs.push(idx); }
                             if let Some(idx) = map_capstone_reg(m.index().0) { read_regs.push(idx); }
                         },
                         _ => {}
                     }
                 }
             }

             // Dedup
             read_regs.sort();
             read_regs.dedup();
             Ok(read_regs)
        } else {
            Ok(Vec::new())
        }
    }
}

fn map_capstone_reg(reg: u16) -> Option<usize> {
    use capstone::arch::x86::X86Reg::*;
    // Basic 64-bit mapping
    if reg == X86_REG_RAX as u16 || reg == X86_REG_EAX as u16 { return Some(0); }
    if reg == X86_REG_RBX as u16 || reg == X86_REG_EBX as u16 { return Some(1); }
    if reg == X86_REG_RCX as u16 || reg == X86_REG_ECX as u16 { return Some(2); }
    if reg == X86_REG_RDX as u16 || reg == X86_REG_EDX as u16 { return Some(3); }
    if reg == X86_REG_RSI as u16 || reg == X86_REG_ESI as u16 { return Some(4); }
    if reg == X86_REG_RDI as u16 || reg == X86_REG_EDI as u16 { return Some(5); }
    if reg == X86_REG_RBP as u16 || reg == X86_REG_EBP as u16 { return Some(6); }
    if reg == X86_REG_RSP as u16 || reg == X86_REG_ESP as u16 { return Some(7); }
    if reg == X86_REG_R8 as u16 || reg == X86_REG_R8D as u16 { return Some(8); }
    if reg == X86_REG_R9 as u16 || reg == X86_REG_R9D as u16 { return Some(9); }
    if reg == X86_REG_R10 as u16 || reg == X86_REG_R10D as u16 { return Some(10); }
    if reg == X86_REG_R11 as u16 || reg == X86_REG_R11D as u16 { return Some(11); }
    if reg == X86_REG_R12 as u16 || reg == X86_REG_R12D as u16 { return Some(12); }
    if reg == X86_REG_R13 as u16 || reg == X86_REG_R13D as u16 { return Some(13); }
    if reg == X86_REG_R14 as u16 || reg == X86_REG_R14D as u16 { return Some(14); }
    if reg == X86_REG_R15 as u16 || reg == X86_REG_R15D as u16 { return Some(15); }
    None
}
