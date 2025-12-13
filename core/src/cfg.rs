use crate::db::{TraceDB, ChangeFlags};
use crate::il::{ControlFlowGraph, BasicBlock, Edge, Instruction, Operation};
use std::collections::{HashMap, HashSet};

impl TraceDB {
    pub fn analyze_cfg(&self, only_user_code: bool, start_from_main: bool) -> ControlFlowGraph {
        let changes = self.changes.read();
        
        // Pass 1: Identify leaders and edges from trace
        let mut block_starts = HashSet::new();
        
        let mut min_clnum = 0;
        if start_from_main {
            if let Some(static_addr) = self.find_symbol_by_name("main") {
                let bias = self.get_bias();
                // StaticAddr = RunAddr - Bias  => RunAddr = StaticAddr + Bias
                // Note: bias can be negative, so be careful with types
                let run_addr = (static_addr as i128 + bias as i128) as u64;
                
                // Find first execution of main
                if let Some(first_exec) = changes.iter().find(|c| {
                    c.address == run_addr && ChangeFlags::from_bits_truncate(c.flags).contains(ChangeFlags::IS_START)
                }) {
                    min_clnum = first_exec.clnum;
                    println!("[DEBUG] Found main at run_addr={:x}, clnum={}", run_addr, min_clnum);
                } else {
                    println!("[DEBUG] 'main' symbol found at static {:x} (run {:x}), but no execution trace found.", static_addr, run_addr);
                }
            } else {
                println!("[DEBUG] 'main' symbol not found in symbol table.");
            }
        }

        // #region agent log
        {
            use std::fs::OpenOptions;
            use std::io::Write;
            let path = "/Users/shinta/git/github.com/geohot/qira/.cursor/debug.log";
            if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
                let _ = writeln!(file, "{{\"id\":\"log_cfg_filter\",\"timestamp\":{},\"location\":\"core/cfg.rs:analyze_cfg\",\"message\":\"Filter Params\",\"data\":{{\"start_from_main\":{}, \"min_clnum\":{}, \"bias\":{}}},\"sessionId\":\"debug-session\",\"runId\":\"debug-run\",\"hypothesisId\":\"filter-logic\"}}", 
                    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis(),
                    start_from_main,
                    min_clnum,
                    self.get_bias()
                );
            }
        }
        // #endregion

        // Filter changes to get only PC changes (instructions)
        let total_pc_changes = changes.iter()
            .filter(|c| c.clnum >= min_clnum)
            .filter(|c| ChangeFlags::from_bits_truncate(c.flags).contains(ChangeFlags::IS_START))
            .count();

        let pc_changes: Vec<_> = changes.iter()
            .filter(|c| c.clnum >= min_clnum)
            .filter(|c| ChangeFlags::from_bits_truncate(c.flags).contains(ChangeFlags::IS_START))
            .filter(|c| !only_user_code || self.is_user_code(c.address))
            .collect();

        if only_user_code {
             println!("[DEBUG] analyze_cfg: Total PC changes: {}, After user_code filter: {}", total_pc_changes, pc_changes.len());
        }

        if pc_changes.is_empty() {
            return ControlFlowGraph { blocks: vec![], edges: vec![] };
        }

        block_starts.insert(pc_changes[0].address);
        
        // Keep track of the first clnum for each block start address *encountered in this trace*
        // Ideally we want the clnum corresponding to the *execution* of the block.
        // But the graph merges all executions of the same block into one node.
        // So we just pick the *first* time it was executed?
        // Or should we not merge? 
        // Standard CFG merges.
        // So we'll point to the first execution.
        let mut block_first_clnum: HashMap<u64, u32> = HashMap::new();
        block_first_clnum.insert(pc_changes[0].address, pc_changes[0].clnum);
        
        for i in 0..pc_changes.len()-1 {
            let curr = pc_changes[i];
            let next = pc_changes[i+1];
            
            let curr_addr = curr.address;
            let next_addr = next.address;
            
            let mut is_jump = true;
            if let Some(bytes) = self.instructions.get(&curr.clnum) {
                if !bytes.is_empty() {
                    let size = bytes.len() as u64;
                    if curr_addr + size == next_addr {
                        is_jump = false;
                    }
                }
            }
            
            if is_jump {
                block_starts.insert(next_addr);
                block_first_clnum.entry(next_addr).or_insert(next.clnum);
            }
        }
        
        // Pass 2: Build Blocks
        
        let mut final_blocks = HashMap::new();
        let mut final_edges = HashSet::new();
        
        let mut current_start = pc_changes[0].address;
        let mut current_insns = Vec::new();
        
        for i in 0..pc_changes.len() {
            let curr = pc_changes[i];
            
            // If this address is a known block start (and not the first one we are building), finish previous block
            if block_starts.contains(&curr.address) && curr.address != current_start {
                // Finish current block
                final_blocks.insert(current_start, current_insns.clone());
                
                // Add edge from previous instruction to this one
                if i > 0 {
                    final_edges.insert((current_start, curr.address)); 
                }
                
                // Start new
                current_start = curr.address;
                current_insns.clear();
            }
            
            let disassembly = self.get_disassembly_at(curr.clnum);
            let mnemonic = disassembly.split_whitespace().next().unwrap_or("???").to_string();
            let operands = disassembly[mnemonic.len()..].trim().to_string();
            
            let mut is_jump = false;
            if i < pc_changes.len() - 1 {
                let next = pc_changes[i+1];
                if let Some(bytes) = self.instructions.get(&curr.clnum) {
                     if !bytes.is_empty() {
                        if curr.address + bytes.len() as u64 != next.address {
                            is_jump = true;
                        }
                     }
                }
            }
            
            if !current_insns.iter().any(|insn: &Instruction| insn.address == curr.address) {
                 current_insns.push(Instruction {
                    operation: Operation::Nop, 
                    address: curr.address,
                    mnemonic,
                    operands,
                });
            }
            
            if is_jump {
                // Record edge
                if i < pc_changes.len() - 1 {
                    let next_addr = pc_changes[i+1].address;
                    final_edges.insert((current_start, next_addr));
                    
                    // Finish block
                    final_blocks.insert(current_start, current_insns.clone());
                    
                    // Start new block
                    current_start = next_addr;
                    current_insns.clear();
                }
            }
        }
        // Final block
        final_blocks.insert(current_start, current_insns);
        
        // Construct Graph
        let mut nodes = Vec::new();
        let mut node_indices = HashMap::new();
        
        // Sort blocks by address for stability
        let mut sorted_starts: Vec<_> = final_blocks.keys().cloned().collect();
        sorted_starts.sort();
        
        for (i, start) in sorted_starts.iter().enumerate() {
            node_indices.insert(*start, i);
            let instructions = final_blocks.get(start).unwrap().clone();
            
            let bias = self.get_bias();
            let static_addr = (*start as i128 - bias as i128) as u64;
            let symbol = self.find_symbol(static_addr).map(|(name, _)| name);
            let clnum = *block_first_clnum.get(start).unwrap_or(&0);

            nodes.push(BasicBlock {
                index: i,
                instructions,
                symbol,
                clnum,
            });
        }
        
        let mut graph_edges = Vec::new();
        for (src, dst) in final_edges {
            if let (Some(&head), Some(&tail)) = (node_indices.get(&src), node_indices.get(&dst)) {
                graph_edges.push(Edge {
                    head, 
                    tail, 
                    condition: None,
                });
            }
        }
        
        ControlFlowGraph {
            blocks: nodes,
            edges: graph_edges,
        }
    }
}
