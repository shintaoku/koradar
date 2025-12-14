use futures::{SinkExt, StreamExt};
use gloo_net::websocket::{futures::WebSocket, Message};
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::{Event, HtmlInputElement, InputEvent, KeyboardEvent};
use yew::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_name = renderMermaid)]
    fn render_mermaid(id: &str, text: &str) -> js_sys::Promise;

    #[wasm_bindgen(js_name = searchFunctionInCFG)]
    fn search_function_in_cfg(func_name: &str);
}

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
        memory_addr: Option<u64>,
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
    GetCFG {
        only_user_code: bool,
        start_from_main: bool,
    },
    GetSlice {
        clnum: u32,
        target: String,
    },
    AskAI {
        clnum: u32,
    },
    GetMemoryWrites {
        address: u64,
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
    CFG {
        graph: String,
    },
    AIResponse {
        text: String,
    },
    MemoryWrites {
        address: u64,
        writes: Vec<u32>,
    },
    Slice {
        entries: Vec<TraceEntry>,
    },
}

#[function_component(App)]
pub fn app() -> Html {
    let trace_log = use_state(Vec::new);
    let current_clnum = use_state(|| 0u32);
    let max_clnum = use_state(|| 0u32);
    let registers = use_state(|| vec![0u64; 16]);
    let memory = use_state(|| vec![0u8; 256]);
    let memory_addr = use_state(|| 0u64);
    let memory_writes = use_state(Vec::<u32>::new);
    let current_disasm = use_state(|| String::from("Waiting for trace..."));
    let ws_sender = use_state(|| None::<futures::channel::mpsc::UnboundedSender<Message>>);

    let ai_response = use_state(|| String::new());
    let is_ai_loading = use_state(|| false);

    let view_mode = use_state(|| "timeline"); // "log" or "timeline" or "cfg"
    let only_user_code = use_state(|| false);
    let start_from_main = use_state(|| false);
    let search_term = use_state(|| String::new());
    let slice_target = use_state(|| String::new());
    
    let timeline_entries = use_state(Vec::<TraceEntry>::new);
    let cfg_graph = use_state(|| String::new());

    {
        let trace_log = trace_log.clone();
        let current_clnum = current_clnum.clone();
        let max_clnum = max_clnum.clone();
        let registers = registers.clone();
        let memory = memory.clone();
        let memory_addr = memory_addr.clone();
        let memory_writes = memory_writes.clone();
        let current_disasm = current_disasm.clone();
        let ws_sender = ws_sender.clone();
        let timeline_entries = timeline_entries.clone();
        let cfg_graph = cfg_graph.clone();
        let ai_response = ai_response.clone();
        let is_ai_loading = is_ai_loading.clone();
        let view_mode = view_mode.clone();
        let slice_target = slice_target.clone();

        use_effect_with((), move |_| {
            let ws = WebSocket::open("ws://localhost:3000/ws").unwrap();
            let (mut write, mut read) = ws.split();

            // Create channel for sending messages
            let (tx, mut rx) = futures::channel::mpsc::unbounded();
            ws_sender.set(Some(tx.clone()));

            // Setup global CFG click handler
            let tx_clone = tx.clone();
            let callback = Closure::wrap(Box::new(move |clnum_val: u32| {
                // Send QueryState message
                let msg = ClientMessage::QueryState {
                    clnum: clnum_val,
                    memory_addr: None,
                };
                if let Ok(json) = serde_json::to_string(&msg) {
                    let _ = tx_clone.unbounded_send(Message::Text(json));
                }
            }) as Box<dyn FnMut(u32)>);

            let window = web_sys::window().unwrap();
            js_sys::Reflect::set(
                &window,
                &JsValue::from_str("onCfgNodeClick"),
                callback.as_ref().unchecked_ref(),
            )
            .unwrap();
            callback.forget();

            // Initial State from URL Hash
            // Format: #clnum=123
            if let Ok(hash) = window.location().hash() {
                if hash.starts_with("#clnum=") {
                    if let Ok(clnum) = hash[7..].parse::<u32>() {
                        let msg = ClientMessage::QueryState {
                            clnum,
                            memory_addr: None,
                        };
                        if let Ok(json) = serde_json::to_string(&msg) {
                            let _ = tx.unbounded_send(Message::Text(json));
                        }
                    }
                }
            }

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
                                    memory_addr: mem_addr,
                                    disassembly,
                                    ..
                                } => {
                                    // #region agent log
                                    {
                                        web_sys::console::log_1(&format!("StateUpdate: clnum={}, regs_len={}", clnum, regs.len()).into());
                                        if !regs.is_empty() {
                                            web_sys::console::log_1(&format!("Reg0 (RAX?): {:x}", regs[0]).into());
                                        }
                                    }
                                    // #endregion
                                    current_clnum.set(clnum);
                                    registers.set(regs);
                                    memory.set(mem);
                                    memory_addr.set(mem_addr);
                                    current_disasm.set(disassembly);

                                    // Update URL hash
                                    let window = web_sys::window().unwrap();
                                    let _ = window.location().set_hash(&format!("clnum={}", clnum));
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
                                ServerMessage::CFG { graph } => {
                                    cfg_graph.set(graph.clone());
                                    // Trigger render
                                    spawn_local(async move {
                                        let promise = render_mermaid("cfg-view", &graph);
                                        let _ = JsFuture::from(promise).await;
                                    });
                                }
                                ServerMessage::AIResponse { text } => {
                                    ai_response.set(text);
                                    is_ai_loading.set(false);
                                }
                                ServerMessage::MemoryWrites { address: _, writes } => {
                                    memory_writes.set(writes);
                                }
                                ServerMessage::Slice { entries } => {
                                    timeline_entries.set(entries);
                                    view_mode.set("slice");
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
        let memory_addr = memory_addr.clone();
        Callback::from(move |e: Event| {
            if let Some(input) = e.target_dyn_into::<HtmlInputElement>() {
                if let Ok(clnum) = input.value().parse::<u32>() {
                    current_clnum.set(clnum);
                    if let Some(sender) = &*ws_sender {
                        let msg = ClientMessage::QueryState {
                            clnum,
                            memory_addr: Some(*memory_addr),
                        };
                        if let Ok(json) = serde_json::to_string(&msg) {
                            let _ = sender.unbounded_send(Message::Text(json));
                        }
                    }
                }
            }
        })
    };

    let on_memory_addr_change = {
        let ws_sender = ws_sender.clone();
        let current_clnum = current_clnum.clone();
        let memory_addr = memory_addr.clone();
        Callback::from(move |e: Event| {
            if let Some(input) = e.target_dyn_into::<HtmlInputElement>() {
                if let Ok(addr) = u64::from_str_radix(&input.value(), 16) {
                    memory_addr.set(addr);
                    if let Some(sender) = &*ws_sender {
                        let msg = ClientMessage::QueryState {
                            clnum: *current_clnum,
                            memory_addr: Some(addr),
                        };
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
        let ws_sender = ws_sender.clone();
        let only_user_code = *only_user_code;
        let start_from_main = *start_from_main;
        
        Callback::from(move |_: MouseEvent| {
            if *view_mode == "log" {
                view_mode.set("timeline");
            } else if *view_mode == "timeline" || *view_mode == "slice" {
                view_mode.set("cfg");
                // Fetch CFG
                if let Some(sender) = &*ws_sender {
                    let msg = ClientMessage::GetCFG { only_user_code, start_from_main };
                    if let Ok(json) = serde_json::to_string(&msg) {
                        let _ = sender.unbounded_send(Message::Text(json));
                    }
                }
            } else {
                view_mode.set("log");
            }
        })
    };

    let toggle_user_code = {
        let only_user_code = only_user_code.clone();
        Callback::from(move |e: Event| {
            let target: Option<HtmlInputElement> = e.target_dyn_into();
            if let Some(input) = target {
                let val = input.checked();
                only_user_code.set(val);
            }
        })
    };

    let toggle_start_main = {
        let start_from_main = start_from_main.clone();
        Callback::from(move |e: Event| {
            let target: Option<HtmlInputElement> = e.target_dyn_into();
            if let Some(input) = target {
                let val = input.checked();
                start_from_main.set(val);
            }
        })
    };
    
    let on_search_change = {
        let search_term = search_term.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(input) = e.target_dyn_into::<HtmlInputElement>() {
                search_term.set(input.value());
            }
        })
    };
    
    let on_search_submit = {
        let search_term = search_term.clone();
        Callback::from(move |e: KeyboardEvent| {
            if e.key() == "Enter" {
                let term = (*search_term).clone();
                search_function_in_cfg(&term);
            }
        })
    };

    let on_ask_ai = {
        let ws_sender = ws_sender.clone();
        let current_clnum = current_clnum.clone();
        let is_ai_loading = is_ai_loading.clone();
        let ai_response = ai_response.clone();

        Callback::from(move |_| {
            is_ai_loading.set(true);
            ai_response.set(String::new());
            if let Some(sender) = &*ws_sender {
                let msg = ClientMessage::AskAI {
                    clnum: *current_clnum,
                };
                if let Ok(json) = serde_json::to_string(&msg) {
                    let _ = sender.unbounded_send(Message::Text(json));
                }
            }
        })
    };

    let on_get_writes = {
        let ws_sender = ws_sender.clone();
        let memory_addr = memory_addr.clone();
        Callback::from(move |_| {
             if let Some(sender) = &*ws_sender {
                 let msg = ClientMessage::GetMemoryWrites {
                     address: *memory_addr,
                 };
                 if let Ok(json) = serde_json::to_string(&msg) {
                     let _ = sender.unbounded_send(Message::Text(json));
                 }
             }
        })
    };

    let on_slice_target_change = {
        let slice_target = slice_target.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(input) = e.target_dyn_into::<HtmlInputElement>() {
                slice_target.set(input.value());
            }
        })
    };

    let on_slice = {
        let ws_sender = ws_sender.clone();
        let current_clnum = current_clnum.clone();
        let slice_target = slice_target.clone();
        Callback::from(move |_| {
            if let Some(sender) = &*ws_sender {
                let msg = ClientMessage::GetSlice {
                    clnum: *current_clnum,
                    target: (*slice_target).clone(),
                };
                if let Ok(json) = serde_json::to_string(&msg) {
                    let _ = sender.unbounded_send(Message::Text(json));
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
        let start_from_main = start_from_main.clone();

        use_effect_with(
            (
                current_clnum.clone(),
                view_mode.clone(),
                only_user_code.clone(),
                start_from_main.clone(),
            ),
            move |(current_clnum, view_mode, only_user_code, start_from_main)| {
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
                } else if **view_mode == "cfg" {
                    if let Some(sender) = &*ws_sender {
                        let msg = ClientMessage::GetCFG {
                            only_user_code: **only_user_code,
                            start_from_main: **start_from_main,
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
                
                .ai-panel { margin: 10px; padding: 10px; background: #252526; border: 1px solid #444; border-radius: 4px; }
                .ai-panel pre { white-space: pre-wrap; margin: 0; font-family: monospace; font-size: 12px; color: #9cdcfe; }
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
                             <button onclick={toggle_view} style="font-size: 10px; margin-right: 5px;">
                                { match *view_mode {
                                    "log" => "Switch to Timeline",
                                    "timeline" => "Switch to CFG",
                                    "slice" => "Switch to CFG",
                                    "cfg" => "Switch to Raw Log",
                                    _ => "Unknown"
                                } }
                             </button>
                             {
                                if *view_mode == "timeline" || *view_mode == "cfg" || *view_mode == "slice" {
                                    html! {
                                        <>
                                            <label style="font-size: 10px; cursor: pointer; margin-right: 5px;">
                                                <input type="checkbox" checked={*only_user_code} onchange={toggle_user_code} />
                                                {" User Code"}
                                            </label>
                                            {
                                                if *view_mode == "cfg" {
                                                    html! {
                                                        <>
                                                            <label style="font-size: 10px; cursor: pointer; margin-right: 5px;">
                                                                <input type="checkbox" checked={*start_from_main} onchange={toggle_start_main} />
                                                                {" From Main"}
                                                            </label>
                                                            <input 
                                                                type="text" 
                                                                placeholder="Search Func..." 
                                                                value={(*search_term).clone()}
                                                                oninput={on_search_change}
                                                                onkeydown={on_search_submit}
                                                                style="font-size: 10px; padding: 2px; width: 100px; background: #333; color: white; border: 1px solid #555;"
                                                            />
                                                        </>
                                                    }
                                                } else { html! {} }
                                            }
                                        </>
                                    }
                                } else {
                                    html! {}
                                }
                             }
                             <button onclick={on_ask_ai} style="font-size: 10px; margin-left: 5px; background: #0e639c; color: white; border: none; cursor: pointer;">
                                { if *is_ai_loading { "Thinking..." } else { "Ask AI ✨" } }
                             </button>
                             <div style="display: flex; gap: 5px; margin-left: 10px; align-items: center;">
                                 <input
                                     type="text"
                                     placeholder="Slice (rax..)"
                                     value={(*slice_target).clone()}
                                     oninput={on_slice_target_change}
                                     style="font-size: 10px; width: 80px; background: #333; color: white; border: 1px solid #555; padding: 2px;"
                                 />
                                 <button onclick={on_slice} style="font-size: 10px; cursor: pointer; padding: 2px;">{ "Slice" }</button>
                             </div>
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
                            if !ai_response.is_empty() {
                                html! {
                                    <div class="ai-panel">
                                        <div class="header" style="color: #ce9178;">{ "AI Analysis" }</div>
                                        <pre>{ &*ai_response }</pre>
                                    </div>
                                }
                            } else { html! {} }
                        }

                        {
                            if *view_mode == "log" {
                                html! {
                                    for trace_log.iter().map(|line| html! {
                                        <div class="log-entry">{ line }</div>
                                    })
                                }
                            } else if *view_mode == "cfg" {
                                html! {
                                    <div id="cfg-view" style="width: 100%; height: 100%; overflow: auto; background: white;">
                                        // Mermaid will render here
                                        { "Loading CFG..." }
                                    </div>
                                }
                            } else {
                                html! {
                                    <>
                                        { if *view_mode == "slice" {
                                            html! { <div style="background: #333; color: #fff; padding: 2px; font-size: 10px; border-bottom: 1px solid #555;">{ format!("Slice Results for '{}'", *slice_target) }</div> }
                                        } else { html! {} } }
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
                                                        let memory_addr = memory_addr.clone();
                                                        Callback::from(move |_| {
                                                            current_clnum.set(clnum);
                                                            if let Some(sender) = &*ws_sender {
                                                                let msg = ClientMessage::QueryState { clnum, memory_addr: Some(*memory_addr) };
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
                                    </>
                                }
                            }
                        }
                    </div>
                </div>

                // Memory Panel
                <div class="panel mem">
                    <div class="header" style="display: flex; justify-content: space-between; align-items: center;">
                        <span>{ "MEMORY" }</span>
                        <div style="display: flex; gap: 5px;">
                            <input
                                type="text"
                                placeholder="Addr (Hex)"
                                onchange={on_memory_addr_change}
                                style="width: 80px; font-size: 11px; background: #333; color: #d4d4d4; border: 1px solid #555; padding: 2px;"
                                value={format!("{:x}", *memory_addr)}
                            />
                            <button onclick={on_get_writes} style="font-size: 10px; cursor: pointer; padding: 2px;">{ "Writes" }</button>
                        </div>
                    </div>
                    <div style="font-size: 11px; line-height: 1.4; font-family: monospace;">
                        {
                            for memory.chunks(16).enumerate().map(|(i, chunk)| {
                                let addr = *memory_addr + (i * 16) as u64;
                                let hex: String = chunk.iter().map(|b| format!("{:02x} ", b)).collect();
                                let ascii: String = chunk.iter().map(|&b| {
                                    if b >= 32 && b < 127 { b as char } else { '.' }
                                }).collect();
                                html! {
                                    <div style="margin-bottom: 2px; display: flex;">
                                        <span style="color: #ce9178; width: 70px; flex-shrink: 0;">{ format!("{:08x}:", addr) }</span>
                                        <span style="color: #d4d4d4; margin-right: 10px; width: 230px; flex-shrink: 0;">{ hex }</span>
                                        <span style="color: #6a9955;">{ format!("|{}|", ascii) }</span>
                                    </div>
                                }
                            })
                        }
                    </div>
                    <div style="margin-top: 10px; border-top: 1px solid #444; padding-top: 5px;">
                        <div style="font-weight: bold; margin-bottom: 5px; font-size: 11px;">{ "Write History" }</div>
                         {
                             if memory_writes.is_empty() {
                                 html! { <div style="color: #666; font-size: 10px;">{ "No writes found" }</div> }
                             } else {
                                 html! {
                                     <div style="display: flex; flex-wrap: wrap; gap: 5px; font-size: 10px;">
                                         {
                                             for memory_writes.iter().map(|&w| {
                                                 let on_click = {
                                                     let ws_sender = ws_sender.clone();
                                                     let current_clnum = current_clnum.clone();
                                                     let memory_addr = memory_addr.clone();
                                                     Callback::from(move |_| {
                                                         current_clnum.set(w);
                                                         if let Some(sender) = &*ws_sender {
                                                             let msg = ClientMessage::QueryState {
                                                                 clnum: w,
                                                                 memory_addr: Some(*memory_addr),
                                                             };
                                                             if let Ok(json) = serde_json::to_string(&msg) {
                                                                 let _ = sender.unbounded_send(Message::Text(json));
                                                             }
                                                         }
                                                     })
                                                 };
                                                 html! {
                                                     <span onclick={on_click} style="cursor: pointer; color: #569cd6; text-decoration: underline;">{ w }</span>
                                                 }
                                             })
                                         }
                                     </div>
                                 }
                             }
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
