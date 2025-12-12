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

struct AppState {
    db: Arc<TraceDB>,
    tx: broadcast::Sender<String>,
    max_clnum: Arc<std::sync::atomic::AtomicU32>,
}

#[tokio::main]
async fn main() {
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
                                    } => {
                                        if current_clnum < 10 {
                                            // println!("[DEBUG] Server received InsnExec: pc={:x}, bytes={:?}, disasm={:?}", pc, bytes, disasm);
                                            
                                            // Auto-detect Bias on first instruction (or first few)
                                            if current_clnum == 1 {
                                                if let Some(ep) = ipc_db.get_entry_point() {
                                                    let bias = (*pc as i64) - (ep as i64);
                                                    if bias != 0 {
                                                        println!("[INFO] Detected PIE/ASLR bias: {:x} (PC={:x}, Entry={:x})", bias, pc, ep);
                                                        ipc_db.set_bias(bias);
                                                    }
                                                }
                                            }
                                        }
                                        ipc_db.add_instruction(current_clnum, bytes.clone());
                                        if let Some(d) = disasm {
                                            ipc_db.add_instruction_disasm(current_clnum, d.clone());
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
                        // Debug log: received message
                        // println!("Received: {}", text); // Too verbose for all messages

                        match serde_json::from_str::<ClientMessage>(&text) {
                            Ok(client_msg) => {
                                match client_msg {
                                    ClientMessage::QueryState { clnum } => {
                                        let regs = db.get_registers_at(clnum);
                                        let mem = db.get_memory_at(clnum, 0, 256);
                                        let disasm = db.get_disassembly_at(clnum);
                                        let response = ServerMessage::StateUpdate {
                                            clnum,
                                            registers: regs,
                                            memory: mem,
                                            memory_addr: 0,
                                            disassembly: disasm,
                                        };
                                        if let Ok(json) = serde_json::to_string(&response) {
                                            let _ = socket.send(Message::Text(json)).await;
                                        }
                                    }
                                    ClientMessage::GetTraceLog { start, count, only_user_code } => {
                                        // println!("[DEBUG] GetTraceLog: start={}, count={}, only_user_code={}", start, count, only_user_code);
                                        let entries = db.get_trace_log(start, count, only_user_code);
                                        // println!("[DEBUG] GetTraceLog: returning {} entries", entries.len());
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
                                    ClientMessage::GetCFG { only_user_code } => {
                                        // TODO: Run in blocking task if heavy
                                        let cfg = db.analyze_cfg(only_user_code);
                                        let mermaid = cfg.to_mermaid();
                                        println!("[INFO] Generated CFG size: {} bytes", mermaid.len());
                                        let response = ServerMessage::CFG { graph: mermaid };
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
