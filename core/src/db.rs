use std::collections::BTreeMap;
use parking_lot::RwLock;
use serde::{Serialize, Deserialize};
use dashmap::DashMap;

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
        }
    }

    pub fn load_static_memory(&self, start_addr: Address, data: &[u8]) {
        for (i, &byte) in data.iter().enumerate() {
            let addr = start_addr + i as u64;
            self.memory.entry(addr)
                .or_default()
                .static_value = Some(byte);
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
                    
                    self.memory.entry(addr)
                        .or_default()
                        .history.push((change.clnum, byte));
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
        let type_char = if flags.contains(ChangeFlags::IS_WRITE) { b'W' } else { b'R' }; 
        self.access_index.entry((change.address, type_char))
            .or_default()
            .push(change.clnum);
    }

    pub fn get_memory_at(&self, clnum: Clnum, addr: Address, size: usize) -> Vec<u8> {
        let mut result = Vec::with_capacity(size);
        for i in 0..size {
            let a = addr + i as u64;
            let val = self.memory.get(&a)
                .and_then(|cell| cell.get_value_at(clnum))
                .unwrap_or(0); 
            result.push(val);
        }
        result
    }

    pub fn get_registers_at(&self, clnum: Clnum) -> Vec<u64> {
        let regs = self.registers.read();
        regs.iter().map(|history| {
            let idx = history.partition_point(|&(c, _)| c <= clnum);
            if idx == 0 { 0 } else { history[idx - 1].1 }
        }).collect()
    }
}
