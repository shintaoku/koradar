use futures::{SinkExt, StreamExt};
use gloo_net::websocket::{futures::WebSocket, Message};
use serde::{Deserialize, Serialize};
use wasm_bindgen_futures::spawn_local;
use web_sys::{Event, HtmlInputElement};
use yew::prelude::*;

#[derive(Serialize, Deserialize, Debug, Clone)]
struct TraceEntry {
    clnum: u32,
    address: u64,
    disassembly: String,
    reg_diff: Option<(usize, u64)>,
    mem_access: Option<(u64, u64, bool)>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type")]
enum ClientMessage {
    QueryState {
        clnum: u32,
    },
    GetTraceLog {
        start: u32,
        count: u32,
        only_user_code: bool,
    },
    StepForward {
        current: u32,
    },
    StepBackward {
        current: u32,
    },
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type")]
enum ServerMessage {
    StateUpdate {
        clnum: u32,
        registers: Vec<u64>,
        memory: Vec<u8>,
        memory_addr: u64,
        disassembly: String,
    },
    TraceEvent(serde_json::Value),
    TraceLog {
        entries: Vec<TraceEntry>,
    },
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
    let current_disasm = use_state(|| String::from("Waiting for trace..."));
    let ws_sender = use_state(|| None::<futures::channel::mpsc::UnboundedSender<Message>>);

    let view_mode = use_state(|| "timeline"); // "log" or "timeline"
    let only_user_code = use_state(|| false);
    let timeline_entries = use_state(Vec::<TraceEntry>::new);

    {
        let trace_log = trace_log.clone();
        let current_clnum = current_clnum.clone();
        let max_clnum = max_clnum.clone();
        let registers = registers.clone();
        let memory = memory.clone();
        let current_disasm = current_disasm.clone();
        let ws_sender = ws_sender.clone();
        let timeline_entries = timeline_entries.clone();

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
                                    disassembly,
                                    ..
                                } => {
                                    current_clnum.set(clnum);
                                    registers.set(regs);
                                    memory.set(mem);
                                    current_disasm.set(disassembly);
                                }
                                ServerMessage::MaxClnum { max } => {
                                    max_clnum.set(max);
                                    // Don't reset current_clnum here, it disturbs tracing
                                }
                                ServerMessage::TraceLog { entries } => {
                                    timeline_entries.set(entries);
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

    let toggle_view = {
        let view_mode = view_mode.clone();
        Callback::from(move |_: MouseEvent| {
            if *view_mode == "log" {
                view_mode.set("timeline");
            } else {
                view_mode.set("log");
            }
        })
    };

    let fetch_timeline = {
        let ws_sender = ws_sender.clone();
        let current_clnum = current_clnum.clone();
        let only_user_code = *only_user_code;
        Callback::from(move |_: MouseEvent| {
            // Fetch around current clnum
            let center = *current_clnum;
            let start = center.saturating_sub(50);
            let count = 100;

            if let Some(sender) = &*ws_sender {
                let msg = ClientMessage::GetTraceLog {
                    start,
                    count,
                    only_user_code,
                };
                if let Ok(json) = serde_json::to_string(&msg) {
                    let _ = sender.unbounded_send(Message::Text(json));
                }
            }
        })
    };

    let toggle_user_code = {
        let only_user_code = only_user_code.clone();
        let ws_sender = ws_sender.clone();
        Callback::from(move |e: Event| {
            let target: Option<HtmlInputElement> = e.target_dyn_into();
            if let Some(input) = target {
                let val = input.checked();
                only_user_code.set(val);

                // Re-fetch timeline
                if let Some(sender) = &*ws_sender {
                    let msg = ClientMessage::GetTraceLog {
                        start: 0, // Reset to 0 or keep current? For now, fetch from 0 to see overview
                        count: 1000,
                        only_user_code: val,
                    };
                    if let Ok(json) = serde_json::to_string(&msg) {
                        let _ = sender.unbounded_send(Message::Text(json));
                    }
                }
            }
        })
    };

    // Auto-refresh timeline when clnum, view_mode, or only_user_code changes
    {
        let ws_sender = ws_sender.clone();
        let current_clnum = current_clnum.clone();
        let view_mode = view_mode.clone();
        let only_user_code = only_user_code.clone();

        use_effect_with(
            (
                current_clnum.clone(),
                view_mode.clone(),
                only_user_code.clone(),
            ),
            move |(current_clnum, view_mode, only_user_code)| {
                if **view_mode == "timeline" {
                    let center = **current_clnum;
                    let start = center.saturating_sub(20);
                    let count = 40;
                    if let Some(sender) = &*ws_sender {
                        let msg = ClientMessage::GetTraceLog {
                            start,
                            count,
                            only_user_code: **only_user_code,
                        };
                        if let Ok(json) = serde_json::to_string(&msg) {
                            let _ = sender.unbounded_send(Message::Text(json));
                        }
                    }
                }
                || {}
            },
        );
    }

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
                
                .header { font-weight: bold; border-bottom: 1px solid #444; margin-bottom: 5px; padding-bottom: 5px; flex-shrink: 0; display: flex; justify-content: space-between; }
                .log-entry { white-space: pre-wrap; font-size: 12px; border-bottom: 1px solid #333; padding: 2px 0; }
                .log-entry:hover { background: #2a2d2e; cursor: pointer; }
                
                .timeline-table { width: 100%; border-collapse: collapse; font-size: 12px; }
                .timeline-table th { text-align: left; border-bottom: 1px solid #555; padding: 4px; color: #aaa; }
                .timeline-table td { padding: 2px 4px; border-bottom: 1px solid #333; }
                .timeline-row:hover { background: #2a2d2e; cursor: pointer; }
                .timeline-row.active { background: #094771; }
                .col-clnum { width: 60px; color: #569cd6; }
                .col-addr { width: 80px; color: #ce9178; }
                .col-insn { color: #d4d4d4; }
                .col-effect { color: #6a9955; }

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
                    <div class="header">
                        <span>{ "EXECUTION TRACE" }</span>
                        <div>
                             <button onclick={toggle_view} style="font-size: 10px; margin-right: 5px;">{ if *view_mode == "log" { "Switch to Timeline" } else { "Switch to Raw Log" } }</button>
                             {
                                if *view_mode == "timeline" {
                                    html! {
                                        <label style="font-size: 10px; cursor: pointer;">
                                            <input type="checkbox" checked={*only_user_code} onchange={toggle_user_code} />
                                            {" User Code Only"}
                                        </label>
                                    }
                                } else {
                                    html! {}
                                }
                             }
                        </div>
                    </div>

                    // Current Instruction Display
                    <div style="padding: 10px; background: #2d2d2d; border-bottom: 1px solid #444; font-size: 14px; color: #4ec9b0;">
                        { &*current_disasm }
                    </div>

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
                            if *view_mode == "log" {
                                html! {
                                    for trace_log.iter().map(|line| html! {
                                        <div class="log-entry">{ line }</div>
                                    })
                                }
                            } else {
                                html! {
                                    <table class="timeline-table">
                                        <thead>
                                            <tr>
                                                <th>{ "Time" }</th>
                                                <th>{ "Addr" }</th>
                                                <th>{ "Instruction" }</th>
                                                <th>{ "Effects" }</th>
                                            </tr>
                                        </thead>
                                        <tbody>
                                            {
                                                for timeline_entries.iter().map(|entry| {
                                                    let is_active = entry.clnum == *current_clnum;
                                                    let class = if is_active { "timeline-row active" } else { "timeline-row" };
                                                    let clnum = entry.clnum;
                                                    let on_click = {
                                                        let ws_sender = ws_sender.clone();
                                                        let current_clnum = current_clnum.clone();
                                                        Callback::from(move |_| {
                                                            current_clnum.set(clnum);
                                                            if let Some(sender) = &*ws_sender {
                                                                let msg = ClientMessage::QueryState { clnum };
                                                                if let Ok(json) = serde_json::to_string(&msg) {
                                                                    let _ = sender.unbounded_send(Message::Text(json));
                                                                }
                                                            }
                                                        })
                                                    };

                                                    let effect_str = {
                                                        let mut s = String::new();
                                                        if let Some((idx, val)) = entry.reg_diff {
                                                            let reg_names = ["RAX", "RBX", "RCX", "RDX", "RSI", "RDI", "RBP", "RSP",
                                                                            "R8", "R9", "R10", "R11", "R12", "R13", "R14", "R15"];
                                                            let name = if idx < reg_names.len() { reg_names[idx] } else { "REG" };
                                                            s.push_str(&format!("{}={:x} ", name, val));
                                                        }
                                                        if let Some((addr, val, is_write)) = entry.mem_access {
                                                            let op = if is_write { "W" } else { "R" };
                                                            s.push_str(&format!("Mem{}[{:x}]={:x}", op, addr, val));
                                                        }
                                                        s
                                                    };

                                                    html! {
                                                        <tr class={class} onclick={on_click}>
                                                            <td class="col-clnum">{ entry.clnum }</td>
                                                            <td class="col-addr">{ format!("{:08x}", entry.address) }</td>
                                                            <td class="col-insn">{ &entry.disassembly }</td>
                                                            <td class="col-effect">{ effect_str }</td>
                                                        </tr>
                                                    }
                                                })
                                            }
                                        </tbody>
                                    </table>
                                }
                            }
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
