use crate::disasm::Disassembler;
use crate::protocol::TraceEntry;
use dashmap::DashMap;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub type Address = u64;
pub type Clnum = u32; // Change Line Number (Logical Time)
pub type Data = u64;

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub struct ChangeFlags: u32 {
        const IS_VALID   = 0x80000000;
        const IS_WRITE   = 0x40000000;
        const IS_MEM     = 0x20000000;
        const IS_START   = 0x10000000;
        const IS_SYSCALL = 0x08000000;
        const SIZE_MASK  = 0x000000FF;
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[repr(C)]
pub struct Change {
    pub address: Address,
    pub data: Data,
    pub clnum: Clnum,
    pub flags: u32,
}

#[derive(Debug, Default)]
struct MemoryCell {
    // Initial static value (from binary loader)
    static_value: Option<u8>,
    // Dynamic history
    history: Vec<(Clnum, u8)>,
}

impl MemoryCell {
    fn get_value_at(&self, clnum: Clnum) -> Option<u8> {
        let idx = self.history.partition_point(|&(c, _)| c <= clnum);
        if idx == 0 {
            self.static_value
        } else {
            Some(self.history[idx - 1].1)
        }
    }
}

pub struct TraceDB {
    changes: RwLock<Vec<Change>>,
    memory: DashMap<Address, MemoryCell>,
    registers: RwLock<Vec<Vec<(Clnum, u64)>>>,
    // Reverse index: (Address, AccessType ('R'|'W')) -> List of Clnums
    access_index: DashMap<(Address, u8), Vec<Clnum>>,
    // Disassembler instance
    disassembler: Mutex<Disassembler>,
    // Instruction cache: (Address, Instruction Bytes) -> Disassembled String
    insn_cache: DashMap<(Address, Vec<u8>), String>,
    // Map from Clnum to instruction bytes
    instructions: DashMap<Clnum, Vec<u8>>,
    // Map from Clnum to disassembly string (fallback if bytes unavailable or disasm failed)
    instructions_disasm: DashMap<Clnum, String>,
    // User code ranges (start, end) inclusive
    user_code_ranges: RwLock<Vec<(u64, u64)>>,
    // Entry point of the binary (static address)
    entry_point: RwLock<Option<u64>>,
    // Execution bias (RunAddr - StaticAddr)
    bias: RwLock<i64>,
}

impl TraceDB {
    pub fn new(reg_count: usize) -> Self {
        let mut regs = Vec::with_capacity(reg_count);
        for _ in 0..reg_count {
            regs.push(Vec::new());
        }

        Self {
            changes: RwLock::new(Vec::new()),
            memory: DashMap::new(),
            registers: RwLock::new(regs),
            access_index: DashMap::new(),
            disassembler: Mutex::new(Disassembler::new().expect("Failed to init disassembler")),
            insn_cache: DashMap::new(),
            instructions: DashMap::new(),
            instructions_disasm: DashMap::new(),
            user_code_ranges: RwLock::new(Vec::new()),
            entry_point: RwLock::new(None),
            bias: RwLock::new(0),
        }
    }

    pub fn set_entry_point(&self, ep: u64) {
        *self.entry_point.write() = Some(ep);
        println!("[DEBUG] TraceDB: Entry Point set to {:x}", ep);
    }

    pub fn set_bias(&self, bias: i64) {
        *self.bias.write() = bias;
        println!("[DEBUG] TraceDB: Bias set to {:x} (RunAddr - StaticAddr)", bias);
    }

    pub fn get_bias(&self) -> i64 {
        *self.bias.read()
    }

    pub fn get_entry_point(&self) -> Option<u64> {
        *self.entry_point.read()
    }

    pub fn load_static_memory(&self, start_addr: Address, data: &[u8]) {
        for (i, &byte) in data.iter().enumerate() {
            let addr = start_addr + i as u64;
            self.memory.entry(addr).or_default().static_value = Some(byte);
        }
    }

    pub fn register_code_range(&self, start: u64, size: u64) {
        println!(
            "[DEBUG] register_code_range: {:x} - {:x}",
            start,
            start + size
        );
        let mut ranges = self.user_code_ranges.write();
        ranges.push((start, start + size));
    }

    pub fn is_user_code(&self, address: u64) -> bool {
        let ranges = self.user_code_ranges.read();
        // If no ranges registered, treat everything as user code
        if ranges.is_empty() {
            return true;
        }
        
        // Normalize address by removing bias
        // StaticAddr = RunAddr - Bias
        let bias = *self.bias.read();
        // Handle negative result safely (though address should be > bias if bias is positive)
        let static_addr = (address as i128 - bias as i128) as u64;

        ranges
            .iter()
            .any(|&(start, end)| static_addr >= start && static_addr < end)
    }

    pub fn add_instruction(&self, clnum: Clnum, bytes: Vec<u8>) {
        if !bytes.is_empty() {
            self.instructions.insert(clnum, bytes);
        }
    }

    pub fn add_instruction_disasm(&self, clnum: Clnum, disasm: String) {
        if !disasm.is_empty() {
            self.instructions_disasm.insert(clnum, disasm);
        }
    }

    pub fn add_change(&self, change: Change) {
        // 1. Add to raw log
        {
            let mut w = self.changes.write();
            w.push(change);
        }

        let flags = ChangeFlags::from_bits_truncate(change.flags);

        // 2. Update Indices
        if flags.contains(ChangeFlags::IS_MEM) {
            // Memory Access
            if flags.contains(ChangeFlags::IS_WRITE) {
                let size = (change.flags & ChangeFlags::SIZE_MASK.bits()) as u64 / 8;
                let mut data = change.data;
                for i in 0..size {
                    let addr = change.address + i;
                    let byte = (data & 0xFF) as u8;
                    data >>= 8;

                    self.memory
                        .entry(addr)
                        .or_default()
                        .history
                        .push((change.clnum, byte));
                }
            }
        } else if flags.contains(ChangeFlags::IS_WRITE) {
            // Register Write
            let reg_idx = (change.address / 8) as usize;
            let mut regs = self.registers.write();
            if reg_idx < regs.len() {
                regs[reg_idx].push((change.clnum, change.data));
            }
        }

        // 3. Update Reverse Index
        let type_char = if flags.contains(ChangeFlags::IS_WRITE) {
            b'W'
        } else {
            b'R'
        };
        self.access_index
            .entry((change.address, type_char))
            .or_default()
            .push(change.clnum);
    }

    pub fn get_memory_at(&self, clnum: Clnum, addr: Address, size: usize) -> Vec<u8> {
        let mut result = Vec::with_capacity(size);
        for i in 0..size {
            let a = addr + i as u64;
            let val = self
                .memory
                .get(&a)
                .and_then(|cell| cell.get_value_at(clnum))
                .unwrap_or(0);
            result.push(val);
        }
        result
    }

    pub fn get_registers_at(&self, clnum: Clnum) -> Vec<u64> {
        let regs = self.registers.read();
        regs.iter()
            .map(|history| {
                let idx = history.partition_point(|&(c, _)| c <= clnum);
                if idx == 0 {
                    0
                } else {
                    history[idx - 1].1
                }
            })
            .collect()
    }

    pub fn disassemble(&self, address: Address, bytes: &[u8]) -> String {
        if bytes.is_empty() {
            return String::from("...");
        }

        let key = (address, bytes.to_vec());
        if let Some(s) = self.insn_cache.get(&key) {
            return s.clone();
        }

        let disasm = self
            .disassembler
            .lock()
            .disassemble(bytes, address)
            .unwrap_or_else(|_| "invalid".to_string());
        self.insn_cache.insert(key, disasm.clone());
        disasm
    }

    pub fn get_disassembly_at(&self, clnum: Clnum) -> String {
        // Find the PC at this clnum
        // PC change is recorded as a Change with IS_START flag
        let changes = self.changes.read();

        // Find the instruction change for this clnum (or the one before it)
        // Optimization: binary search could be used here if changes are sorted
        let pc_change = changes.iter().rev().find(|c| {
            c.clnum <= clnum
                && ChangeFlags::from_bits_truncate(c.flags).contains(ChangeFlags::IS_START)
        });

        if let Some(change) = pc_change {
            // Get bytes for this clnum
            let mut _bytes_ok = false;
            if let Some(bytes) = self.instructions.get(&change.clnum) {
                if !bytes.is_empty() && !bytes.iter().all(|&b| b == 0) {
                    return self.disassemble(change.address, &bytes);
                }
                // If bytes are empty or all zeros, fall through
            }
            
            // Check for pre-calculated disassembly (from QEMU/Tracer)
            if let Some(disasm) = self.instructions_disasm.get(&change.clnum) {
                 return disasm.clone();
            }

            // Fallback: Read from memory (static code)
            let bytes = self.get_memory_at(change.clnum, change.address, 16);
            if !bytes.iter().all(|&b| b == 0) {
                return self.disassemble(change.address, &bytes);
            }
        }

        String::from("???")
    }

    pub fn get_trace_log(&self, start: Clnum, count: u32, only_user_code: bool) -> Vec<TraceEntry> {
        // println!(
        //     "[DEBUG] get_trace_log start={} count={} only_user_code={}",
        //     start, count, only_user_code
        // );
        
        // Debug: Dump user code ranges if only_user_code is true
        // if only_user_code {
        //     let ranges = self.user_code_ranges.read();
        //     println!("[DEBUG] Current user_code_ranges: {:?}", *ranges);
        // }

        let changes = self.changes.read();
        let mut entries = Vec::new();

        let mut c = start;
        let mut collected = 0;
        let max_clnum = changes.last().map(|c| c.clnum).unwrap_or(0);
        // println!("[DEBUG] max_clnum={}", max_clnum);

        // Safety break
        while collected < count && c <= max_clnum {
            // Find the IS_START change for this clnum
            let start_change = changes.iter().find(|ch| {
                ch.clnum == c
                    && ChangeFlags::from_bits_truncate(ch.flags).contains(ChangeFlags::IS_START)
            });

            if let Some(change) = start_change {
                if !only_user_code || self.is_user_code(change.address) {
                    let disassembly = {
                        let mut d = String::new();
                        let mut done = false;
                        
                        // 1. Try bytes if valid (non-zero)
                        if let Some(bytes) = self.instructions.get(&c) {
                            if !bytes.is_empty() && !bytes.iter().all(|&b| b == 0) {
                                d = self.disassemble(change.address, &bytes);
                                done = true;
                            }
                        }
                        
                        // 2. Try QEMU disasm
                        if !done {
                            if let Some(qs) = self.instructions_disasm.get(&c) {
                                d = qs.clone();
                                done = true;
                            }
                        }
                        
                        // 3. Fallback to memory (using static address)
                        if !done {
                            let bias = *self.bias.read();
                            let static_addr = (change.address as i128 - bias as i128) as u64;
                            let bytes = self.get_memory_at(c, static_addr, 16);
                            d = self.disassemble(change.address, &bytes);
                        }
                        d
                    };

                    // Find register/memory effects
                    // Just take the first one for now
                    let reg_diff = changes
                        .iter()
                        .find(|ch| {
                            ch.clnum == c
                                && !ChangeFlags::from_bits_truncate(ch.flags)
                                    .contains(ChangeFlags::IS_MEM)
                                && !ChangeFlags::from_bits_truncate(ch.flags)
                                    .contains(ChangeFlags::IS_START)
                                && ChangeFlags::from_bits_truncate(ch.flags)
                                    .contains(ChangeFlags::IS_WRITE)
                        })
                        .map(|ch| ((ch.address / 8) as usize, ch.data));

                    let mem_access = changes
                        .iter()
                        .find(|ch| {
                            ch.clnum == c
                                && ChangeFlags::from_bits_truncate(ch.flags)
                                    .contains(ChangeFlags::IS_MEM)
                        })
                        .map(|ch| {
                            (
                                ch.address,
                                ch.data,
                                ChangeFlags::from_bits_truncate(ch.flags)
                                    .contains(ChangeFlags::IS_WRITE),
                            )
                        });

                    entries.push(TraceEntry {
                        clnum: c,
                        address: change.address,
                        disassembly,
                        reg_diff,
                        mem_access,
                    });
                    collected += 1;
                }
            } else {
                // Too noisy to log every missing clnum, but useful for debugging holes
                // if c % 1000 == 0 {
                //     println!("[DEBUG] No IS_START change found for clnum {}", c);
                // }
            }
            c += 1;

            if c > start + 100000 && collected == 0 {
                // println!("[DEBUG] Break due to scan limit. collected={}", collected);
                break; // Prevent infinite loop if no user code found
            }
        }
        // println!("[DEBUG] returning {} entries", entries.len());
        entries
    }
}
