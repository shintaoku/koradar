use lazy_static::lazy_static;
use qemu_plugin_sys::*;
use serde::Serialize;
use std::collections::HashMap;
use std::io::Write;
use std::net::TcpStream;
use std::os::raw::{c_char, c_int, c_uint, c_void};
use std::sync::Mutex;

// Wrapper for pointers to make them Send+Sync
struct SyncPtr<T>(*mut T);
unsafe impl<T> Send for SyncPtr<T> {}
unsafe impl<T> Sync for SyncPtr<T> {}

impl<T> Clone for SyncPtr<T> {
    fn clone(&self) -> Self {
        SyncPtr(self.0)
    }
}

impl<T> Copy for SyncPtr<T> {}

// GLib FFI
#[repr(C)]
struct LocalGArray {
    data: *mut c_char,
    len: c_uint,
}

#[repr(C)]
struct LocalGByteArray {
    data: *mut u8,
    len: c_uint,
}

#[cfg(target_os = "linux")]
#[link(name = "glib-2.0")]
extern "C" {
    fn g_byte_array_new() -> *mut LocalGByteArray;
    fn g_byte_array_free(array: *mut LocalGByteArray, free_segment: c_int) -> *mut u8;
}

#[repr(C)]
struct qemu_plugin_reg_descriptor_local {
    handle: *mut c_void,
    name: *const c_char,
    feature: *const c_char,
}

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
        regs: Vec<u64>, // Add registers
    },
    MemAccess {
        vcpu_index: u32,
        vaddr: u64,
        is_store: bool,
        value: u64, // Not used yet, placeholder
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
    
    // Register Cache
    static ref REGS: Mutex<Vec<SyncPtr<c_void>>> = Mutex::new(Vec::new());
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
    
    // Initialize registers if not done
    let mut regs = REGS.lock().unwrap();
    if regs.is_empty() {
        unsafe {
            let reg_array_ptr = qemu_plugin_get_registers();
            if !reg_array_ptr.is_null() {
                // reg_array_ptr is *mut GArray (opaque in sys)
                let reg_array = &*(reg_array_ptr as *mut LocalGArray);
                let count = reg_array.len as usize;
                let data_ptr = reg_array.data as *mut qemu_plugin_reg_descriptor_local;
                
                println!("Koradar Tracer: Found {} registers", count);
                
                // Map of name -> handle
                let mut reg_map = HashMap::new();

                println!("Koradar Tracer: Scanning registers...");
                for i in 0..count {
                    let desc = &*data_ptr.add(i);
                    let name_c = std::ffi::CStr::from_ptr(desc.name);
                    let name = name_c.to_string_lossy().into_owned();
                    println!("Koradar Tracer: Reg[{}] = {}", i, name); // DEBUG PRINT
                    reg_map.insert(name.to_lowercase(), desc.handle);
                }

                // Target order: RAX, RBX, RCX, RDX, RSI, RDI, RBP, RSP, R8-R15
                let target_regs = [
                    "rax", "rbx", "rcx", "rdx", "rsi", "rdi", "rbp", "rsp",
                    "r8", "r9", "r10", "r11", "r12", "r13", "r14", "r15"
                ];

                for &target in target_regs.iter() {
                    if let Some(&handle) = reg_map.get(target) {
                        regs.push(SyncPtr(handle));
                    } else {
                        println!("Koradar Tracer: Warning - Register {} not found", target);
                        // Push null pointer as placeholder? 
                        // Or handle it in read loop.
                        // Let's push null and check for it.
                        regs.push(SyncPtr(std::ptr::null_mut()));
                    }
                }
            }
        }
    }
    
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
    let pc = userdata as u64;
    drop(state);

    // Capture registers (x86_64)
    let mut reg_values = Vec::new();
    let regs_handles = REGS.lock().unwrap();
    
    #[cfg(target_os = "linux")]
    unsafe {
        for &handle in regs_handles.iter() {
             if handle.0.is_null() {
                 reg_values.push(0);
                 continue;
             }
             
             let buf = g_byte_array_new();
             
             // Cast our LocalGByteArray* to sys::GByteArray* (opaque/compatible)
             // Cast handle.0 (*mut c_void) to *mut qemu_plugin_register
             qemu_plugin_read_register(handle.0 as *mut _, buf as *mut _);
             
             let arr = &*buf;
             let len = arr.len; 
             let data = arr.data;

             if len >= 8 {
                 let val_ptr = data as *const u64;
                 reg_values.push(*val_ptr);
             } else if len == 4 {
                 let val_ptr = data as *const u32;
                 reg_values.push(*val_ptr as u64);
             } else {
                 reg_values.push(0);
             }
             
             g_byte_array_free(buf, 1);
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        // Dummy values for non-Linux builds
        reg_values.resize(16, 0);
    }
    
    let regs = reg_values;

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
        regs,
    });
}

extern "C" fn vcpu_mem_access(vcpu_index: u32, info: qemu_plugin_meminfo_t, vaddr: u64, _userdata: *mut c_void) {
    let is_store = unsafe { qemu_plugin_mem_is_store(info) };
    
    send_event(TraceEvent::MemAccess {
        vcpu_index,
        vaddr,
        is_store,
        value: 0, // Value capture requires more callbacks or register inspection
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
            let mut bytes = vec![0xAAu8; size];
            
            let haddr = qemu_plugin_insn_haddr(insn);
            let captured;

            if !haddr.is_null() {
                std::ptr::copy_nonoverlapping(haddr as *const u8, bytes.as_mut_ptr(), size);
                captured = true;
            } else {
                qemu_plugin_insn_data(insn, bytes.as_mut_ptr() as *mut c_void, size);
                if bytes.iter().all(|&b| b == 0xAA) {
                    captured = false;
                } else {
                    captured = true;
                }
            }

            if !captured {
                bytes.clear();
            }
            
            if let Ok(mut cache) = INSN_CACHE.lock() {
                cache.insert(vaddr, bytes);
            }
            
            let disas_ptr = qemu_plugin_insn_disas(insn);
            if !disas_ptr.is_null() {
                let s = std::ffi::CStr::from_ptr(disas_ptr).to_string_lossy().into_owned();
                 if let Ok(mut cache) = DISASM_CACHE.lock() {
                    cache.insert(vaddr, s);
                 }
            }

            qemu_plugin_register_vcpu_insn_exec_cb(
                insn,
                Some(vcpu_insn_exec),
                qemu_plugin_cb_flags::QEMU_PLUGIN_CB_R_REGS,
                vaddr as *mut c_void,
            );
            
            qemu_plugin_register_vcpu_mem_cb(
                insn,
                Some(vcpu_mem_access),
                qemu_plugin_cb_flags::QEMU_PLUGIN_CB_R_REGS,
                qemu_plugin_mem_rw::QEMU_PLUGIN_MEM_RW,
                std::ptr::null_mut(),
            );
        }
    }
}

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