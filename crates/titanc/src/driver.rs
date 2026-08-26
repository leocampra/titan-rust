//! Driver: amarra lexer → parser → checker → codegen e invoca o `cargo` para
//! produzir o executável nativo (PRD.md, T7).
//!
//! Fluxo (`compile`):
//! 1. lê o fonte → lexer → parser → checker;
//! 2. gera `<out_dir>/<nome>/src/main.rs` e `<out_dir>/<nome>/Cargo.toml`
//!    (com `titan-runtime` referenciado por caminho absoluto);
//! 3. invoca `cargo build --release` nesse diretório;
//! 4. copia o executável para o diretório atual como `<nome>`.
//!
//! Duas armadilhas do Cargo evitadas aqui (PRD.md, T7):
//! - o `Cargo.toml` gerado leva um `[workspace]` **vazio**, senão o cargo
//!   tenta anexá-lo ao workspace pai e a build quebra;
//! - `titan-runtime` é referenciado por **caminho absoluto**, sem rede nem
//!   registry.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::checker::{self, CheckError};
use crate::codegen::{self, CodegenError};
use crate::lexer::{self, LexError};
use crate::parser::{self, ParseError};

/// Qualquer etapa do pipeline pode falhar; todo caso vira uma mensagem em
/// português, nunca panic (PRD.md, convenções de trabalho).
#[derive(Debug)]
pub enum CompileError {
    Lex(LexError),
    Parse(ParseError),
    Check(Vec<CheckError>),
    Codegen(CodegenError),
    Io {
        context: String,
        source: std::io::Error,
    },
    CargoFailed {
        status: std::process::ExitStatus,
    },
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompileError::Lex(e) => write!(f, "{e}"),
            CompileError::Parse(e) => write!(f, "{e}"),
            CompileError::Check(errs) => {
                let messages: Vec<String> = errs.iter().map(|e| e.to_string()).collect();
                write!(f, "{}", messages.join("\n"))
            }
            CompileError::Codegen(e) => write!(f, "{e}"),
            CompileError::Io { context, source } => {
                write!(f, "{context}: {source}")
            }
            CompileError::CargoFailed { status } => {
                write!(f, "'cargo build --release' falhou ({status}).")
            }
        }
    }
}

impl std::error::Error for CompileError {}

impl From<LexError> for CompileError {
    fn from(e: LexError) -> Self {
        CompileError::Lex(e)
    }
}

impl From<CodegenError> for CompileError {
    fn from(e: CodegenError) -> Self {
        CompileError::Codegen(e)
    }
}

impl From<ParseError> for CompileError {
    fn from(e: ParseError) -> Self {
        CompileError::Parse(e)
    }
}

/// Opções de compilação, espelhando a CLI (`main.rs`).
pub struct Options {
    /// Arquivo `.titan` de entrada.
    pub input: PathBuf,
    /// Diretório onde `build/<nome>/` é criado. Default: diretório atual.
    pub out_dir: PathBuf,
    /// Para depois de gerar o Rust e imprimi-lo, sem invocar o cargo.
    pub emit_rust: bool,
    /// Mostra a invocação do cargo (como o `-v` do `titanc` original).
    pub verbose: bool,
}

/// Deriva `<nome>` de `input` a partir do stem do arquivo (`hello.titan` →
/// `hello`), usado tanto para o diretório de build quanto para o executável
/// final.
fn program_name(input: &Path) -> Result<String, CompileError> {
    input
        .file_stem()
        .and_then(|s| s.to_str())
        .map(str::to_string)
        .ok_or_else(|| CompileError::Io {
            context: format!(
                "não foi possível derivar um nome a partir de '{}'",
                input.display()
            ),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "nome de arquivo inválido",
            ),
        })
}

fn io_err(context: impl Into<String>) -> impl FnOnce(std::io::Error) -> CompileError {
    let context = context.into();
    move |source| CompileError::Io { context, source }
}

/// `Cargo.toml` do projeto gerado: `[workspace]` vazio (para não ser anexado
/// ao workspace pai) e `titan-runtime` por caminho absoluto.
fn generate_cargo_toml(name: &str, runtime_path: &Path) -> String {
    format!(
        "[workspace]\n\n[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\ntitan-runtime = {{ path = {runtime_path:?} }}\n"
    )
}

/// Caminho absoluto de `crates/titan-runtime`, relativo à raiz do workspace
/// deste binário (`CARGO_MANIFEST_DIR` é `crates/titanc` em tempo de build).
fn runtime_crate_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../titan-runtime")
}

/// Executa o pipeline completo. Devolve o caminho do executável final.
pub fn compile(opts: &Options) -> Result<PathBuf, CompileError> {
    let source = std::fs::read_to_string(&opts.input).map_err(io_err(format!(
        "não foi possível ler '{}'",
        opts.input.display()
    )))?;

    let tokens = lexer::lex(&source)?;
    let program = parser::parse(&tokens)?;
    let typed = checker::check(&program).map_err(CompileError::Check)?;
    let rust_code = codegen::generate(&typed)?;

    if opts.emit_rust {
        println!("{rust_code}");
        return Ok(PathBuf::new());
    }

    let name = program_name(&opts.input)?;
    let project_dir = opts.out_dir.join("build").join(&name);
    let src_dir = project_dir.join("src");
    std::fs::create_dir_all(&src_dir).map_err(io_err(format!(
        "não foi possível criar o diretório '{}'",
        src_dir.display()
    )))?;

    std::fs::write(src_dir.join("main.rs"), &rust_code).map_err(io_err(
        "não foi possível escrever o main.rs gerado".to_string(),
    ))?;

    let cargo_toml = generate_cargo_toml(&name, &runtime_crate_path());
    std::fs::write(project_dir.join("Cargo.toml"), cargo_toml).map_err(io_err(
        "não foi possível escrever o Cargo.toml gerado".to_string(),
    ))?;

    let mut command = Command::new("cargo");
    command
        .arg("build")
        .arg("--release")
        .current_dir(&project_dir);

    if opts.verbose {
        eprintln!(
            "titanc: executando '{} build --release' em '{}'",
            command.get_program().to_string_lossy(),
            project_dir.display()
        );
    }

    let status = command.status().map_err(io_err(
        "não foi possível invocar 'cargo build --release'".to_string(),
    ))?;
    if !status.success() {
        return Err(CompileError::CargoFailed { status });
    }

    let built_binary = project_dir.join("target").join("release").join(&name);
    let final_binary = opts.out_dir.join(&name);
    std::fs::copy(&built_binary, &final_binary).map_err(io_err(format!(
        "não foi possível copiar '{}' para '{}'",
        built_binary.display(),
        final_binary.display()
    )))?;

    Ok(final_binary)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn examples_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples")
    }

    fn temp_out_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "titanc-driver-test-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("cria diretório temporário de teste");
        dir
    }

    #[test]
    fn emit_rust_nao_grava_arquivos_nem_invoca_cargo() {
        let out_dir = temp_out_dir("emit-rust");
        let opts = Options {
            input: examples_dir().join("hello.titan"),
            out_dir: out_dir.clone(),
            emit_rust: true,
            verbose: false,
        };

        compile(&opts).unwrap_or_else(|e| panic!("esperava sucesso: {e}"));

        assert!(!out_dir.join("build").exists());
        let _ = std::fs::remove_dir_all(&out_dir);
    }

    #[test]
    fn gera_cargo_toml_com_workspace_vazio_e_path_absoluto() {
        let runtime_path = runtime_crate_path();
        let toml = generate_cargo_toml("hello", &runtime_path);
        assert!(toml.starts_with("[workspace]\n"));
        assert!(runtime_path.is_absolute());
        assert!(toml.contains(&format!("path = {runtime_path:?}")));
    }

    #[test]
    fn compila_e_produz_executavel_que_imprime_e_retorna_zero() {
        let out_dir = temp_out_dir("full-build");
        let opts = Options {
            input: examples_dir().join("hello.titan"),
            out_dir: out_dir.clone(),
            emit_rust: false,
            verbose: false,
        };

        let binary = compile(&opts).unwrap_or_else(|e| panic!("esperava sucesso: {e}"));
        assert_eq!(binary, out_dir.join("hello"));
        assert!(binary.exists());

        let output = Command::new(&binary)
            .output()
            .expect("executa o binário gerado");
        assert_eq!(String::from_utf8_lossy(&output.stdout), "Olá, mundo!\n");
        assert_eq!(output.status.code(), Some(0));

        let _ = std::fs::remove_dir_all(&out_dir);
    }

    #[test]
    fn erro_lexico_nao_gera_arquivos() {
        let out_dir = temp_out_dir("lex-error");
        let bad_source = out_dir.join("ruim.titan");
        std::fs::write(&bad_source, "\"sem fechar").unwrap();

        let opts = Options {
            input: bad_source,
            out_dir: out_dir.clone(),
            emit_rust: false,
            verbose: false,
        };

        let err = compile(&opts).unwrap_err();
        assert!(matches!(err, CompileError::Lex(_)));
        assert!(!out_dir.join("build").exists());

        let _ = std::fs::remove_dir_all(&out_dir);
    }

    #[test]
    fn erro_de_tipo_nao_gera_arquivos() {
        let out_dir = temp_out_dir("check-error");
        let bad_source = out_dir.join("ruim.titan");
        std::fs::write(
            &bad_source,
            "function main(args: {string}): integer\n    print(42)\n    return 0\nend",
        )
        .unwrap();

        let opts = Options {
            input: bad_source,
            out_dir: out_dir.clone(),
            emit_rust: false,
            verbose: false,
        };

        let err = compile(&opts).unwrap_err();
        assert!(matches!(err, CompileError::Check(_)));
        assert!(!out_dir.join("build").exists());

        let _ = std::fs::remove_dir_all(&out_dir);
    }
}
