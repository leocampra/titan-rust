//! Compilador da linguagem Titan.
//!
//! Esqueleto da Fase 0 (T0): o pipeline real — lexer, parser, checker, codegen e
//! driver — chega nas tarefas seguintes. Por ora o binário apenas valida os
//! argumentos e reporta que a compilação ainda não está implementada, sempre com
//! mensagens em português e sem entrar em pânico.

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
