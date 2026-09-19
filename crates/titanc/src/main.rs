//! Compilador da linguagem Titan.
//!
//! CLI fina sobre `titanc` (lib): amarra o pipeline completo — lexer, parser,
//! checker, codegen e driver (invocação do `cargo`) — e produz o executável
//! nativo (PRD.md, T7).
//!
//! Uso: `titanc [--emit-rust] [--manifesto DIR|ARQUIVO] [--out DIR] [-v] [arquivo.titan]`
//!
//! O arquivo de entrada é obrigatório, **exceto** com `--manifesto`: ali o
//! `pacote.principal` do `titan.toml` já diz por onde o programa começa
//! (PRD.md, T80).

use std::path::PathBuf;
use std::process::ExitCode;

use titanc::driver::{self, Options};

const USO: &str =
    "uso: titanc [--emit-rust] [--manifesto DIR|ARQUIVO] [--out DIR] [-v] [arquivo.titan]";

fn parse_args(argumentos: &[String]) -> Result<Options, String> {
    let mut input: Option<PathBuf> = None;
    let mut manifesto: Option<PathBuf> = None;
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
            // `--manifesto` aceita o diretório **ou** o próprio `titan.toml`
            // (`manifesto::carregar` trata os dois), para que tanto
            // `--manifesto .` quanto `--manifesto ./titan.toml` funcionem —
            // quem digita não deveria ter de lembrar qual dos dois o
            // compilador quer.
            "--manifesto" => {
                let caminho = it.next().ok_or_else(|| {
                    "'--manifesto' precisa de um diretório ou arquivo em seguida.".to_string()
                })?;
                manifesto = Some(PathBuf::from(caminho));
            }
            _ if input.is_none() => input = Some(PathBuf::from(arg)),
            outro => return Err(format!("argumento inesperado: '{outro}'.")),
        }
    }

    // Sem `--manifesto`, o arquivo é a única fonte possível; com ele, o
    // `principal` do `titan.toml` basta, e a entrada vira redundante. Um
    // `input` vazio nesse caso é inofensivo: `grafo::resolver` só o usa no
    // caminho de arquivo único, que o manifesto explícito já descartou.
    let input = match (input, &manifesto) {
        (Some(input), _) => input,
        (None, Some(_)) => PathBuf::new(),
        (None, None) => return Err("nenhum arquivo de entrada.".to_string()),
    };
    let out_dir = out_dir.unwrap_or_else(|| PathBuf::from("."));

    Ok(Options {
        input,
        manifesto,
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
