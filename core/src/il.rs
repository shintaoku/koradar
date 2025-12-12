use serde::{Serialize, Deserialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Operation {
    Assign { dst: Scalar, src: Expression },
    Store { index: Expression, src: Expression },
    Load { dst: Scalar, index: Expression },
    Branch { target: Expression },
    Intrinsic { intrinsic: String },
    Nop,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Scalar {
    pub name: String,
    pub bits: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Expression {
    Scalar(Scalar),
    Constant(Constant),
    Add(Box<Expression>, Box<Expression>),
    Sub(Box<Expression>, Box<Expression>),
    // ... extend as needed
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Constant {
    pub value: u64,
    pub bits: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Instruction {
    pub operation: Operation,
    pub address: u64,
    pub mnemonic: String,
    pub operands: String,
}

// Simple Control Flow Graph
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ControlFlowGraph {
    pub blocks: Vec<BasicBlock>,
    pub edges: Vec<Edge>,
}

impl ControlFlowGraph {
    pub fn to_mermaid(&self) -> String {
        if self.blocks.is_empty() {
            return String::from("graph TD;\n    Empty[\"No User Code / Empty Trace\"];\n");
        }

        let mut s = String::from("graph TD;\n");
        
        // Define nodes
        for block in &self.blocks {
            let label = if let Some(first) = block.instructions.first() {
                format!("{:x}", first.address)
            } else {
                format!("Block {}", block.index)
            };
            
            // Limit content size for label
            let content = block.instructions.iter()
                .take(5) // Show first 5 instructions
                .map(|i| format!("{:x}: {} {}", i.address, i.mnemonic, i.operands))
                .collect::<Vec<_>>()
                .join("<br/>");
                
            let content = if block.instructions.len() > 5 {
                format!("{}<br/>...", content)
            } else {
                content
            };
            
            // Escape quotes in content
            let content = content.replace("\"", "#quot;"); // Use HTML entity for quote? Or just empty?
            // Mermaid double quote escaping can be tricky.
            // Let's try replacing " with ' first to be safe.
            let content = content.replace("\"", "'");

            // Node definition
            s.push_str(&format!("    block{}[\"{}<br/>{}\"];\n", block.index, label, content));
        }
        
        // Define edges
        for edge in &self.edges {
            s.push_str(&format!("    block{} --> block{};\n", edge.head, edge.tail));
        }

        s
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BasicBlock {
    pub index: usize,
    pub instructions: Vec<Instruction>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Edge {
    pub head: usize,
    pub tail: usize,
    pub condition: Option<Expression>,
}

