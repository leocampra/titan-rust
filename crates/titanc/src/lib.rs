//! Biblioteca do compilador da linguagem Titan.
//!
//! Expõe o pipeline completo — lexer, parser, checker, codegen e driver — como
//! API pública, para que outros crates do workspace (o `titan-lsp`, por
//! exemplo) reusem as mesmas funções puras sem invocar o `cargo` (PRD.md,
//! T47).

// `enum_variant_names`: os nós mantêm o prefixo do `ast.lua` (`ExpString`,
// `StatCall`, ...) de propósito, para que o Titan original continue servindo
// de referência viva — ver PRD.md, tarefa T1.
#[allow(clippy::enum_variant_names)]
pub mod ast;
pub mod builtins;
pub mod capabilities;
pub mod checker;
pub mod codegen;
pub mod driver;
pub mod lexer;
pub mod parser;
pub mod types;
