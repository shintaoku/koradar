use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
    routing::get,
    Router,
};
use koradar_core::{
    protocol::{ClientMessage, ServerMessage, TraceEvent},
    BinaryLoader, Change, ChangeFlags, TraceDB,
};
use serde_json;
use std::env;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::broadcast;
use tower_http::services::ServeDir;

mod ai;

struct AppState {
    db: Arc<TraceDB>,
    tx: broadcast::Sender<String>,
    max_clnum: Arc<std::sync::atomic::AtomicU32>,
}

#[tokio::main]
async fn main() {
    // Load .env
    dotenv::dotenv().ok();

    println!("Koradar Server Starting...");

    let db = Arc::new(TraceDB::new(16));

    // Load binary if provided
    let args: Vec<String> = env::args().collect();
    if args.len() > 1 {
        let binary_path = &args[1];
        println!("Loading binary: {}", binary_path);
        match BinaryLoader::load_file(&db, Path::new(binary_path)) {
            Ok(_) => {
                println!("Binary loaded successfully");
            }
            Err(e) => eprintln!("Failed to load binary: {}", e),
        }
    }

    let (tx, _rx) = broadcast::channel(100);
    let max_clnum = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let state = Arc::new(AppState {
        db: db.clone(),
        tx: tx.clone(),
        max_clnum: max_clnum.clone(),
    });

    // Start IPC Listener
    let ipc_tx = tx.clone();
    let ipc_db = db.clone();
    let ipc_max_clnum = max_clnum.clone();

    tokio::spawn(async move {
        let listener = match tokio::net::TcpListener::bind("0.0.0.0:3001").await {
            Ok(l) => {
                println!("IPC Listener listening on 0.0.0.0:3001");
                l
            }
            Err(e) => {
                panic!("Failed to bind IPC TCP socket: {}", e);
            }
        };

        loop {
            if let Ok((stream, _addr)) = listener.accept().await {
                let ipc_tx = ipc_tx.clone();
                let ipc_db = ipc_db.clone();
                let ipc_max_clnum = ipc_max_clnum.clone();

                tokio::spawn(async move {
                    let mut reader = BufReader::new(stream);
                    let mut line = String::new();
                    let mut current_clnum = 0;

                    while let Ok(bytes_read) = reader.read_line(&mut line).await {
                        if bytes_read == 0 {
                            break;
                        }

                        // Parse JSON
                        if let Ok(event) = serde_json::from_str::<TraceEvent>(&line) {
                            current_clnum += 1;
                            ipc_max_clnum.store(current_clnum, Ordering::Relaxed);

                            // Apply to DB
                            match &event {
                                    TraceEvent::InsnExec {
                                        vcpu_index: _,
                                        pc,
                                        bytes,
                                        disasm,
                                        regs,
                                    } => {
                                        // Debug log for instruction addresses
                                        // #region agent log
                                        if current_clnum < 5 {
                                            use std::fs::OpenOptions;
                                            use std::io::Write;
                                            let path = "/Users/shinta/git/github.com/geohot/qira/.cursor/debug.log";
                                            if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
                                                let _ = writeln!(file, "{{\"id\":\"log_insn_regs\",\"timestamp\":{},\"location\":\"server/main.rs:InsnExec\",\"message\":\"Received registers\",\"data\":{{\"clnum\":{}, \"pc\":{}, \"regs_len\":{}, \"regs_sample\":{:?}}},\"sessionId\":\"debug-session\",\"runId\":\"debug-run\",\"hypothesisId\":\"regs-zero\"}}", 
                                                    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis(),
                                                    current_clnum, pc, regs.len(), regs.iter().take(4).collect::<Vec<_>>()
                                                );
                                            }
                                        }
                                        // #endregion
                                        /*
                                        if current_clnum < 200 {
                                            println!("[DEBUG] Insn: {:x}", pc);
                                        }
                                        */

                                        // Better heuristic: scan all instructions, not just first 10
                                        if let Some(ep) = ipc_db.get_entry_point() {
                                            // Check if this PC matches the entry point pattern
                                            // If -no-pie, pc should equal ep.
                                            // If PIE, pc = ep + bias.
                                            // Since we don't know bias, we check alignment.
                                            // 0x...1234 (pc) vs 0x...1234 (ep)
                                            if (pc & 0xFFF) == (ep & 0xFFF) {
                                                let bias = (*pc as i64) - (ep as i64);
                                                // Only set if we haven't found a bias or it's different/better
                                                // (e.g. bias=0 is preferred if -no-pie)
                                                let current_bias = ipc_db.get_bias();
                                                if current_bias == 0 && bias != 0 {
                                                     // If we thought bias was 0 but found a PIE match, maybe update?
                                                     // But if -no-pie, bias IS 0.
                                                     // Let's print for debug.
                                                     println!("[INFO] Candidate bias: {:x} at clnum {}", bias, current_clnum);
                                                     // If bias is huge (like 0x7fff...) it might be loader matching coincidently?
                                                     // Loader addresses are usually high. Main binary usually 0x55...
                                                     // If pc is 0x40xxxx, it's definitely main binary.
                                                     
                                                     // Prioritize low-memory addresses for main binary if possible
                                                     if *pc < 0x7000_0000_0000 {
                                                         ipc_db.set_bias(bias);
                                                     }
                                                } else if bias == 0 && current_bias != 0 {
                                                    // Found exact match, prefer this!
                                                    ipc_db.set_bias(0);
                                                }
                                            }
                                        }
                                        ipc_db.add_instruction(current_clnum, bytes.clone());
                                        if let Some(d) = disasm {
                                            ipc_db.add_instruction_disasm(current_clnum, d.clone());
                                        }
                                        
                                        if !regs.is_empty() {
                                            ipc_db.update_registers(current_clnum, regs);
                                        }

                                        ipc_db.add_change(Change {
                                        address: *pc,
                                        data: 0,
                                        clnum: current_clnum,
                                        flags: ChangeFlags::IS_VALID.bits()
                                            | ChangeFlags::IS_START.bits(),
                                    });
                                }
                                TraceEvent::Init { .. } => {}
                                TraceEvent::Exit { .. } => {}
                                _ => {}
                            }

                            // Broadcast as ServerMessage::TraceEvent
                            let server_msg = ServerMessage::TraceEvent(event);
                            if let Ok(json_str) = serde_json::to_string(&server_msg) {
                                let _ = ipc_tx.send(json_str);
                            }

                            // Broadcast MaxClnum
                            let max_msg = ServerMessage::MaxClnum { max: current_clnum };
                            if let Ok(json_str) = serde_json::to_string(&max_msg) {
                                let _ = ipc_tx.send(json_str);
                            }
                        }
                        line.clear();
                    }
                });
            }
        }
    });

    let app = Router::new()
        .route("/ws", get(ws_handler))
        .nest_service("/", ServeDir::new("frontend/dist"))
        .with_state(state.clone());

    let listener = match tokio::net::TcpListener::bind("0.0.0.0:3000").await {
        Ok(l) => l,
        Err(e) => {
            panic!("Failed to bind TCP listener: {}", e);
        }
    };
    println!("Listening on http://localhost:3000");

    axum::serve(listener, app).await.unwrap();
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, state: Arc<AppState>) {
    let mut rx = state.tx.subscribe();
    let db = state.db.clone();
    let max_clnum = state.max_clnum.clone();

    // Send initial max_clnum
    let max = max_clnum.load(Ordering::Relaxed);
    if let Ok(json) = serde_json::to_string(&ServerMessage::MaxClnum { max }) {
        let _ = socket.send(Message::Text(json)).await;
    }

    loop {
        tokio::select! {
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<ClientMessage>(&text) {
                            Ok(client_msg) => {
                                match client_msg {
                                    ClientMessage::QueryState { clnum, memory_addr } => {
                                        let regs = db.get_registers_at(clnum);
                                        // Default to 0 or use provided address
                                        let mem_start = memory_addr.unwrap_or(0);
                                        let mem = db.get_memory_at(clnum, mem_start, 256);
                                        let disasm = db.get_disassembly_at(clnum);
                                        let response = ServerMessage::StateUpdate {
                                            clnum,
                                            registers: regs,
                                            memory: mem,
                                            memory_addr: mem_start,
                                            disassembly: disasm,
                                        };
                                        if let Ok(json) = serde_json::to_string(&response) {
                                            let _ = socket.send(Message::Text(json)).await;
                                        }
                                    }
                                    ClientMessage::GetTraceLog { start, count, only_user_code } => {
                                        let entries = db.get_trace_log(start, count, only_user_code);
                                        // #region agent log
                                        {
                                            use std::fs::OpenOptions;
                                            use std::io::Write;
                                            let path = "/Users/shinta/git/github.com/geohot/qira/.cursor/debug.log";
                                            if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
                                                let _ = writeln!(file, "{{\"id\":\"log_get_trace\",\"timestamp\":{},\"location\":\"server/main.rs:GetTraceLog\",\"message\":\"GetTraceLog query\",\"data\":{{\"start\":{}, \"count\":{}, \"only_user_code\":{}, \"result_count\":{}}},\"sessionId\":\"debug-session\",\"runId\":\"debug-run\",\"hypothesisId\":\"user-code-filter\"}}", 
                                                    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis(),
                                                    start, count, only_user_code, entries.len()
                                                );
                                            }
                                        }
                                        // #endregion
                                        let response = ServerMessage::TraceLog { entries };
                                        if let Ok(json) = serde_json::to_string(&response) {
                                            let _ = socket.send(Message::Text(json)).await;
                                        }
                                    }
                                    ClientMessage::StepForward { current } => {
                                        let next_clnum = (current + 1).min(max_clnum.load(Ordering::Relaxed));
                                        let regs = db.get_registers_at(next_clnum);
                                        let mem = db.get_memory_at(next_clnum, 0, 256);
                                        let response = ServerMessage::StateUpdate {
                                            clnum: next_clnum,
                                            registers: regs,
                                            memory: mem,
                                            memory_addr: 0,
                                            disassembly: db.get_disassembly_at(next_clnum),
                                        };
                                        if let Ok(json) = serde_json::to_string(&response) {
                                            let _ = socket.send(Message::Text(json)).await;
                                        }
                                    }
                                    ClientMessage::StepBackward { current } => {
                                        let prev_clnum = current.saturating_sub(1).max(1);
                                        let regs = db.get_registers_at(prev_clnum);
                                        let mem = db.get_memory_at(prev_clnum, 0, 256);
                                        let response = ServerMessage::StateUpdate {
                                            clnum: prev_clnum,
                                            registers: regs,
                                            memory: mem,
                                            memory_addr: 0,
                                            disassembly: db.get_disassembly_at(prev_clnum),
                                        };
                                        if let Ok(json) = serde_json::to_string(&response) {
                                            let _ = socket.send(Message::Text(json)).await;
                                        }
                                    }
                                    ClientMessage::GetCFG { only_user_code, start_from_main } => {
                                        let cfg = db.analyze_cfg(only_user_code, start_from_main);
                                        let mermaid = cfg.to_mermaid();
                                        println!("[INFO] Generated CFG size: {} bytes", mermaid.len());
                                        
                                        // #region agent log
                                        {
                                            use std::fs::OpenOptions;
                                            use std::io::Write;
                                            let path = "/Users/shinta/git/github.com/geohot/qira/.cursor/debug.log";
                                            if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
                                                let _ = writeln!(file, "{{\"id\":\"log_cfg_gen\",\"timestamp\":{},\"location\":\"server/main.rs:GetCFG\",\"message\":\"Generated CFG\",\"data\":{{\"size\":{}, \"head\":\"{}\"}},\"sessionId\":\"debug-session\",\"runId\":\"debug-run\",\"hypothesisId\":\"mermaid-syntax\"}}", 
                                                    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis(),
                                                    mermaid.len(),
                                                    mermaid.chars().take(500).collect::<String>().replace("\"", "'").replace("\n", "\\n")
                                                );
                                            }
                                        }
                                        // #endregion

                                        let response = ServerMessage::CFG { graph: mermaid };
                                        if let Ok(json) = serde_json::to_string(&response) {
                                            let _ = socket.send(Message::Text(json)).await;
                                        }
                                    }
                                    ClientMessage::AskAI { clnum } => {
                                        // Build context
                                        let disasm = db.get_disassembly_at(clnum);
                                        let regs = db.get_registers_at(clnum);
                                        // Simple register dump
                                        let regs_str = regs.iter().enumerate().map(|(i, v)| format!("R{}: {:x}", i, v)).collect::<Vec<_>>().join(", ");
                                        
                                        // Get surrounding code (5 before, 5 after)
                                        // We need addresses... just get 10 disassembly lines
                                        // This is a bit inefficient without `get_trace_log` helper but acceptable
                                        let log = db.get_trace_log(clnum.saturating_sub(5), 10, true);
                                        let code_context = log.iter().map(|e| format!("{:x}: {}", e.address, e.disassembly)).collect::<Vec<_>>().join("\n");

                                        let context_str = format!("Instruction: {}\nRegisters: {}\n\nSurrounding Code:\n{}", disasm, regs_str, code_context);
                                        
                                        // Call AI (in background task to avoid blocking)
                                        // Ideally we should use a separate tokio task
                                        // For now, simple spawn
                                        // socket is consumed... clone sender?
                                        // socket is mutable, can't clone.
                                        // We need to send response back to *this* socket.
                                        // But `ask_ai` is async. We can await it here.
                                        // `handle_socket` is async.
                                        
                                        // Send "Thinking..." message?
                                        let _ = socket.send(Message::Text(serde_json::to_string(&ServerMessage::AIResponse { text: "Thinking...".to_string() }).unwrap())).await;

                                        match ai::ask_ai(context_str).await {
                                            Ok(ans) => {
                                                let _ = socket.send(Message::Text(serde_json::to_string(&ServerMessage::AIResponse { text: ans }).unwrap())).await;
                                            }
                                            Err(e) => {
                                                let _ = socket.send(Message::Text(serde_json::to_string(&ServerMessage::AIResponse { text: format!("Error: {}", e) }).unwrap())).await;
                                            }
                                        }
                                    }
                                    ClientMessage::GetMemoryWrites { address } => {
                                        let writes = db.get_memory_writes(address);
                                        let response = ServerMessage::MemoryWrites { address, writes };
                                        if let Ok(json) = serde_json::to_string(&response) {
                                            let _ = socket.send(Message::Text(json)).await;
                                        }
                                    }
                                    ClientMessage::GetSlice { clnum, target } => {
                                        let clnums = db.get_slice(clnum, target);
                                        let mut entries = Vec::new();
                                        for c in clnums {
                                            // Inefficient but works for now
                                            // get_trace_log returns Vec<TraceEntry>
                                            if let Some(e) = db.get_trace_log(c, 1, false).first() {
                                                entries.push(e.clone());
                                            }
                                        }
                                        let response = ServerMessage::Slice { entries };
                                        if let Ok(json) = serde_json::to_string(&response) {
                                            let _ = socket.send(Message::Text(json)).await;
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                eprintln!("[ERROR] Failed to parse ClientMessage: {} | Text: {}", e, text);
                            }
                        }
                    }
                    Some(Ok(_)) => {} // Ignore other message types
                    Some(Err(e)) => {
                        eprintln!("WebSocket receive error: {}", e);
                        break;
                    }
                    None => break,
                }
            }
            msg = rx.recv() => {
                if let Ok(msg) = msg {
                    if socket.send(Message::Text(msg)).await.is_err() {
                        break;
                    }
                }
            }
        }
    }
}
