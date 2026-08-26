//! Tabela de funções da stdlib (PRD.md, T25 — detalhe A).
//!
//! Hoje só `print`, mas o checker e o codegen conheciam esse nome de forma
//! independente e não sincronizada (`Checker::new` fazia hard-code do tipo, e
//! `emit_call` comparava a string diretamente). Esta tabela é a única fonte de
//! verdade: `Checker::new` itera [`BUILTINS`] para popular o escopo global, e
//! `emit_call` consulta [`lookup`] para decidir o caminho Rust do runtime.

use crate::types::Type;

/// Uma função da stdlib do Titan, mapeada para o runtime em Rust.
pub struct Builtin {
    /// Nome pelo qual o programa Titan chama a função (`print`).
    pub titan_name: &'static str,
    /// Caminho Rust totalmente qualificado que o codegen emite
    /// (`titan_runtime::print`).
    pub rust_path: &'static str,
    pub params: &'static [Type],
    pub rettype: Type,
}

pub const BUILTINS: &[Builtin] = &[Builtin {
    titan_name: "print",
    rust_path: "titan_runtime::print",
    params: &[Type::String],
    rettype: Type::Nil,
}];

/// Busca um builtin pelo nome usado no fonte Titan.
pub fn lookup(titan_name: &str) -> Option<&'static Builtin> {
    BUILTINS.iter().find(|b| b.titan_name == titan_name)
}
