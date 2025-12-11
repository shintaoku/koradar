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

