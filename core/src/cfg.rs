use crate::db::{TraceDB, ChangeFlags};
use crate::il::{ControlFlowGraph, BasicBlock, Edge, Instruction, Operation};
use std::collections::{HashMap, HashSet};

impl TraceDB {
    pub fn analyze_cfg(&self, only_user_code: bool) -> ControlFlowGraph {
        let changes = self.changes.read();
        
        // Pass 1: Identify leaders and edges from trace
        let mut block_starts = HashSet::new();
        
        // Filter changes to get only PC changes (instructions)
        let total_pc_changes = changes.iter()
            .filter(|c| ChangeFlags::from_bits_truncate(c.flags).contains(ChangeFlags::IS_START))
            .count();

        let pc_changes: Vec<_> = changes.iter()
            .filter(|c| ChangeFlags::from_bits_truncate(c.flags).contains(ChangeFlags::IS_START))
            .filter(|c| !only_user_code || self.is_user_code(c.address))
            .collect();

        if only_user_code {
             println!("[DEBUG] analyze_cfg: Total PC changes: {}, After user_code filter: {}", total_pc_changes, pc_changes.len());
             
             // #region agent log
            {
                use std::fs::OpenOptions;
                use std::io::Write;
                let path = "/tmp/koradar_debug.log";
                if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
                    let _ = writeln!(file, "{{\"id\":\"log_cfg_filter\",\"timestamp\":{},\"location\":\"cfg:analyze_cfg\",\"message\":\"Filter Stats\",\"data\":{{\"total\":{}, \"kept\":{}}},\"sessionId\":\"debug-session\"}}", 
                        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis(),
                        total_pc_changes, pc_changes.len()
                    );
                    
                    // Log samples
                    let dropped = changes.iter()
                        .filter(|c| ChangeFlags::from_bits_truncate(c.flags).contains(ChangeFlags::IS_START))
                        .find(|c| !self.is_user_code(c.address));
                    
                    if let Some(d) = dropped {
                         let _ = writeln!(file, "{{\"id\":\"log_cfg_dropped\",\"timestamp\":{},\"location\":\"cfg:analyze_cfg\",\"message\":\"Sample Dropped Addr\",\"data\":{{\"address\":{}, \"bias\":{}, \"is_user_code\":false}},\"sessionId\":\"debug-session\"}}", 
                            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis(),
                            d.address, self.get_bias()
                        );
                    }
                }
            }
            // #endregion
        }

        if pc_changes.is_empty() {
            return ControlFlowGraph { blocks: vec![], edges: vec![] };
        }

        block_starts.insert(pc_changes[0].address);
        
        for i in 0..pc_changes.len()-1 {
            let curr = pc_changes[i];
            let next = pc_changes[i+1];
            
            let curr_addr = curr.address;
            let next_addr = next.address;
            
            // Calculate expected next address (sequential)
            // We need instruction size. TraceDB has it in instructions map?
            // Or we can infer: if next_addr is close to curr_addr, it's sequential.
            // But jumps can be short.
            // Better: use `get_instruction_bytes` to find size.
            
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
                // Edge detected: curr -> next
                // 'next' is a leader (target of jump)
                block_starts.insert(next_addr);
                // 'curr' is the end of a block.
                // But we don't know the start of 'curr's block yet easily without tracking.
                
                // Let's refine strategy:
                // We track "current block start".
                // If jump: 
                //   Add edge (current_block_start -> next_addr)
                //   Next instruction starts a new block at next_addr.
            }
        }
        
        // Pass 2: Build Blocks
        // Iterate again, grouping by detected block starts.
        
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
            
            // Add instruction to current block
            // Avoid duplicates if loop?
            // If we are in a loop, we might visit the same block start again.
            // Dynamic trace unwinds loops.
            // But we want a CFG where loops are cycles.
            // So if we visit a `block_start` that we have already processed?
            
            // Actually, simply:
            // 1. Collect all unique `block_starts`.
            // 2. For each `block_start`, disassemble until next `block_start` or end of flow.
            // But "next block start" depends on flow.
            
            // Hybrid approach:
            // 1. Identify all jump targets (leaders).
            // 2. Also identify function entries? (leaders).
            // 3. Scan memory/instructions to build blocks from leaders.
            
            // Simpler Dynamic Approach:
            // Iterate trace.
            // If `is_jump`:
            //    Edge: `current_block_start` -> `next_pc`
            //    `next_pc` becomes a Leader.
            //    `current_pc` ends the block.
            
            let disassembly = self.get_disassembly_at(curr.clnum);
            let mnemonic = disassembly.split_whitespace().next().unwrap_or("???").to_string();
            let operands = disassembly[mnemonic.len()..].trim().to_string();
            
            // Check for jump
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
            
            // Add to current instructions (if not already there - BasicBlock contains unique instructions usually)
            // But here we are iterating trace.
            // We only want to store the "static" block content once.
            
            if !current_insns.iter().any(|insn: &Instruction| insn.address == curr.address) {
                 current_insns.push(Instruction {
                    operation: Operation::Nop, // TODO: semantic lifting
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
            nodes.push(BasicBlock {
                index: i,
                instructions: final_blocks.get(start).unwrap().clone(),
            });
        }
        
        let mut graph_edges = Vec::new();
        for (src, dst) in final_edges {
            if let (Some(&head), Some(&tail)) = (node_indices.get(&src), node_indices.get(&dst)) {
                graph_edges.push(Edge {
                    head, // Source index
                    tail, // Dest index (Wait, standard Edge is src->dst. Head/Tail terminology varies. Let's assume head=src, tail=dst)
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

