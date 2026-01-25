//! Abstract Syntax Tree for sh9script
//!
//! This module defines the AST types that represent parsed shell scripts.

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Script {
    pub statements: Vec<Statement>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Statement {
    /// Empty statement (blank line or comment)
    Empty,
    /// Variable assignment: name=value
    Assignment(Assignment),
    /// Pipeline of commands: cmd1 | cmd2 | ...
    Pipeline(Pipeline),
    /// If statement
    If(IfStatement),
    /// For loop
    For(ForLoop),
    /// While loop
    While(WhileLoop),
    /// Function definition
    FunctionDef(FunctionDef),
    /// Break statement
    Break,
    /// Continue statement
    Continue,
    /// Return statement with optional value
    Return(Option<Word>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Assignment {
    pub name: String,
    pub value: Word,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pipeline {
    pub commands: Vec<Command>,
    pub background: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Command {
    pub name: Word,
    pub args: Vec<Word>,
    pub redirections: Vec<Redirection>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Word {
    pub parts: Vec<WordPart>,
}

impl Word {
    pub fn literal(s: &str) -> Self {
        Word {
            parts: vec![WordPart::Literal(s.to_string())],
        }
    }

    pub fn empty() -> Self {
        Word { parts: vec![] }
    }

    /// Returns true if this is a simple literal word
    pub fn is_literal(&self) -> bool {
        self.parts.len() == 1 && matches!(&self.parts[0], WordPart::Literal(_))
    }

    /// Get the literal value if this is a simple literal word
    pub fn as_literal(&self) -> Option<&str> {
        if self.parts.len() == 1 {
            if let WordPart::Literal(s) = &self.parts[0] {
                return Some(s);
            }
        }
        None
    }
}

impl fmt::Display for Word {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for part in &self.parts {
            match part {
                WordPart::Literal(s) | WordPart::SingleQuoted(s) => write!(f, "{}", s)?,
                WordPart::Variable(name) => write!(f, "${}", name)?,
                WordPart::BracedVariable(name) => write!(f, "${{{}}}", name)?,
                WordPart::Arithmetic(expr) => write!(f, "$(({}))", expr)?,
                WordPart::CommandSub(cmd) => write!(f, "$({})", cmd)?,
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum WordPart {
    Literal(String),
    SingleQuoted(String),
    Variable(String),
    BracedVariable(String),
    Arithmetic(String),
    CommandSub(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Redirection {
    pub kind: RedirectKind,
    pub target: Word,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RedirectKind {
    /// > file (stdout to file, truncate)
    StdoutWrite,
    /// >> file (stdout to file, append)
    StdoutAppend,
    /// < file (stdin from file)
    StdinRead,
    /// 2> file (stderr to file, truncate)
    StderrWrite,
    /// 2>> file (stderr to file, append)
    StderrAppend,
    /// &> file (stdout and stderr to file)
    BothWrite,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IfStatement {
    pub condition: Box<Pipeline>,
    pub then_body: Vec<Statement>,
    pub elif_clauses: Vec<ElifClause>,
    pub else_body: Option<Vec<Statement>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ElifClause {
    pub condition: Pipeline,
    pub body: Vec<Statement>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForLoop {
    pub variable: String,
    pub items: Vec<Word>,
    pub body: Vec<Statement>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WhileLoop {
    pub condition: Box<Pipeline>,
    pub body: Vec<Statement>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionDef {
    pub name: String,
    pub body: Vec<Statement>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TestExpr {
    /// String equality: s1 = s2
    StringEq(Word, Word),
    /// String inequality: s1 != s2
    StringNe(Word, Word),
    /// Numeric comparison: n1 -eq n2
    NumEq(Word, Word),
    /// Numeric comparison: n1 -ne n2
    NumNe(Word, Word),
    /// Numeric comparison: n1 -lt n2
    NumLt(Word, Word),
    /// Numeric comparison: n1 -le n2
    NumLe(Word, Word),
    /// Numeric comparison: n1 -gt n2
    NumGt(Word, Word),
    /// Numeric comparison: n1 -ge n2
    NumGe(Word, Word),
    /// String not empty: -n string
    StringNotEmpty(Word),
    /// String empty: -z string
    StringEmpty(Word),
    /// File exists: -e file
    FileExists(Word),
    /// File is directory: -d file
    IsDirectory(Word),
    /// File is regular file: -f file
    IsFile(Word),
}
