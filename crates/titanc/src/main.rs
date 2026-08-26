//! Compilador da linguagem Titan.
//!
//! CLI que amarra o pipeline completo — lexer, parser, checker, codegen e
//! driver (invocação do `cargo`) — e produz o executável nativo (PRD.md, T7).
//!
//! Uso: `titanc [--emit-rust] [--out DIR] [-v] <arquivo.titan>`

// `enum_variant_names`: os nós mantêm o prefixo do `ast.lua` (`ExpString`,
// `StatCall`, ...) de propósito, para que o Titan original continue servindo
// de referência viva — ver PRD.md, tarefa T1.
#[allow(dead_code, clippy::enum_variant_names)]
mod ast;
mod builtins;
#[allow(dead_code)]
mod capabilities;
mod checker;
mod codegen;
mod driver;
mod lexer;
mod parser;
#[allow(dead_code)]
mod types;

use std::path::PathBuf;
use std::process::ExitCode;

use driver::Options;

const USO: &str = "uso: titanc [--emit-rust] [--out DIR] [-v] <arquivo.titan>";

fn parse_args(argumentos: &[String]) -> Result<Options, String> {
    let mut input: Option<PathBuf> = None;
    let mut out_dir: Option<PathBuf> = None;
    let mut emit_rust = false;
    let mut verbose = false;

    let mut it = argumentos.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--emit-rust" => emit_rust = true,
            "-v" => verbose = true,
            "--out" => {
                let dir = it
                    .next()
                    .ok_or_else(|| "'--out' precisa de um diretório em seguida.".to_string())?;
                out_dir = Some(PathBuf::from(dir));
            }
            _ if input.is_none() => input = Some(PathBuf::from(arg)),
            outro => return Err(format!("argumento inesperado: '{outro}'.")),
        }
    }

    let input = input.ok_or_else(|| "nenhum arquivo de entrada.".to_string())?;
    let out_dir = out_dir.unwrap_or_else(|| PathBuf::from("."));

    Ok(Options {
        input,
        out_dir,
        emit_rust,
        verbose,
    })
}

fn main() -> ExitCode {
    let argumentos: Vec<String> = std::env::args().skip(1).collect();

    let opts = match parse_args(&argumentos) {
        Ok(opts) => opts,
        Err(mensagem) => {
            eprintln!("titanc: {mensagem}");
            eprintln!("{USO}");
            return ExitCode::FAILURE;
        }
    };

    match driver::compile(&opts) {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("titanc: {e}");
            ExitCode::FAILURE
        }
    }
}
