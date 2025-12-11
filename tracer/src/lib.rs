use qemu_plugin::sys::*;
use serde::Serialize;
use std::io::Write;
use std::os::raw::{c_char, c_int, c_void};
use std::os::unix::net::UnixStream;
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
    },
    Exit {
        vcpu_index: u32,
    },
}

struct TracerState {
    insn_count: u64,
    stream: Option<UnixStream>,
}

static STATE: Mutex<TracerState> = Mutex::new(TracerState {
    insn_count: 0,
    stream: None,
});

// --- Helper to send events ---
fn send_event(event: TraceEvent) {
    let mut state = STATE.lock().unwrap();
    if state.stream.is_none() {
        // Try to connect on first send
        if let Ok(stream) = UnixStream::connect("/tmp/koradar.sock") {
            state.stream = Some(stream);
            println!("Koradar Tracer: Connected to server");
        } else {
            // Failed to connect, maybe server not running. Ignore or retry later.
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

    send_event(TraceEvent::InsnExec {
        vcpu_index,
        pc,
        bytes: Vec::new(),
    }); // bytes not yet available
}

extern "C" fn vcpu_tb_trans(_id: qemu_plugin_id_t, tb: *mut qemu_plugin_tb) {
    unsafe {
        let n = qemu_plugin_tb_n_insns(tb);
        for i in 0..n {
            let insn = qemu_plugin_tb_get_insn(tb, i);
            let vaddr = qemu_plugin_insn_vaddr(insn);

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
