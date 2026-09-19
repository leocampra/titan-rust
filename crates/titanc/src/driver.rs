//! Driver: amarra lexer → parser → checker → codegen e invoca o `cargo` para
//! produzir o executável nativo (PRD.md, T7).
//!
//! Fluxo (`compile`):
//! 1. resolve o conjunto de fontes (`grafo::resolver`, T80) — um arquivo só,
//!    ou os módulos declarados no `titan.toml` em ordem topológica — e passa
//!    cada um pelo checker;
//! 2. gera `<out_dir>/<nome>/src/main.rs` e `<out_dir>/<nome>/Cargo.toml`
//!    (com `titan-runtime` e uma entrada por módulo importado, cada um
//!    referenciado por caminho absoluto, T43);
//! 3. invoca `cargo build --release` nesse diretório;
//! 4. copia o executável para o diretório atual como `<nome>`.
//!
//! Duas armadilhas do Cargo evitadas aqui (PRD.md, T7):
//! - o `Cargo.toml` gerado leva um `[workspace]` **vazio**, senão o cargo
//!   tenta anexá-lo ao workspace pai e a build quebra;
//! - cada dependência é referenciada por **caminho absoluto**, sem rede nem
//!   registry.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::checker::{self, CheckError};
use crate::codegen::{self, CodegenError};
use crate::grafo::{self, Grafo, GrafoError};
use crate::lexer::LexError;
use crate::parser::ParseError;

/// Qualquer etapa do pipeline pode falhar; todo caso vira uma mensagem em
/// português, nunca panic (PRD.md, convenções de trabalho).
#[derive(Debug)]
pub enum CompileError {
    Lex(LexError),
    Parse(ParseError),
    Check(Vec<CheckError>),
    /// Falha ao resolver o conjunto de fontes: manifesto inválido, módulo
    /// declarado que não existe, `import` que não resolve, ciclo (T80).
    Grafo(GrafoError),
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
            CompileError::Grafo(e) => write!(f, "{e}"),
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

impl From<GrafoError> for CompileError {
    /// Erro léxico e sintático de um módulo cai nas variantes que o driver
    /// já tinha, e não em [`CompileError::Grafo`]: quem distingue as etapas
    /// do pipeline pelo tipo do erro (o LSP, os testes) continua vendo
    /// `Lex`/`Parse` onde sempre viu. [`CompileError::Grafo`] fica para o
    /// que é genuinamente novo na T80 — manifesto, módulo que não resolve,
    /// ciclo.
    ///
    /// O arquivo não se perde nessa passagem: ele entra na `message`, que é
    /// pública nos dois erros. Só que **apenas** quando há mais de um
    /// arquivo em jogo — num programa de arquivo único a mensagem tem de
    /// sair como sempre saiu, e o caminho que o usuário acabou de digitar
    /// na linha de comando não é informação nova.
    fn from(e: GrafoError) -> Self {
        match e {
            GrafoError::Lex {
                caminho,
                mut source,
                multi_modulo,
            } => {
                if multi_modulo {
                    source.message = format!("em '{}': {}", caminho.display(), source.message);
                }
                CompileError::Lex(source)
            }
            GrafoError::Parse {
                caminho,
                mut source,
                multi_modulo,
            } => {
                if multi_modulo {
                    source.message = format!("em '{}': {}", caminho.display(), source.message);
                }
                CompileError::Parse(source)
            }
            outro => CompileError::Grafo(outro),
        }
    }
}

/// Opções de compilação, espelhando a CLI (`main.rs`).
pub struct Options {
    /// Arquivo `.titan` de entrada.
    pub input: PathBuf,
    /// `--manifesto DIR|ARQUIVO` (T80): o `titan.toml` a usar. `None` faz o
    /// driver procurar um ao lado de `input` e, não achando, compilar o
    /// arquivo único de sempre.
    pub manifesto: Option<PathBuf>,
    /// Diretório onde `build/<nome>/` é criado. Default: diretório atual.
    pub out_dir: PathBuf,
    /// Para depois de gerar o Rust e imprimi-lo, sem invocar o cargo.
    pub emit_rust: bool,
    /// Mostra a invocação do cargo (como o `-v` do `titanc` original).
    pub verbose: bool,
}

fn io_err(context: impl Into<String>) -> impl FnOnce(std::io::Error) -> CompileError {
    let context = context.into();
    move |source| CompileError::Io { context, source }
}

/// Uma dependência do `Cargo.toml` gerado: nome do crate e caminho absoluto.
struct CrateDep {
    name: &'static str,
    path: PathBuf,
}

/// `Cargo.toml` do projeto gerado: `[workspace]` vazio (para não ser anexado
/// ao workspace pai) e uma entrada `[dependencies]` por `deps`, cada uma por
/// caminho absoluto. `deps` sempre inclui `titan-runtime`, mais uma por
/// módulo importado pelo programa (T43) — programa sem `import` não paga o
/// build das capabilities que não usa.
fn generate_cargo_toml(name: &str, deps: &[CrateDep]) -> String {
    let mut out = format!(
        "[workspace]\n\n[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\n"
    );
    for dep in deps {
        out.push_str(&format!("{} = {{ path = {:?} }}\n", dep.name, dep.path));
    }
    out
}

/// Caminho absoluto de um crate do workspace a partir do caminho relativo à
/// raiz do workspace (`Capability::crate_path`, ex. `crates/titan-data`).
/// `CARGO_MANIFEST_DIR` é `crates/titanc` em tempo de build, daí o `../..`
/// até a raiz.
fn workspace_crate_path(relative_to_root: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative_to_root)
}

/// Caminho absoluto de `crates/titan-runtime`.
fn runtime_crate_path() -> PathBuf {
    workspace_crate_path("crates/titan-runtime")
}

/// Monta a lista de dependências do `Cargo.toml` gerado: `titan-runtime`
/// sempre primeiro, mais uma por módulo importado pelo programa (T43).
fn collect_deps(program: &crate::ast::Program) -> Vec<CrateDep> {
    let mut deps = vec![CrateDep {
        name: "titan-runtime",
        path: runtime_crate_path(),
    }];
    for capability in checker::imported_capabilities(program) {
        deps.push(CrateDep {
            name: capability.crate_name,
            path: workspace_crate_path(capability.crate_path),
        });
    }
    deps
}

/// Monta o grafo de fontes do programa (T80): o manifesto explícito de
/// `--manifesto`, o `titan.toml` ao lado do arquivo de entrada, ou — não
/// havendo nenhum dos dois — o arquivo único de sempre.
fn resolver_grafo(opts: &Options) -> Result<Grafo, CompileError> {
    let manifesto = grafo::localizar_manifesto(&opts.input, opts.manifesto.as_deref())?;
    if opts.verbose {
        match &manifesto {
            Some(m) => eprintln!(
                "titanc: manifesto '{}' ({} módulo(s) declarado(s))",
                m.diretorio.join(crate::manifesto::NOME_ARQUIVO).display(),
                m.modulos.len()
            ),
            None => eprintln!(
                "titanc: sem manifesto; compilando '{}' como arquivo único",
                opts.input.display()
            ),
        }
    }
    Ok(grafo::resolver(&opts.input, manifesto.as_ref())?)
}

/// Checa todos os módulos do grafo, na ordem topológica, e devolve os
/// programas tipados na mesma ordem.
///
/// A ordem importa desde já, mesmo que a T80 ainda não passe símbolos de um
/// módulo para outro: é ela que garante que, quando a T81 ligar os escopos,
/// toda dependência já terá sido checada quando quem a importa for checado.
///
/// Os erros de todos os módulos saem **juntos**, e não só os do primeiro que
/// falha: quem acabou de escrever três módulos prefere a lista inteira a
/// três compilações seguidas. As mensagens vêm prefixadas pelo módulo,
/// porque `CheckError` carrega uma `Loc` sem saber de que arquivo ela é.
fn checar_grafo(grafo: &Grafo) -> Result<Vec<checker::CheckedProgram>, CompileError> {
    let mut checados = Vec::with_capacity(grafo.modulos.len());
    let mut erros = Vec::new();

    for modulo in &grafo.modulos {
        match checker::check(&modulo.programa) {
            Ok(checado) => {
                // Avisos (T76) não impedem a compilação, mas precisam ser
                // vistos: saem em stderr, para não se misturarem ao Rust que
                // `--emit-rust` manda para stdout.
                for aviso in &checado.warnings {
                    eprintln!(
                        "titanc: aviso{} (linha {}, coluna {}): {}",
                        prefixo_de_modulo(grafo, modulo),
                        aviso.loc.line,
                        aviso.loc.col,
                        aviso.message
                    );
                }
                checados.push(checado);
            }
            Err(modulo_erros) => erros.extend(
                modulo_erros
                    .into_iter()
                    .map(|erro| prefixar(erro, grafo, modulo)),
            ),
        }
    }

    if erros.is_empty() {
        Ok(checados)
    } else {
        Err(CompileError::Check(erros))
    }
}

/// `" em 'lexer'"` num programa multi-módulo, e nada num arquivo único —
/// dizer "em 'hello'" de um programa que só tem um arquivo seria ruído, e
/// mudaria a saída que a T80 precisa preservar byte-a-byte.
fn prefixo_de_modulo(grafo: &Grafo, modulo: &grafo::Modulo) -> String {
    if grafo.arquivo_unico() {
        String::new()
    } else {
        format!(" em '{}'", modulo.nome)
    }
}

/// Prefixa a mensagem de um erro de checagem com o módulo de origem,
/// preservando a `Loc` — que segue apontando para a linha dentro do arquivo
/// daquele módulo.
fn prefixar(mut erro: CheckError, grafo: &Grafo, modulo: &grafo::Modulo) -> CheckError {
    if !grafo.arquivo_unico() {
        erro.message = format!("em '{}': {}", modulo.nome, erro.message);
    }
    erro
}

/// Executa o pipeline completo. Devolve o caminho do executável final.
pub fn compile(opts: &Options) -> Result<PathBuf, CompileError> {
    let grafo = resolver_grafo(opts)?;
    let checados = checar_grafo(&grafo)?;

    // A emissão multi-módulo (um `mod` Rust por módulo Titan) é a T82, e o
    // `import` de módulo de usuário só passa a tipar na T81. Até lá o Rust
    // sai do módulo principal, e os demais foram lidos, parseados e
    // checados — o que já paga o grafo, porque é aqui que ciclo, módulo
    // inexistente e erro de sintaxe numa dependência são pegos, antes de
    // qualquer checagem.
    let rust_code = codegen::generate(&checados[grafo.principal].program)?;

    if opts.emit_rust {
        println!("{rust_code}");
        return Ok(PathBuf::new());
    }

    let name = grafo.nome.clone();
    let project_dir = opts.out_dir.join("build").join(&name);
    let src_dir = project_dir.join("src");
    std::fs::create_dir_all(&src_dir).map_err(io_err(format!(
        "não foi possível criar o diretório '{}'",
        src_dir.display()
    )))?;

    std::fs::write(src_dir.join("main.rs"), &rust_code).map_err(io_err(
        "não foi possível escrever o main.rs gerado".to_string(),
    ))?;

    let cargo_toml = generate_cargo_toml(&name, &collect_deps(&grafo.principal().programa));
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

    use crate::lexer;
    use crate::parser;

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
            manifesto: None,
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
        let deps = vec![CrateDep {
            name: "titan-runtime",
            path: runtime_path.clone(),
        }];
        let toml = generate_cargo_toml("hello", &deps);
        assert!(toml.starts_with("[workspace]\n"));
        assert!(runtime_path.is_absolute());
        assert!(toml.contains(&format!("path = {runtime_path:?}")));
    }

    #[test]
    fn programa_sem_import_so_depende_do_titan_runtime() {
        let source = "function main(args: {string}): integer\n    print(\"oi\")\n    return 0\nend";
        let tokens = lexer::lex(source).unwrap();
        let program = parser::parse(&tokens).unwrap();

        let deps = collect_deps(&program);

        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "titan-runtime");
    }

    #[test]
    fn programa_com_import_ganha_dependencia_do_modulo() {
        let source = "import data\n\nfunction main(args: {string}): integer\n    return 0\nend";
        let tokens = lexer::lex(source).unwrap();
        let program = parser::parse(&tokens).unwrap();

        let deps = collect_deps(&program);

        assert_eq!(deps.len(), 2);
        assert_eq!(deps[0].name, "titan-runtime");
        assert_eq!(deps[1].name, "titan-data");

        let toml = generate_cargo_toml("com_data", &deps);
        assert!(toml.contains("titan-runtime = "));
        assert!(toml.contains("titan-data = "));
    }

    #[test]
    fn compila_e_produz_executavel_que_imprime_e_retorna_zero() {
        let out_dir = temp_out_dir("full-build");
        let opts = Options {
            input: examples_dir().join("hello.titan"),
            manifesto: None,
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

            manifesto: None,
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

            manifesto: None,
            out_dir: out_dir.clone(),
            emit_rust: false,
            verbose: false,
        };

        let err = compile(&opts).unwrap_err();
        assert!(matches!(err, CompileError::Check(_)));
        assert!(!out_dir.join("build").exists());

        let _ = std::fs::remove_dir_all(&out_dir);
    }

    // ------------------------------------------------------------------
    // T80 — manifesto e grafo de módulos
    // ------------------------------------------------------------------

    /// Monta um programa multi-módulo em disco e devolve o diretório raiz.
    fn projeto(rotulo: &str, arquivos: &[(&str, &str)]) -> PathBuf {
        let dir = temp_out_dir(rotulo);
        std::fs::create_dir_all(dir.join("src")).expect("cria src/");
        for (nome, conteudo) in arquivos {
            std::fs::write(dir.join(nome), conteudo).expect("escreve arquivo do projeto");
        }
        dir
    }

    fn opts_de(dir: &Path, manifesto: Option<PathBuf>, input: PathBuf) -> Options {
        Options {
            input,
            manifesto,
            out_dir: dir.to_path_buf(),
            emit_rust: true,
            verbose: false,
        }
    }

    /// Executa um binário recém-copiado, tolerando o `ExecutableFileBusy`
    /// que o Linux devolve enquanto o descritor de escrita do `fs::copy`
    /// ainda não fechou de fato — um `execve` num arquivo aberto para
    /// escrita é `ETXTBSY`, e sob paralelismo de testes a janela existe.
    /// Não é defeito do compilador, e sim do instante em que se executa.
    fn executar(binario: &Path) -> std::process::Output {
        for _ in 0..50 {
            match Command::new(binario).output() {
                Ok(saida) => return saida,
                Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Err(e) => panic!("executa o binário: {e}"),
            }
        }
        panic!("o binário seguiu ocupado depois de 1s");
    }

    const MAIN_VAZIA: &str =
        "function main(args: {string}): integer\n    print(\"ok\")\n    return 0\nend\n";

    /// O critério de aceite central da T80: sem manifesto, o Rust emitido
    /// para cada exemplo tem de ser **exatamente** o de antes. Aqui isso é
    /// verificado pela via mais direta possível — o grafo de um vértice
    /// produz o mesmo `TypedProgram` que o `read_to_string` + parse fazia,
    /// e portanto o mesmo Rust.
    #[test]
    fn arquivo_unico_emite_o_mesmo_rust_que_o_pipeline_direto() {
        for exemplo in ["hello", "nucleo", "compostos", "dados", "lexer"] {
            let caminho = examples_dir().join(format!("{exemplo}.titan"));

            // O pipeline de antes da T80, escrito à mão.
            let fonte = std::fs::read_to_string(&caminho).expect("lê o exemplo");
            let tokens = lexer::lex(&fonte).expect("exemplo lexa");
            let programa = parser::parse(&tokens).expect("exemplo parseia");
            let direto =
                codegen::generate(&checker::check(&programa).expect("exemplo tipa").program)
                    .expect("exemplo emite");

            // O pipeline da T80, pelo grafo.
            let grafo = grafo::resolver(&caminho, None).expect("arquivo único resolve");
            assert!(grafo.arquivo_unico(), "{exemplo} deveria ser arquivo único");
            let pelo_grafo = codegen::generate(
                &checar_grafo(&grafo).expect("exemplo tipa pelo grafo")[grafo.principal].program,
            )
            .expect("exemplo emite pelo grafo");

            assert_eq!(
                direto, pelo_grafo,
                "o Rust de '{exemplo}.titan' mudou com a T80"
            );
        }
    }

    #[test]
    fn arquivo_unico_nomeia_o_programa_pelo_stem_como_antes() {
        let grafo = grafo::resolver(&examples_dir().join("hello.titan"), None)
            .expect("arquivo único resolve");

        assert_eq!(grafo.nome, "hello");
    }

    #[test]
    fn titan_toml_ao_lado_do_fonte_e_usado_sem_a_opcao() {
        let dir = projeto(
            "acha-manifesto",
            &[
                ("src/main.titan", MAIN_VAZIA),
                (
                    "src/titan.toml",
                    "[pacote]\nnome = \"achado\"\nprincipal = \"main.titan\"\n",
                ),
            ],
        );

        let opts = opts_de(&dir, None, dir.join("src/main.titan"));
        let grafo = resolver_grafo(&opts).expect("o manifesto ao lado é achado");

        assert_eq!(grafo.nome, "achado");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn manifesto_explicito_dispensa_o_arquivo_de_entrada() {
        let dir = projeto(
            "manifesto-explicito",
            &[
                ("src/main.titan", MAIN_VAZIA),
                (
                    "titan.toml",
                    "[pacote]\nnome = \"explicito\"\nprincipal = \"src/main.titan\"\n",
                ),
            ],
        );

        // `input` vazio é o que a CLI passa quando só `--manifesto` foi dado.
        let opts = opts_de(&dir, Some(dir.clone()), PathBuf::new());
        let grafo = resolver_grafo(&opts).expect("o manifesto explícito basta");

        assert_eq!(grafo.nome, "explicito");
        assert_eq!(grafo.principal().caminho, dir.join("src/main.titan"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ciclo_de_imports_falha_antes_de_qualquer_checagem() {
        let dir = projeto(
            "ciclo",
            &[
                ("src/main.titan", "import a\n"),
                ("src/a.titan", "import b\n"),
                ("src/b.titan", "import a\n"),
                (
                    "titan.toml",
                    "[pacote]\nnome = \"p\"\nprincipal = \"src/main.titan\"\n\n                     [modulos]\na = \"src/a.titan\"\nb = \"src/b.titan\"\n",
                ),
            ],
        );

        let opts = opts_de(&dir, Some(dir.clone()), PathBuf::new());
        let erro = compile(&opts).expect_err("o ciclo a → b → a deve ser rejeitado");

        // O erro é de grafo, e não de checagem: nenhum dos módulos chegou a
        // ser tipado, embora nenhum deles tenha `main`.
        assert!(
            matches!(erro, CompileError::Grafo(grafo::GrafoError::Ciclo { .. })),
            "esperava erro de ciclo, veio: {erro}"
        );
        assert!(
            erro.to_string().contains("ciclo: a → b → a"),
            "a mensagem precisa nomear o ciclo: {erro}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn modulo_declarado_mas_inexistente_da_erro_claro() {
        let dir = projeto(
            "modulo-sumido",
            &[
                ("src/main.titan", MAIN_VAZIA),
                (
                    "titan.toml",
                    "[pacote]\nnome = \"p\"\nprincipal = \"src/main.titan\"\n\n                     [modulos]\nlexer = \"src/lexer.titan\"\n",
                ),
            ],
        );

        let opts = opts_de(&dir, Some(dir.clone()), PathBuf::new());
        let erro = compile(&opts).expect_err("'src/lexer.titan' não existe");

        let mensagem = erro.to_string();
        assert!(
            mensagem.contains("o módulo 'lexer' aponta para"),
            "mensagem inesperada: {mensagem}"
        );
        assert!(
            mensagem.contains("que não existe"),
            "mensagem inesperada: {mensagem}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn erros_de_checagem_de_varios_modulos_saem_juntos_e_prefixados() {
        // Dois módulos, cada um com um erro de tipo diferente. Os dois têm de
        // aparecer numa compilação só, cada um nomeando o seu módulo.
        let dir = projeto(
            "erros-juntos",
            &[
                (
                    "src/main.titan",
                    "function main(args: {string}): integer\n    print(42)\n    return 0\nend\n",
                ),
                (
                    "src/util.titan",
                    "function util(): integer\n    return \"texto\"\nend\n",
                ),
                (
                    "titan.toml",
                    "[pacote]\nnome = \"p\"\nprincipal = \"src/main.titan\"\n\n                     [modulos]\nutil = \"src/util.titan\"\n",
                ),
            ],
        );

        let opts = opts_de(&dir, Some(dir.clone()), PathBuf::new());
        let erro = compile(&opts).expect_err("os dois módulos têm erro de tipo");

        let mensagem = erro.to_string();
        assert!(
            mensagem.contains("em 'util':"),
            "faltou o erro de 'util': {mensagem}"
        );
        assert!(
            mensagem.contains("em 'p':"),
            "faltou o erro do principal: {mensagem}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn erro_de_arquivo_unico_nao_ganha_prefixo_de_modulo() {
        // O contraponto do teste acima: num programa de um arquivo só, dizer
        // "em 'ruim'" seria ruído, e mudaria a mensagem que sempre saiu.
        let out_dir = temp_out_dir("sem-prefixo");
        let fonte = out_dir.join("ruim.titan");
        std::fs::write(
            &fonte,
            "function main(args: {string}): integer\n    print(42)\n    return 0\nend",
        )
        .unwrap();

        let erro = compile(&opts_de(&out_dir, None, fonte)).expect_err("erro de tipo");

        assert!(
            !erro.to_string().contains("em 'ruim'"),
            "arquivo único não deve ganhar prefixo: {erro}"
        );

        let _ = std::fs::remove_dir_all(&out_dir);
    }

    #[test]
    fn manifesto_da_o_nome_do_executavel_e_do_diretorio_de_build() {
        let dir = projeto(
            "nome-do-build",
            &[
                ("src/main.titan", MAIN_VAZIA),
                (
                    "titan.toml",
                    "[pacote]\nnome = \"batizado\"\nprincipal = \"src/main.titan\"\n",
                ),
            ],
        );

        let opts = Options {
            input: PathBuf::new(),
            manifesto: Some(dir.clone()),
            out_dir: dir.clone(),
            emit_rust: false,
            verbose: false,
        };

        let binario = compile(&opts).unwrap_or_else(|e| panic!("esperava sucesso: {e}"));

        // O nome vem de `pacote.nome`, e não do stem `main` do arquivo.
        assert_eq!(binario, dir.join("batizado"));
        assert!(dir.join("build").join("batizado").is_dir());

        let saida = executar(&binario);
        assert_eq!(String::from_utf8_lossy(&saida.stdout), "ok\n");
        assert_eq!(saida.status.code(), Some(0));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
