use futures::{SinkExt, StreamExt};
use gloo_net::websocket::{futures::WebSocket, Message};
use serde::{Deserialize, Serialize};
use wasm_bindgen_futures::spawn_local;
use web_sys::HtmlInputElement;
use yew::prelude::*;

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type")]
enum ClientMessage {
    QueryState { clnum: u32 },
    StepForward { current: u32 },
    StepBackward { current: u32 },
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type")]
enum ServerMessage {
    StateUpdate {
        clnum: u32,
        registers: Vec<u64>,
        memory: Vec<u8>,
        memory_addr: u64,
    },
    TraceEvent(serde_json::Value),
    MaxClnum {
        max: u32,
    },
}

#[function_component(App)]
pub fn app() -> Html {
    let trace_log = use_state(Vec::new);
    let current_clnum = use_state(|| 0u32);
    let max_clnum = use_state(|| 0u32);
    let registers = use_state(|| vec![0u64; 16]);
    let memory = use_state(|| vec![0u8; 256]);
    let ws_sender = use_state(|| None::<futures::channel::mpsc::UnboundedSender<Message>>);

    {
        let trace_log = trace_log.clone();
        let current_clnum = current_clnum.clone();
        let max_clnum = max_clnum.clone();
        let registers = registers.clone();
        let memory = memory.clone();
        let ws_sender = ws_sender.clone();

        use_effect_with((), move |_| {
            let ws = WebSocket::open("ws://localhost:3000/ws").unwrap();
            let (mut write, mut read) = ws.split();

            // Create channel for sending messages
            let (tx, mut rx) = futures::channel::mpsc::unbounded();
            ws_sender.set(Some(tx));

            // Spawn task to send messages
            spawn_local(async move {
                while let Some(msg) = rx.next().await {
                    let _ = write.send(msg).await;
                }
            });

            spawn_local(async move {
                while let Some(msg) = read.next().await {
                    if let Ok(Message::Text(text)) = msg {
                        // Try to parse as ServerMessage
                        if let Ok(server_msg) = serde_json::from_str::<ServerMessage>(&text) {
                            match server_msg {
                                ServerMessage::StateUpdate {
                                    clnum,
                                    registers: regs,
                                    memory: mem,
                                    ..
                                } => {
                                    current_clnum.set(clnum);
                                    registers.set(regs);
                                    memory.set(mem);
                                }
                                ServerMessage::MaxClnum { max } => {
                                    max_clnum.set(max);
                                    current_clnum.set(0);
                                }
                                ServerMessage::TraceEvent(_) => {
                                    // Keep raw JSON for display
                                    trace_log.set({
                                        let mut current = (*trace_log).clone();
                                        current.push(text);
                                        if current.len() > 100 {
                                            current.remove(0);
                                        }
                                        current
                                    });
                                }
                            }
                        } else {
                            // Fallback: treat as raw trace event
                            trace_log.set({
                                let mut current = (*trace_log).clone();
                                current.push(text);
                                if current.len() > 100 {
                                    current.remove(0);
                                }
                                current
                            });
                        }
                    }
                }
            });
            || {}
        });
    }

    let on_slider_change = {
        let ws_sender = ws_sender.clone();
        let current_clnum = current_clnum.clone();
        Callback::from(move |e: Event| {
            if let Some(input) = e.target_dyn_into::<HtmlInputElement>() {
                if let Ok(clnum) = input.value().parse::<u32>() {
                    current_clnum.set(clnum);
                    if let Some(sender) = &*ws_sender {
                        let msg = ClientMessage::QueryState { clnum };
                        if let Ok(json) = serde_json::to_string(&msg) {
                            let _ = sender.unbounded_send(Message::Text(json));
                        }
                    }
                }
            }
        })
    };

    let on_step_forward = {
        let ws_sender = ws_sender.clone();
        let current_clnum = current_clnum.clone();
        Callback::from(move |_| {
            let current = *current_clnum;
            if let Some(sender) = &*ws_sender {
                let msg = ClientMessage::StepForward { current };
                if let Ok(json) = serde_json::to_string(&msg) {
                    let _ = sender.unbounded_send(Message::Text(json));
                }
            }
        })
    };

    let on_step_backward = {
        let ws_sender = ws_sender.clone();
        let current_clnum = current_clnum.clone();
        Callback::from(move |_| {
            let current = *current_clnum;
            if let Some(sender) = &*ws_sender {
                let msg = ClientMessage::StepBackward { current };
                if let Ok(json) = serde_json::to_string(&msg) {
                    let _ = sender.unbounded_send(Message::Text(json));
                }
            }
        })
    };

    html! {
        <>
            <style>
                { "
                * { box-sizing: border-box; }
                body { margin: 0; padding: 0; background: #1e1e1e; color: #d4d4d4; font-family: monospace; overflow: hidden; height: 100vh; width: 100vw; }
                .container { display: flex; height: 100vh; width: 100vw; overflow: hidden; }
                .panel { border-right: 1px solid #333; overflow-y: auto; overflow-x: hidden; padding: 10px; }
                .regs { width: 200px; min-width: 200px; background: #252526; flex-shrink: 0; }
                .trace { flex: 1; min-width: 0; background: #1e1e1e; display: flex; flex-direction: column; overflow: hidden; }
                .mem { width: 300px; min-width: 300px; background: #252526; flex-shrink: 0; }
                
                .controls { width: 100%; padding: 10px; background: #252526; border-bottom: 1px solid #333; flex-shrink: 0; }
                .controls-inner { display: flex; align-items: center; gap: 10px; }
                
                .trace-content { flex: 1; overflow-y: auto; overflow-x: hidden; }
                
                .header { font-weight: bold; border-bottom: 1px solid #444; margin-bottom: 5px; padding-bottom: 5px; flex-shrink: 0; }
                .log-entry { white-space: pre-wrap; font-size: 12px; border-bottom: 1px solid #333; padding: 2px 0; }
                .log-entry:hover { background: #2a2d2e; cursor: pointer; }
                
                /* Scrollbar */
                ::-webkit-scrollbar { width: 10px; height: 10px; }
                ::-webkit-scrollbar-track { background: #1e1e1e; }
                ::-webkit-scrollbar-thumb { background: #444; }
                ::-webkit-scrollbar-thumb:hover { background: #555; }
                " }
            </style>

            <div class="container">
                // Registers Panel
                <div class="panel regs">
                    <div class="header">{ "REGISTERS" }</div>
                    {
                        for registers.iter().enumerate().map(|(i, &val)| {
                            let reg_names = ["RAX", "RBX", "RCX", "RDX", "RSI", "RDI", "RBP", "RSP",
                                             "R8", "R9", "R10", "R11", "R12", "R13", "R14", "R15"];
                            let name = if i < reg_names.len() { reg_names[i] } else { "REG" };
                            html! {
                                <div>{ format!("{}: {:016x}", name, val) }</div>
                            }
                        })
                    }
                </div>

                // Trace (Disassembly) Panel
                <div class="panel trace">
                    <div class="header">{ "EXECUTION TRACE" }</div>
                    // Controls
                    <div class="controls">
                        <div class="controls-inner">
                            <button onclick={on_step_backward.clone()} style="padding: 5px 10px; background: #333; color: #d4d4d4; border: 1px solid #555; cursor: pointer;">{ "◀ Step Back" }</button>
                            <input
                                type="range"
                                min="0"
                                max={max_clnum.to_string()}
                                value={current_clnum.to_string()}
                                onchange={on_slider_change.clone()}
                                style="flex: 1;"
                            />
                            <span>{ format!("{} / {}", *current_clnum, *max_clnum) }</span>
                            <button onclick={on_step_forward.clone()} style="padding: 5px 10px; background: #333; color: #d4d4d4; border: 1px solid #555; cursor: pointer;">{ "Step Forward ▶" }</button>
                        </div>
                    </div>
                    <div class="trace-content">
                        {
                            for trace_log.iter().map(|line| html! {
                                <div class="log-entry">{ line }</div>
                            })
                        }
                    </div>
                </div>

                // Memory Panel
                <div class="panel mem">
                    <div class="header">{ "MEMORY" }</div>
                    <div style="font-size: 11px; line-height: 1.4;">
                        {
                            for memory.chunks(16).enumerate().map(|(i, chunk)| {
                                let addr = i * 16;
                                let hex: String = chunk.iter().map(|b| format!("{:02x} ", b)).collect();
                                let ascii: String = chunk.iter().map(|&b| {
                                    if b >= 32 && b < 127 { b as char } else { '.' }
                                }).collect();
                                html! {
                                    <div style="margin-bottom: 2px;">
                                        { format!("{:08x}: {} |{}|", addr, hex, ascii) }
                                    </div>
                                }
                            })
                        }
                    </div>
                </div>
            </div>
        </>
    }
}

fn main() {
    yew::Renderer::<App>::new().render();
}
