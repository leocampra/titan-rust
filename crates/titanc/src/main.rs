//! Compilador da linguagem Titan.
//!
//! Esqueleto da Fase 0 (T0): o pipeline real — lexer, parser, checker, codegen e
//! driver — chega nas tarefas seguintes. Por ora o binário apenas valida os
//! argumentos e reporta que a compilação ainda não está implementada, sempre com
//! mensagens em português e sem entrar em pânico.

// `enum_variant_names`: os nós mantêm o prefixo do `ast.lua` (`ExpString`,
// `StatCall`, ...) de propósito, para que o Titan original continue servindo
// de referência viva — ver PRD.md, tarefa T1.
#[allow(dead_code, clippy::enum_variant_names)]
mod ast;
#[allow(dead_code)]
mod checker;
#[allow(dead_code)]
mod lexer;
#[allow(dead_code)]
mod parser;
#[allow(dead_code)]
mod types;

use std::process::ExitCode;

const USO: &str = "uso: titanc <arquivo.titan>";

fn main() -> ExitCode {
    let argumentos: Vec<String> = std::env::args().skip(1).collect();

    match argumentos.as_slice() {
        [] => {
            eprintln!("titanc: nenhum arquivo de entrada.");
            eprintln!("{USO}");
            ExitCode::FAILURE
        }
        [entrada] => {
            eprintln!("titanc: compilação de '{entrada}' ainda não implementada.");
            ExitCode::FAILURE
        }
        _ => {
            eprintln!("titanc: esperado exatamente um arquivo de entrada.");
            eprintln!("{USO}");
            ExitCode::FAILURE
        }
    }
}
