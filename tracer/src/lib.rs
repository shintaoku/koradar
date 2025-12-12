use lazy_static::lazy_static;
use qemu_plugin_sys::*;
use serde::Serialize;
use std::collections::HashMap;
use std::io::Write;
use std::net::TcpStream;
use std::os::raw::{c_char, c_int, c_void};
use std::sync::Mutex;

// Define the protocol structs locally or share via a common crate if possible.
// For simplicity in a plugin (which is a dylib), we'll define a mirroring struct
// or just use serde_json Value if we want to avoid linking complex crates,
// but linking `koradar-core` might be tricky due to `cdylib` nature.
// Let's define a simple local struct for serialization.

#[derive(Serialize)]
enum TraceEvent {
    Init {
        vcpu_index: u32,
    },
    InsnExec {
        vcpu_index: u32,
        pc: u64,
        bytes: Vec<u8>,
        disasm: Option<String>,
    },
    Exit {
        vcpu_index: u32,
    },
}

struct TracerState {
    insn_count: u64,
    stream: Option<TcpStream>,
}

lazy_static! {
    static ref STATE: Mutex<TracerState> = Mutex::new(TracerState {
        insn_count: 0,
        stream: None,
    });

    // Cache for instruction bytes: PC -> Bytes
    static ref INSN_CACHE: Mutex<HashMap<u64, Vec<u8>>> = Mutex::new(HashMap::new());
    // Cache for disassembly: PC -> String
    static ref DISASM_CACHE: Mutex<HashMap<u64, String>> = Mutex::new(HashMap::new());
}

// --- Helper to send events ---
fn send_event(event: TraceEvent) {
    let mut state = STATE.lock().unwrap();
    if state.stream.is_none() {
        // Try to connect on first send
        // Use host.docker.internal for macOS Docker, or localhost for native
        let addr = "host.docker.internal:3001";
        if let Ok(stream) = TcpStream::connect(addr) {
            state.stream = Some(stream);
            println!("Koradar Tracer: Connected to server at {}", addr);
        } else if let Ok(stream) = TcpStream::connect("127.0.0.1:3001") {
            // Fallback to localhost (e.g. Linux native)
            state.stream = Some(stream);
            println!("Koradar Tracer: Connected to server at 127.0.0.1:3001");
        } else {
            // Failed to connect
            return;
        }
    }

    if let Some(stream) = &mut state.stream {
        if let Ok(json) = serde_json::to_string(&event) {
            let _ = stream.write_all(json.as_bytes());
            let _ = stream.write_all(b"\n"); // NDJSON
        }
    }
}

// --- Callbacks ---

extern "C" fn vcpu_init(_id: qemu_plugin_id_t, vcpu_index: u32) {
    println!("Koradar Tracer: vCPU {} initialized", vcpu_index);
    send_event(TraceEvent::Init { vcpu_index });
}

extern "C" fn vcpu_exit(_id: qemu_plugin_id_t, vcpu_index: u32) {
    println!("Koradar Tracer: vCPU {} exited", vcpu_index);
    send_event(TraceEvent::Exit { vcpu_index });
}

extern "C" fn plugin_exit(_id: qemu_plugin_id_t, _data: *mut c_void) {
    let count = STATE.lock().unwrap().insn_count;
    println!("Koradar Tracer: Exiting. Total instructions: {}", count);
}

extern "C" fn vcpu_insn_exec(vcpu_index: u32, userdata: *mut c_void) {
    let mut state = STATE.lock().unwrap();
    state.insn_count += 1;
    // For performance, we shouldn't lock and send JSON every instruction in a real scenario.
    // We should buffer. But for Phase 1.5 proof-of-concept, we'll do it.

    // userdata is the PC (passed as pointer)
    let pc = userdata as u64;

    // Drop lock before sending to avoid contention if send blocks (though unix stream buffer helps)
    drop(state);

    // Retrieve bytes from cache
    let (bytes, disasm) = if let Ok(cache) = INSN_CACHE.lock() {
        let b = cache.get(&pc).cloned().unwrap_or_default();
        let d = if let Ok(d_cache) = DISASM_CACHE.lock() {
            d_cache.get(&pc).cloned()
        } else {
            None
        };
        (b, d)
    } else {
        (Vec::new(), None)
    };

    send_event(TraceEvent::InsnExec {
        vcpu_index,
        pc,
        bytes,
        disasm,
    });
}

extern "C" fn vcpu_tb_trans(_id: qemu_plugin_id_t, tb: *mut qemu_plugin_tb) {
    unsafe {
        let n = qemu_plugin_tb_n_insns(tb);
        for i in 0..n {
            let insn = qemu_plugin_tb_get_insn(tb, i);
            let vaddr = qemu_plugin_insn_vaddr(insn);

            // Extract bytes
            let size = qemu_plugin_insn_size(insn);
            // Initialize with a pattern to detect if write failed
            let mut bytes = vec![0xAAu8; size];
            
            // Try haddr first (more reliable for reading bytes)
            let haddr = qemu_plugin_insn_haddr(insn);
            let captured;

            if !haddr.is_null() {
                std::ptr::copy_nonoverlapping(haddr as *const u8, bytes.as_mut_ptr(), size);
                captured = true;
            } else {
                // Fallback
                // We assume qemu_plugin_insn_data writes to the buffer.
                // If it doesn't, bytes will remain 0xAA.
                // Note: qemu_plugin_insn_data return value is actually usize (bytes copied) in some versions,
                // but header signature might vary. We'll rely on pattern check.
                qemu_plugin_insn_data(insn, bytes.as_mut_ptr() as *mut c_void, size);
                // Check if unchanged
                if bytes.iter().all(|&b| b == 0xAA) {
                    captured = false;
                } else {
                    captured = true;
                }
            }

            if !captured {
                // If capture failed, send empty bytes so server falls back to QEMU disasm
                bytes.clear();
            }
            
            // Store in cache
            if let Ok(mut cache) = INSN_CACHE.lock() {
                cache.insert(vaddr, bytes);
            }
            
            // Get disassembly
            let disas_ptr = qemu_plugin_insn_disas(insn);
            if !disas_ptr.is_null() {
                let s = std::ffi::CStr::from_ptr(disas_ptr).to_string_lossy().into_owned();
                 if let Ok(mut cache) = DISASM_CACHE.lock() {
                    cache.insert(vaddr, s);
                 }
            }

            // Pass PC as userdata
            qemu_plugin_register_vcpu_insn_exec_cb(
                insn,
                Some(vcpu_insn_exec),
                qemu_plugin_cb_flags::QEMU_PLUGIN_CB_NO_REGS,
                vaddr as *mut c_void,
            );
        }
    }
}

// --- Entry Point ---

#[no_mangle]
#[used]
pub static qemu_plugin_version: c_int = 2;

#[no_mangle]
pub extern "C" fn qemu_plugin_install(
    id: qemu_plugin_id_t,
    _info: *const qemu_info_t,
    _argc: c_int,
    _argv: *const *const c_char,
) -> c_int {
    println!("Koradar Tracer: Install");

    unsafe {
        qemu_plugin_register_vcpu_init_cb(id, Some(vcpu_init));
        qemu_plugin_register_vcpu_exit_cb(id, Some(vcpu_exit));
        qemu_plugin_register_atexit_cb(id, Some(plugin_exit), std::ptr::null_mut());
        qemu_plugin_register_vcpu_tb_trans_cb(id, Some(vcpu_tb_trans));
    }

    0
}
