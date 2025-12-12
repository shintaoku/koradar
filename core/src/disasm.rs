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
}
