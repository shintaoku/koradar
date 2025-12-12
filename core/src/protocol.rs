use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub enum TraceEvent {
    Init {
        vcpu_index: u32,
    },
    InsnExec {
        vcpu_index: u32,
        pc: u64,
        bytes: Vec<u8>,
        #[serde(default)]
        disasm: Option<String>,
    }, // Simplified for now
    MemAccess {
        vcpu_index: u32,
        vaddr: u64,
        is_store: bool,
        value: u64,
    }, // Placeholder
    Exit {
        vcpu_index: u32,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TraceEntry {
    pub clnum: u32,
    pub address: u64,
    pub disassembly: String,
    pub reg_diff: Option<(usize, u64)>,       // (index, value)
    pub mem_access: Option<(u64, u64, bool)>, // (addr, value, is_write)
}

// Client -> Server messages
#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type")]
pub enum ClientMessage {
    QueryState {
        clnum: u32,
    },
    GetTraceLog {
        start: u32,
        count: u32,
        #[serde(default)]
        only_user_code: bool,
    },
    StepForward {
        current: u32,
    },
    StepBackward {
        current: u32,
    },
}

// Server -> Client messages (beyond raw TraceEvent)
#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type")]
pub enum ServerMessage {
    StateUpdate {
        clnum: u32,
        registers: Vec<u64>,
        memory: Vec<u8>, // Memory dump at a specific address
        memory_addr: u64,
        disassembly: String,
    },
    TraceEvent(TraceEvent),
    TraceLog {
        entries: Vec<TraceEntry>,
    },
    MaxClnum {
        max: u32,
    },
}
