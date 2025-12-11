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
    Change, ChangeFlags, TraceDB,
};
use serde_json;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::{broadcast, mpsc};
use tower_http::services::ServeDir;

struct AppState {
    db: Arc<TraceDB>,
    tx: broadcast::Sender<String>,
    max_clnum: Arc<std::sync::atomic::AtomicU32>,
}

// Helper function for sending log entry to channel
async fn send_log(log_tx: &mpsc::Sender<String>, entry: serde_json::Value) {
    if let Ok(json_string) = serde_json::to_string(&entry) {
        let _ = log_tx.send(json_string).await;
    }
}

#[tokio::main]
async fn main() {
    // #region agent log
    let log_path = "/Users/shinta/git/github.com/geohot/qira/.cursor/debug.log";
    let (log_tx, mut log_rx) = mpsc::channel::<String>(1000);

    // Spawn dedicated log writer task
    let log_path_writer = log_path.to_string();
    tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        let mut file = match tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path_writer)
            .await
        {
            Ok(f) => f,
            Err(_) => return,
        };

        while let Some(line) = log_rx.recv().await {
            let _ = file.write_all(line.as_bytes()).await;
            let _ = file.write_all(b"\n").await;
            let _ = file.flush().await;
        }
    });

    let log_entry = serde_json::json!({
        "sessionId": "debug-session",
        "runId": "server-startup",
        "hypothesisId": "A",
        "location": "server/src/main.rs:25",
        "message": "Server starting",
        "data": {},
        "timestamp": SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_millis()
    });
    send_log(&log_tx, log_entry).await;
    // #endregion

    println!("Koradar Server Starting...");

    let db = Arc::new(TraceDB::new(16));
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
        let socket_path = "/tmp/koradar.sock";

        if Path::new(socket_path).exists() {
            let _ = std::fs::remove_file(socket_path);
        }

        let listener = match UnixListener::bind(socket_path) {
            Ok(l) => {
                println!("IPC Listener listening on {}", socket_path);
                l
            }
            Err(e) => {
                panic!("Failed to bind Unix socket: {}", e);
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
                                    bytes: _,
                                } => {
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
                        if let Ok(client_msg) = serde_json::from_str::<ClientMessage>(&text) {
                            match client_msg {
                                ClientMessage::QueryState { clnum } => {
                                    let regs = db.get_registers_at(clnum);
                                    let mem = db.get_memory_at(clnum, 0, 256);
                                    let response = ServerMessage::StateUpdate {
                                        clnum,
                                        registers: regs,
                                        memory: mem,
                                        memory_addr: 0,
                                    };
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
                                    };
                                    if let Ok(json) = serde_json::to_string(&response) {
                                        let _ = socket.send(Message::Text(json)).await;
                                    }
                                }
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
