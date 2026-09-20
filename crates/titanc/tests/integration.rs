//! Teste de integração ponta a ponta (PRD.md, T8): invoca o binário `titanc`
//! de verdade (não as funções internas do pipeline) via `Command`, exercendo
//! a CLI como um usuário faria — `cargo test` aciona o `cargo build` do
//! binário automaticamente antes de rodar este arquivo.
//!
//! Frentes:
//! - caminho feliz: compila `examples/hello.titan` e confere **stdout e exit
//!   code** do executável gerado (critério de aceite do PRD.md, T8);
//! - caminho feliz da Fase 1 (PRD.md, T17): compila `examples/nucleo.titan`
//!   — aritmética, `if`, `while`, `for` e atribuição em funções reais
//!   (fatorial e fibonacci) — e confere stdout completo e exit code;
//! - caminho feliz da Fase 2 (PRD.md, T32): compila `examples/compostos.titan`
//!   — arrays, records e maps, incluindo mutação in-place via `&mut` e
//!   semântica de valor na atribuição — e confere stdout completo e exit
//!   code;
//! - suíte de negativos de T4/T5 rodando pelo pipeline completo: cada
//!   construção fora do subconjunto da Fase 0 precisa produzir uma mensagem
//!   de erro clara na saída do `titanc`, nunca um panic (sem "thread
//!   'main' panicked");
//! - suíte consolidada da Fase 2 (PRD.md, T31): tudo que **segue** fora de
//!   escopo após arrays/records/maps serem aceitos — métodos, `import`,
//!   `break`, bitwise, `//`, `Option`, `as` e as regras de tipos de
//!   record/map — continua rejeitado com erro claro. `v[i]`,
//!   `{...}` e `#` saíram desta lista: têm suporte real no codegen desde a
//!   T30; bitwise e `//` saíram na T61, `repeat`/`until` na T64, os
//!   retornos múltiplos na T65 (tipagem) e T66 (a tupla emitida) e o
//!   multi-assign na T67, e o que resta deles é o negativo de tipo
//!   (`1.5 & 2`, condição do `until` não-boolean, aridade de `return` e de
//!   multi-assign);
//! - arquivos `.titan` reais do Titan original nunca panicam ao serem
//!   processados (compilam ou falham com erro claro), e os que usam somente
//!   o idioma de arrays já suportado (`sieve.titan`, `selection_sort.titan`)
//!   compilam e executam de verdade quando envolvidos por um `main`.
//! - suíte consolidada da Fase 3 (PRD.md, T44): `import data` (a forma de
//!   topo sem alias/string) virou caminho feliz — saiu da tabela de fora de
//!   escopo, mesmo movimento já feito para `indexacao_de_array` etc. na
//!   T30/T31; e o que segue fora de escopo depois de `import`/capabilities
//!   serem aceitos — capability inexistente, membro inexistente em módulo ou
//!   em tipo opaco, opaco usado como record, módulo usado como valor ou
//!   atribuído, alias de `import` colidindo com nome já declarado e
//!   `import` como expressão — é rejeitado com erro claro, sem pagar o
//!   build do Polars (`--emit-rust` em todo caso negativo). Desde a T72 o
//!   alias (`import data as d`) e o método com dois-pontos
//!   (`df:soma(...)`) são formas **aceitas**, não rejeições;
//! - a prova ponta a ponta da Fase 3 (PRD.md, T45): compila e executa
//!   `examples/dados.titan` — único caminho feliz desta suíte que paga o
//!   build do Polars de propósito — conferindo stdout completo e exit code.
//! - curadoria da Fase 4 (PRD.md, T57): `CASOS_FORA_DE_ESCOPO_FASE_4` fecha
//!   com tipos soma (`enum`/`match`, que ganharam sintaxe na T75 e agora são
//!   rejeitados pelo **checker**, à espera da tipagem da T76),
//!   `.titan` importando `.titan` (mesma rejeição de `import` com string da
//!   T35) e `s[i]` (indexação de string, branch própria no checker); e o
//!   risco 5 (Cargo.toml gerado nunca depender do LSP) é conferido dentro do
//!   build de `hello.titan` já pago pelo caminho feliz, sem custo extra.
//! - abertura da Fase 5 (PRD.md, T59): as tabelas de fora-de-escopo mudam de
//!   camada onde o léxico abriu e `KEYWORDS_NOVAS_DA_T59` registra a quebra
//!   compatível das palavras-chave novas — as sete da T59 mais `with`, que
//!   a sintaxe do `match` exigiu na T75;
//! - tipos opcionais (PRD.md, T68/T69): `integer?` saiu de
//!   `CASOS_FORA_DE_ESCOPO_FASE_2` — parser e checker o aceitam, e a T69
//!   fechou a emissão, então o caso virou caminho feliz em
//!   `compila_e_executa_tipos_opcionais`;
//! - bitwise e `//` completos (PRD.md, T61): `& | ~ << >> //` percorreram
//!   lexer (T59), parser (T60) e agora checker/codegen — o caminho feliz é
//!   provado por execução real em
//!   `compila_e_executa_bitwise_e_divisao_inteira`, com `-7 // 2` dando -4
//!   (piso, não truncagem).

use std::path::{Path, PathBuf};
use std::process::Command;

fn titanc_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_titanc"))
}

fn examples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples")
}

/// Raiz do workspace — `dados.titan` lê `examples/vendas.csv` por caminho
/// relativo a ela, então o executável gerado precisa rodar com este
/// diretório como cwd (T45).
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Diretório temporário isolado por teste, para não colidir `build/` entre
/// execuções paralelas do `cargo test`.
fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "titanc-integration-{label}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("cria diretório temporário de teste");
    dir
}

fn write_source(dir: &Path, filename: &str, contents: &str) -> PathBuf {
    let path = dir.join(filename);
    std::fs::write(&path, contents).expect("escreve fonte .titan de teste");
    path
}

/// Nunca deve aparecer na saída do `titanc`, em nenhum cenário — panic
/// significa que uma etapa do pipeline não tratou o erro como `Result`.
fn assert_never_panics(output: &std::process::Output) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("panicked"),
        "titanc entrou em pânico (stderr): {stderr}"
    );
}

#[test]
fn compila_e_executa_hello_titan_conferindo_stdout_e_exit_code() {
    let out_dir = temp_dir("hello");

    let compile_output = Command::new(titanc_bin())
        .arg("--out")
        .arg(&out_dir)
        .arg(examples_dir().join("hello.titan"))
        .output()
        .expect("invoca titanc");
    assert_never_panics(&compile_output);
    assert!(
        compile_output.status.success(),
        "titanc falhou ao compilar hello.titan: {}",
        String::from_utf8_lossy(&compile_output.stderr)
    );

    let binary = out_dir.join("hello");
    assert!(binary.exists(), "esperava executável em {binary:?}");

    let run_output = Command::new(&binary).output().expect("executa ./hello");
    assert_eq!(String::from_utf8_lossy(&run_output.stdout), "Olá, mundo!\n");
    assert_eq!(run_output.status.code(), Some(0));

    // Risco 5 da Fase 4 (PRD.md, T57): `hello.titan` não tem `import`, então
    // `collect_deps` (driver.rs) só deveria listar `titan-runtime`. Reusa o
    // build já pago acima em vez de compilar de novo só para checar isso.
    let cargo_toml =
        std::fs::read_to_string(out_dir.join("build").join("hello").join("Cargo.toml"))
            .expect("lê o Cargo.toml gerado");
    assert!(
        cargo_toml.contains("titan-runtime"),
        "Cargo.toml gerado deveria depender de titan-runtime:\n{cargo_toml}"
    );
    assert!(
        !cargo_toml.contains("titan-lsp"),
        "Cargo.toml gerado não deveria depender do LSP:\n{cargo_toml}"
    );
    // Decisão técnica 9 da Fase 5 (PRD.md, T79): o crate `toml`, que lê o
    // manifesto, é dep do workspace do compilador e nunca do programa
    // gerado — mesma disciplina que o ADR 0019 impôs às deps do LSP.
    assert!(
        !cargo_toml.contains("toml = "),
        "Cargo.toml gerado não deveria depender do crate toml:\n{cargo_toml}"
    );

    let _ = std::fs::remove_dir_all(&out_dir);
}

#[test]
fn compila_e_executa_nucleo_titan_conferindo_stdout_e_exit_code() {
    let out_dir = temp_dir("nucleo");

    let compile_output = Command::new(titanc_bin())
        .arg("--out")
        .arg(&out_dir)
        .arg(examples_dir().join("nucleo.titan"))
        .output()
        .expect("invoca titanc");
    assert_never_panics(&compile_output);
    assert!(
        compile_output.status.success(),
        "titanc falhou ao compilar nucleo.titan: {}",
        String::from_utf8_lossy(&compile_output.stderr)
    );

    let binary = out_dir.join("nucleo");
    assert!(binary.exists(), "esperava executável em {binary:?}");

    let run_output = Command::new(&binary).output().expect("executa ./nucleo");
    assert_eq!(
        String::from_utf8_lossy(&run_output.stdout),
        "Fatorial de 5: 120\nFibonacci de 10: 55\n"
    );
    assert_eq!(run_output.status.code(), Some(0));

    let _ = std::fs::remove_dir_all(&out_dir);
}

/// Caminho feliz da Fase 2 (PRD.md, T32): compila `examples/compostos.titan`
/// — record (construção por contexto, leitura e escrita de campo), array
/// (literal, `#`, indexação, mutação in-place por função, push via
/// `#res+1`), array de floats e map — e confere stdout completo e exit code.
/// As duas linhas mais importantes provam as decisões da fase: "Original
/// preservado" prova a semântica de valor (decisão 1, `local copia = qs;
/// copia[1] = 999` não altera `qs`); "Primeiro estoque dobrado" prova
/// parâmetros compostos por `&mut` (decisão 4, `dobrar_estoque(qs)` muda o
/// que o chamador vê).
#[test]
fn compila_e_executa_compostos_titan_conferindo_stdout_e_exit_code() {
    let out_dir = temp_dir("compostos");

    let compile_output = Command::new(titanc_bin())
        .arg("--out")
        .arg(&out_dir)
        .arg(examples_dir().join("compostos.titan"))
        .output()
        .expect("invoca titanc");
    assert_never_panics(&compile_output);
    assert!(
        compile_output.status.success(),
        "titanc falhou ao compilar compostos.titan: {}",
        String::from_utf8_lossy(&compile_output.stderr)
    );

    let binary = out_dir.join("compostos");
    assert!(binary.exists(), "esperava executável em {binary:?}");

    let run_output = Command::new(&binary).output().expect("executa ./compostos");
    let esperado = "Estoque: Parafuso x10\n\
                    Apos reposicao: 15\n\
                    Original preservado: 5\n\
                    Ordenado: 1,2,3,4,5\n\
                    Res tamanho: 5\n\
                    Res ultimo: 50\n\
                    Soma pesos: 6.75\n\
                    Primeiro estoque: Parafuso x10\n\
                    Primeiro estoque dobrado: 20\n\
                    Segundo estoque dobrado: 40\n\
                    Preco parafuso: 0.5\n\
                    Preco arruela: 0.1\n";
    assert_eq!(String::from_utf8_lossy(&run_output.stdout), esperado);
    assert_eq!(run_output.status.code(), Some(0));

    let _ = std::fs::remove_dir_all(&out_dir);
}

/// A prova da Fase 3 (PRD.md, T45): compila `examples/dados.titan` —
/// `import data`, leitura de `examples/vendas.csv`, dimensões (`linhas`/
/// `colunas`), extração de uma coluna como array Titan (soma via `for`,
/// exercitando a Fase 2 sobre o resultado) e as quatro agregações, incluindo
/// `soma` **pelas duas formas** (`data.soma(df, "valor")` e
/// `df.soma("valor")`) — e confere stdout completo e exit code. O binário
/// gerado roda com `workspace_root()` como cwd porque `dados.titan` lê o CSV
/// por caminho relativo à raiz do projeto.
#[test]
fn compila_e_executa_dados_titan_conferindo_stdout_e_exit_code() {
    let out_dir = temp_dir("dados");

    let compile_output = Command::new(titanc_bin())
        .arg("--out")
        .arg(&out_dir)
        .arg(examples_dir().join("dados.titan"))
        .output()
        .expect("invoca titanc");
    assert_never_panics(&compile_output);
    assert!(
        compile_output.status.success(),
        "titanc falhou ao compilar dados.titan: {}",
        String::from_utf8_lossy(&compile_output.stderr)
    );

    let binary = out_dir.join("dados");
    assert!(binary.exists(), "esperava executável em {binary:?}");

    let run_output = Command::new(&binary)
        .current_dir(workspace_root())
        .output()
        .expect("executa ./dados");
    let esperado = "Linhas: 4\n\
                    Colunas: produto,quantidade,valor\n\
                    Total de unidades (array Titan): 360\n\
                    Soma do valor (data.soma): 1250.74\n\
                    Soma do valor (df.soma): 1250.74\n\
                    Media do valor: 312.685\n\
                    Minimo do valor: 20\n\
                    Maximo do valor: 999.99\n";
    assert_eq!(String::from_utf8_lossy(&run_output.stdout), esperado);
    assert_eq!(run_output.status.code(), Some(0));

    let _ = std::fs::remove_dir_all(&out_dir);
}

/// A prova da Fase 4 (PRD.md, T56): compila e executa
/// `examples/lexer.titan` — o lexer do Titan escrito em Titan, self-hosting
/// desta fase — apontando para `examples/nucleo.titan` como entrada e
/// conferindo **stdout completo e exit code**, no molde de
/// `compila_e_executa_dados_titan_conferindo_stdout_e_exit_code`. `args[1]`
/// chega pelo shim de `main` (`codegen.rs:113-119`), e a leitura do arquivo
/// usa `io.ler_arquivo` (T54) sobre bytes indexados por `texto` (T53).
#[test]
fn compila_e_executa_lexer_titan_sobre_nucleo_titan_conferindo_stdout_e_exit_code() {
    let out_dir = temp_dir("lexer");

    let compile_output = Command::new(titanc_bin())
        .arg("--out")
        .arg(&out_dir)
        .arg(examples_dir().join("lexer.titan"))
        .output()
        .expect("invoca titanc");
    assert_never_panics(&compile_output);
    assert!(
        compile_output.status.success(),
        "titanc falhou ao compilar lexer.titan: {}",
        String::from_utf8_lossy(&compile_output.stderr)
    );

    let binary = out_dir.join("lexer");
    assert!(binary.exists(), "esperava executável em {binary:?}");

    let run_output = Command::new(&binary)
        .arg(examples_dir().join("nucleo.titan"))
        .output()
        .expect("executa ./lexer examples/nucleo.titan");
    let esperado = "PALAVRA_CHAVE 'function' 1:1\n\
                    NOME 'fatorial' 1:10\n\
                    SIMBOLO '(' 1:18\n\
                    NOME 'n' 1:19\n\
                    SIMBOLO ':' 1:20\n\
                    PALAVRA_CHAVE 'integer' 1:22\n\
                    SIMBOLO ')' 1:29\n\
                    SIMBOLO ':' 1:30\n\
                    PALAVRA_CHAVE 'integer' 1:32\n\
                    PALAVRA_CHAVE 'if' 2:5\n\
                    NOME 'n' 2:8\n\
                    SIMBOLO '<=' 2:10\n\
                    INTEIRO '1' 2:13\n\
                    PALAVRA_CHAVE 'then' 2:15\n\
                    PALAVRA_CHAVE 'return' 3:9\n\
                    INTEIRO '1' 3:16\n\
                    PALAVRA_CHAVE 'end' 4:5\n\
                    PALAVRA_CHAVE 'local' 5:5\n\
                    NOME 'resultado' 5:11\n\
                    SIMBOLO ':' 5:20\n\
                    PALAVRA_CHAVE 'integer' 5:22\n\
                    SIMBOLO '=' 5:30\n\
                    INTEIRO '1' 5:32\n\
                    PALAVRA_CHAVE 'local' 6:5\n\
                    NOME 'i' 6:11\n\
                    SIMBOLO ':' 6:12\n\
                    PALAVRA_CHAVE 'integer' 6:14\n\
                    SIMBOLO '=' 6:22\n\
                    INTEIRO '2' 6:24\n\
                    PALAVRA_CHAVE 'while' 7:5\n\
                    NOME 'i' 7:11\n\
                    SIMBOLO '<=' 7:13\n\
                    NOME 'n' 7:16\n\
                    PALAVRA_CHAVE 'do' 7:18\n\
                    NOME 'resultado' 8:9\n\
                    SIMBOLO '=' 8:19\n\
                    NOME 'resultado' 8:21\n\
                    SIMBOLO '*' 8:31\n\
                    NOME 'i' 8:33\n\
                    NOME 'i' 9:9\n\
                    SIMBOLO '=' 9:11\n\
                    NOME 'i' 9:13\n\
                    SIMBOLO '+' 9:15\n\
                    INTEIRO '1' 9:17\n\
                    PALAVRA_CHAVE 'end' 10:5\n\
                    PALAVRA_CHAVE 'return' 11:5\n\
                    NOME 'resultado' 11:12\n\
                    PALAVRA_CHAVE 'end' 12:1\n\
                    PALAVRA_CHAVE 'function' 14:1\n\
                    NOME 'fibonacci' 14:10\n\
                    SIMBOLO '(' 14:19\n\
                    NOME 'n' 14:20\n\
                    SIMBOLO ':' 14:21\n\
                    PALAVRA_CHAVE 'integer' 14:23\n\
                    SIMBOLO ')' 14:30\n\
                    SIMBOLO ':' 14:31\n\
                    PALAVRA_CHAVE 'integer' 14:33\n\
                    PALAVRA_CHAVE 'if' 15:5\n\
                    NOME 'n' 15:8\n\
                    SIMBOLO '<=' 15:10\n\
                    INTEIRO '1' 15:13\n\
                    PALAVRA_CHAVE 'then' 15:15\n\
                    PALAVRA_CHAVE 'return' 16:9\n\
                    NOME 'n' 16:16\n\
                    PALAVRA_CHAVE 'end' 17:5\n\
                    PALAVRA_CHAVE 'local' 18:5\n\
                    NOME 'a' 18:11\n\
                    SIMBOLO ':' 18:12\n\
                    PALAVRA_CHAVE 'integer' 18:14\n\
                    SIMBOLO '=' 18:22\n\
                    INTEIRO '0' 18:24\n\
                    PALAVRA_CHAVE 'local' 19:5\n\
                    NOME 'b' 19:11\n\
                    SIMBOLO ':' 19:12\n\
                    PALAVRA_CHAVE 'integer' 19:14\n\
                    SIMBOLO '=' 19:22\n\
                    INTEIRO '1' 19:24\n\
                    PALAVRA_CHAVE 'for' 20:5\n\
                    NOME 'j' 20:9\n\
                    SIMBOLO '=' 20:11\n\
                    INTEIRO '2' 20:13\n\
                    SIMBOLO ',' 20:14\n\
                    NOME 'n' 20:16\n\
                    PALAVRA_CHAVE 'do' 20:18\n\
                    PALAVRA_CHAVE 'local' 21:9\n\
                    NOME 'prox' 21:15\n\
                    SIMBOLO ':' 21:19\n\
                    PALAVRA_CHAVE 'integer' 21:21\n\
                    SIMBOLO '=' 21:29\n\
                    NOME 'a' 21:31\n\
                    SIMBOLO '+' 21:33\n\
                    NOME 'b' 21:35\n\
                    NOME 'a' 22:9\n\
                    SIMBOLO '=' 22:11\n\
                    NOME 'b' 22:13\n\
                    NOME 'b' 23:9\n\
                    SIMBOLO '=' 23:11\n\
                    NOME 'prox' 23:13\n\
                    PALAVRA_CHAVE 'end' 24:5\n\
                    PALAVRA_CHAVE 'return' 25:5\n\
                    NOME 'b' 25:12\n\
                    PALAVRA_CHAVE 'end' 26:1\n\
                    PALAVRA_CHAVE 'function' 28:1\n\
                    NOME 'main' 28:10\n\
                    SIMBOLO '(' 28:14\n\
                    NOME 'args' 28:15\n\
                    SIMBOLO ':' 28:19\n\
                    SIMBOLO '{' 28:21\n\
                    PALAVRA_CHAVE 'string' 28:22\n\
                    SIMBOLO '}' 28:28\n\
                    SIMBOLO ')' 28:29\n\
                    SIMBOLO ':' 28:30\n\
                    PALAVRA_CHAVE 'integer' 28:32\n\
                    NOME 'print' 29:5\n\
                    SIMBOLO '(' 29:10\n\
                    STRING 'Fatorial de 5: ' 29:11\n\
                    SIMBOLO '..' 29:29\n\
                    NOME 'fatorial' 29:32\n\
                    SIMBOLO '(' 29:40\n\
                    INTEIRO '5' 29:41\n\
                    SIMBOLO ')' 29:42\n\
                    SIMBOLO ')' 29:43\n\
                    NOME 'print' 30:5\n\
                    SIMBOLO '(' 30:10\n\
                    STRING 'Fibonacci de 10: ' 30:11\n\
                    SIMBOLO '..' 30:31\n\
                    NOME 'fibonacci' 30:34\n\
                    SIMBOLO '(' 30:43\n\
                    INTEIRO '10' 30:44\n\
                    SIMBOLO ')' 30:46\n\
                    SIMBOLO ')' 30:47\n\
                    PALAVRA_CHAVE 'return' 31:5\n\
                    INTEIRO '0' 31:12\n\
                    PALAVRA_CHAVE 'end' 32:1\n\
                    EOF '' 33:1\n";
    assert_eq!(String::from_utf8_lossy(&run_output.stdout), esperado);
    assert_eq!(run_output.status.code(), Some(0));

    let _ = std::fs::remove_dir_all(&out_dir);
}

/// A verificação do T17 pede o `--emit-rust` do `nucleo.titan` "sem warnings
/// de mut" — a decisão 6 da Fase 1 exige `let mut` apenas nas variáveis
/// reatribuídas. Conferimos as duas direções: quem é reatribuída
/// (`resultado`, `i`, `a`, `b`) sai `mut`; quem não é (`prox`) sai sem.
#[test]
fn emit_rust_de_nucleo_marca_mut_apenas_nas_variaveis_reatribuidas() {
    let out_dir = temp_dir("emit-rust-nucleo");

    let output = Command::new(titanc_bin())
        .arg("--emit-rust")
        .arg("--out")
        .arg(&out_dir)
        .arg(examples_dir().join("nucleo.titan"))
        .output()
        .expect("invoca titanc --emit-rust");
    assert_never_panics(&output);
    assert!(
        output.status.success(),
        "titanc --emit-rust falhou para nucleo.titan: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    for reatribuida in ["resultado", "i", "a", "b"] {
        assert!(
            stdout.contains(&format!("let mut {reatribuida}: i64")),
            "esperava `let mut {reatribuida}` no Rust gerado:\n{stdout}"
        );
    }
    assert!(
        stdout.contains("let prox: i64"),
        "`prox` nunca é reatribuída — não deveria ser `mut`:\n{stdout}"
    );

    let _ = std::fs::remove_dir_all(&out_dir);
}

#[test]
fn emit_rust_imprime_o_rust_gerado_sem_compilar() {
    let out_dir = temp_dir("emit-rust");

    let output = Command::new(titanc_bin())
        .arg("--emit-rust")
        .arg("--out")
        .arg(&out_dir)
        .arg(examples_dir().join("hello.titan"))
        .output()
        .expect("invoca titanc --emit-rust");
    assert_never_panics(&output);
    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    // `args` não é lido no corpo de `hello.titan` — sai `_args` para o Rust
    // gerado não emitir `unused_variables`.
    assert!(stdout.contains("fn titan_main(_args: &mut Vec<String>) -> i64"));
    assert!(stdout.contains("titan_runtime::print(&\"Olá, mundo!\".to_string());"));
    assert!(
        !out_dir.join("build").exists(),
        "--emit-rust não deveria gerar build/"
    );

    let _ = std::fs::remove_dir_all(&out_dir);
}

/// O `_` inalcançável é o primeiro diagnóstico do checker que **não**
/// impede a compilação (T76): sai como aviso em stderr, e não como erro.
///
/// Desde a T77 o programa segue até o fim e o Rust sai — o que fecha o
/// argumento que a T76 só pôde deixar em aberto: o aviso não barra nada.
#[test]
fn curinga_inalcancavel_sai_como_aviso_e_nao_como_erro() {
    let out_dir = temp_dir("aviso-curinga");
    let fonte = write_source(
        &out_dir,
        "cor.titan",
        "enum Cor\n\
         \x20   Vermelho\n\
         \x20   Verde\n\
         end\n\
         \n\
         function main(args: {string}): integer\n\
         \x20   local c: Cor = Vermelho\n\
         \x20   match c with\n\
         \x20       Vermelho then\n\
         \x20           return 0\n\
         \x20       Verde then\n\
         \x20           return 1\n\
         \x20       _ then\n\
         \x20           return 2\n\
         \x20   end\n\
         \x20   return 0\n\
         end\n",
    );

    let output = Command::new(titanc_bin())
        .arg("--emit-rust")
        .arg("--out")
        .arg(&out_dir)
        .arg(&fonte)
        .output()
        .expect("invoca titanc");
    assert_never_panics(&output);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("aviso") && stderr.contains("nunca é alcançado"),
        "esperava o aviso do `_` inalcançável em stderr, obteve: {stderr}"
    );
    assert!(
        !stderr.contains("erro de tipo"),
        "o `_` inalcançável não é erro de tipo: {stderr}"
    );
    assert!(
        output.status.success(),
        "o aviso não deveria barrar a compilação: {stderr}"
    );

    let _ = std::fs::remove_dir_all(&out_dir);
}

/// O critério de aceite da T77 visto de fora, pelo binário: um `.titan` com
/// tipo soma recursivo atravessa o pipeline inteiro e sai como Rust.
///
/// Usa `--emit-rust` (e não o build completo) pelo mesmo motivo dos casos de
/// array/record da T30: o que importa aqui é o backend produzir o `enum`, o
/// `Box` e o `match`, e a execução de verdade já é coberta pelos testes de
/// `codegen.rs`, que compilam o gerado com o `rustc` e conferem a saída.
#[test]
fn emit_rust_de_enum_recursivo_sai_com_box_e_match() {
    let out_dir = temp_dir("emit-rust-enum-recursivo");
    let fonte = write_source(
        &out_dir,
        "exp.titan",
        "enum Exp\n\
         \x20   ExpInteger(integer)\n\
         \x20   ExpSoma(Exp, Exp)\n\
         end\n\
         \n\
         function avalia(e: Exp): integer\n\
         \x20   local r: integer = match e with\n\
         \x20       ExpInteger(n) then\n\
         \x20           n\n\
         \x20       ExpSoma(l, d) then\n\
         \x20           avalia(l) + avalia(d)\n\
         \x20   end\n\
         \x20   return r\n\
         end\n\
         \n\
         function main(args: {string}): integer\n\
         \x20   print(\"soma: \" .. avalia(ExpSoma(ExpInteger(1), ExpInteger(2))))\n\
         \x20   return 0\n\
         end\n",
    );

    let output = Command::new(titanc_bin())
        .arg("--emit-rust")
        .arg("--out")
        .arg(&out_dir)
        .arg(&fonte)
        .output()
        .expect("invoca titanc --emit-rust");
    assert_never_panics(&output);
    assert!(
        output.status.success(),
        "titanc --emit-rust falhou para `enum` recursivo: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("pub enum Exp {"), "gerado: {stdout}");
    // O `Box` dos dois campos recursivos — a armadilha central da fase.
    assert!(
        stdout.contains("ExpSoma(Box<Exp>, Box<Exp>)"),
        "gerado: {stdout}"
    );
    assert!(stdout.contains("match &e {"), "gerado: {stdout}");

    let _ = std::fs::remove_dir_all(&out_dir);
}

/// `import_de_modulo` saiu da tabela de fora-de-escopo na T44: desde a T35 a
/// forma de topo `import data` (sem alias, sem string) é aceita pelo parser e
/// resolvida pelo checker (T38). Usa `--emit-rust` para não pagar o build do
/// Polars (risco 1) — o precedente é o mesmo movimento já feito na T30/T31
/// para `indexacao_de_array`/`construtor_de_array`/`operador_length`.
#[test]
fn emit_rust_de_import_data_compila_sem_erro() {
    let out_dir = temp_dir("emit-rust-import-data");

    let source = "import data\n\nfunction main(args: {string}): integer\n    return 0\nend";
    let source_path = write_source(&out_dir, "caso.titan", source);

    let output = Command::new(titanc_bin())
        .arg("--emit-rust")
        .arg("--out")
        .arg(&out_dir)
        .arg(&source_path)
        .output()
        .expect("invoca titanc --emit-rust");
    assert_never_panics(&output);
    assert!(
        output.status.success(),
        "titanc --emit-rust falhou para `import data`: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !out_dir.join("build").exists(),
        "--emit-rust não deveria gerar build/"
    );

    let _ = std::fs::remove_dir_all(&out_dir);
}

/// Mesmo precedente de `emit_rust_de_import_data_compila_sem_erro`, para o
/// módulo `texto` (T53): usa `--emit-rust` para não pagar o build do crate
/// real e confirma que `import texto` mais uma chamada de cada função de
/// módulo resolve e gera Rust sem erro.
#[test]
fn emit_rust_de_import_texto_compila_sem_erro() {
    let out_dir = temp_dir("emit-rust-import-texto");

    let source = concat!(
        "import texto\n\n",
        "function main(args: {string}): integer\n",
        "    local b: integer = texto.byte(\"abc\", 1)\n",
        "    local s: string = texto.sub(\"abc\", 1, 2)\n",
        "    local n: integer = texto.para_inteiro(\"42\")\n",
        "    local t: string = texto.de_inteiro(n)\n",
        "    local tam: integer = texto.tamanho(s)\n",
        "    return 0\n",
        "end",
    );
    let source_path = write_source(&out_dir, "caso.titan", source);

    let output = Command::new(titanc_bin())
        .arg("--emit-rust")
        .arg("--out")
        .arg(&out_dir)
        .arg(&source_path)
        .output()
        .expect("invoca titanc --emit-rust");
    assert_never_panics(&output);
    assert!(
        output.status.success(),
        "titanc --emit-rust falhou para `import texto`: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("titan_texto::byte("), "stdout: {stdout}");
    assert!(stdout.contains("titan_texto::sub("), "stdout: {stdout}");
    assert!(
        stdout.contains("titan_texto::para_inteiro("),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains("titan_texto::de_inteiro("),
        "stdout: {stdout}"
    );
    assert!(stdout.contains("titan_texto::tamanho("), "stdout: {stdout}");
    assert!(
        !out_dir.join("build").exists(),
        "--emit-rust não deveria gerar build/"
    );

    let _ = std::fs::remove_dir_all(&out_dir);
}

/// Mesmo precedente de `emit_rust_de_import_texto_compila_sem_erro`, para o
/// módulo `io` (T54): usa `--emit-rust` para não pagar o build real e
/// confirma que `import io` mais uma chamada de cada função de módulo
/// resolve e gera Rust sem erro.
#[test]
fn emit_rust_de_import_io_compila_sem_erro() {
    let out_dir = temp_dir("emit-rust-import-io");

    let source = concat!(
        "import io\n\n",
        "function main(args: {string}): integer\n",
        "    local conteudo: string = io.ler_arquivo(\"caso.titan\")\n",
        "    io.escrever_arquivo(\"saida.txt\", conteudo)\n",
        "    return 0\n",
        "end",
    );
    let source_path = write_source(&out_dir, "caso.titan", source);

    let output = Command::new(titanc_bin())
        .arg("--emit-rust")
        .arg("--out")
        .arg(&out_dir)
        .arg(&source_path)
        .output()
        .expect("invoca titanc --emit-rust");
    assert_never_panics(&output);
    assert!(
        output.status.success(),
        "titanc --emit-rust falhou para `import io`: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("titan_io::ler_arquivo("),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains("titan_io::escrever_arquivo("),
        "stdout: {stdout}"
    );
    assert!(
        !out_dir.join("build").exists(),
        "--emit-rust não deveria gerar build/"
    );

    let _ = std::fs::remove_dir_all(&out_dir);
}

/// Critério de aceite da T54 (execução real): um `.titan` que lê um arquivo
/// e imprime seu tamanho compila e roda. Lê o próprio fonte gerado
/// (`caso.titan`) por caminho relativo ao cwd do executável — igual ao
/// precedente de `dados.titan` (T45), que também lê por caminho relativo.
#[test]
fn compila_e_executa_import_io_lendo_arquivo_e_imprimindo_tamanho() {
    let out_dir = temp_dir("run-import-io");

    let conteudo_arquivo = "abcde";
    let arquivo_lido = write_source(&out_dir, "entrada.txt", conteudo_arquivo);

    let source = concat!(
        "import io\n",
        "import texto\n\n",
        "function main(args: {string}): integer\n",
        "    local conteudo: string = io.ler_arquivo(\"entrada.txt\")\n",
        "    local tam: integer = texto.tamanho(conteudo)\n",
        "    print(texto.de_inteiro(tam))\n",
        "    return 0\n",
        "end",
    );
    let source_path = write_source(&out_dir, "caso.titan", source);

    let compile_output = Command::new(titanc_bin())
        .arg("--out")
        .arg(&out_dir)
        .arg(&source_path)
        .output()
        .expect("invoca titanc");
    assert_never_panics(&compile_output);
    assert!(
        compile_output.status.success(),
        "titanc falhou ao compilar `import io`: {}",
        String::from_utf8_lossy(&compile_output.stderr)
    );

    let binary = out_dir.join("caso");
    assert!(binary.exists(), "esperava executável em {binary:?}");

    let run_output = Command::new(&binary)
        .current_dir(&out_dir)
        .output()
        .expect("executa ./caso");
    assert_eq!(
        String::from_utf8_lossy(&run_output.stdout),
        format!("{}\n", conteudo_arquivo.len())
    );
    assert_eq!(run_output.status.code(), Some(0));

    let _ = std::fs::remove_file(&arquivo_lido);
    let _ = std::fs::remove_dir_all(&out_dir);
}

/// Um caso negativo: fonte, trecho esperado na mensagem de erro em stderr.
struct CasoNegativo {
    nome: &'static str,
    fonte: &'static str,
    trecho_esperado: &'static str,
}

const CASOS_NEGATIVOS: &[CasoNegativo] = &[
    CasoNegativo {
        nome: "print_com_argumento_incompativel",
        fonte: "function main(args: {string}): integer\n    print(42)\n    return 0\nend",
        trecho_esperado: "incompatível",
    },
    CasoNegativo {
        nome: "chamada_a_funcao_inexistente",
        fonte: "function main(args: {string}): integer\n    funcao_inexistente()\n    return 0\nend",
        trecho_esperado: "não foi declarada",
    },
    CasoNegativo {
        nome: "main_retornando_tipo_incompativel",
        fonte: "function main(args: {string}): integer\n    return \"oi\"\nend",
        trecho_esperado: "retorno incompatível",
    },
    CasoNegativo {
        // O `if` passou a ser aceito na Fase 1 (T12); o caso negativo agora
        // exercita a atribuição a variável não declarada.
        nome: "atribuicao_sem_declarar",
        fonte: "function main(args: {string}): integer\n    x = 10\n    return 0\nend",
        trecho_esperado: "não foi declarado",
    },
    CasoNegativo {
        nome: "end_faltando",
        fonte: "function main(args: {string}): integer\n    return 0\n",
        trecho_esperado: "end",
    },
    CasoNegativo {
        nome: "assinatura_de_main_invalida",
        fonte: "function main(): integer\n    return 0\nend",
        trecho_esperado: "main",
    },
    CasoNegativo {
        nome: "string_nao_terminada",
        fonte: "function main(args: {string}): integer\n    print(\"sem fechar)\n    return 0\nend",
        trecho_esperado: "não terminada",
    },
];

/// Compila `caso.fonte` pelo binário real e confere a tripla que define um
/// "erro claro": falha sem panic, stderr com o trecho esperado, nenhum
/// `build/` deixado para trás.
fn verifica_caso_negativo(caso: &CasoNegativo, label: &str) {
    let out_dir = temp_dir(&format!("{label}-{}", caso.nome));
    let source_path = write_source(&out_dir, "caso.titan", caso.fonte);

    let output = Command::new(titanc_bin())
        .arg("--emit-rust")
        .arg("--out")
        .arg(&out_dir)
        .arg(&source_path)
        .output()
        .unwrap_or_else(|e| panic!("[{}] falha ao invocar titanc: {e}", caso.nome));

    assert_never_panics(&output);
    assert!(
        !output.status.success(),
        "[{}] esperava falha, titanc reportou sucesso",
        caso.nome
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.trim().is_empty(),
        "[{}] esperava mensagem de erro em stderr, veio vazio",
        caso.nome
    );
    assert!(
        stderr.contains(caso.trecho_esperado),
        "[{}] esperava stderr contendo '{}', obteve: {stderr}",
        caso.nome,
        caso.trecho_esperado
    );
    assert!(
        !out_dir.join("build").exists(),
        "[{}] erro não deveria deixar build/ para trás",
        caso.nome
    );

    let _ = std::fs::remove_dir_all(&out_dir);
}

#[test]
fn casos_negativos_de_t4_e_t5_produzem_erro_claro_sem_panic() {
    for caso in CASOS_NEGATIVOS {
        verifica_caso_negativo(caso, "negativo");
    }
}

/// Fora de escopo da Fase 2 (PRD.md, T31): a Fase 2 ensinou o pipeline a
/// aceitar arrays, records e maps — esta tabela garante que ela não afrouxou
/// nada além do pretendido. Cada construção que segue fora do subconjunto é
/// rejeitada em alguma etapa (léxica, sintática ou de tipos) com erro claro,
/// nunca panic.
///
/// `indexacao_de_array`, `construtor_de_array`, `operador_length` (T30/T31)
/// e os seis de bitwise/`//` (T61) saíram desta tabela por terem virado
/// caminho feliz — arrays e operadores têm suporte real no codegen.
/// `chamada_de_metodo` continua rejeitado, mas desceu de camada duas vezes:
/// com `.` e `[` lexados e o parser sabendo indexação, saiu do lexer para o
/// parser; e na T72, que ensinou o parser a ler `:` como chamada de método,
/// saiu do parser para o **checker**. O que segue fora de escopo não é a
/// sintaxe do `:` — é o método sobre um tipo **do usuário**: método só
/// existe sobre tipo opaco de capability (`df:soma(...)`), nunca sobre
/// record declarado no programa.
/// `tipo_option` saiu na T68 (ver o comentário no lugar dele).
/// `break_fora_de_escopo` saiu desta tabela na T55 (Fase 4): `break` é keyword e vira caso positivo
/// dentro de laço — os negativos de `break`/`continue` da T55 têm tabela
/// própria, [`CASOS_FORA_DE_ESCOPO_FASE_4`].
///
/// `retornos_multiplos` saiu desta tabela na T65 (Fase 5): a assinatura
/// `: integer, integer` e o `return a, b` passaram a ser sintaxe e tipagem
/// legítimas — mesmo movimento de `indexacao_de_array` (T30), `break` (T55),
/// bitwise (T61) e `repeat` (T64). O que restou da família são negativos de
/// **aridade** e de **tipo por posição**, cobertos nos testes de unidade do
/// checker.
const CASOS_FORA_DE_ESCOPO_FASE_2: &[CasoNegativo] = &[
    CasoNegativo {
        nome: "chamada_de_metodo_sobre_record_do_usuario",
        // Desde a T72 o parser lê `p:dist()` sem reclamar — quem rejeita é o
        // checker, ao ver que o receptor não tem `Type::Opaque`. Método do
        // usuário (`record` com `function Ponto:dist()`) segue fora de
        // escopo; o que existe é método de capability.
        fonte: "record Ponto\n    x: float\nend\n\nfunction main(args: {string}): integer\n    local p: Ponto = { x = 1.0 }\n    local d: float = p:dist()\n    return 0\nend",
        trecho_esperado: "só é possível chamar um nome de função diretamente",
    },
    // `repeat_until` saiu desta tabela na T64: virou caso **positivo**, com
    // execução real em `compila_e_executa_repeat_until` — o mesmo movimento
    // que `break` fez na T55, bitwise na T61 e `continue` na T63. O que
    // sobrou da família é o negativo de sintaxe (`repeat` sem `until`) e o
    // de tipo (condição não-boolean), ambos na tabela da Fase 4.
    // Os seis casos de bitwise e `//` que ficavam aqui saíram da tabela na
    // T61: viraram casos **positivos**, com execução real em
    // `compila_e_executa_bitwise_e_divisao_inteira` — mesmo movimento que
    // `indexacao_de_array` fez na T30 e `break` na T55. O que sobrou da
    // família é o negativo de tipo, `bitwise_com_float`, logo abaixo: o
    // operador existe, o que não existe é coerção float→integer.
    CasoNegativo {
        nome: "bitwise_com_float",
        fonte: "function main(args: {string}): integer\n    local a = 1.5 & 2\n    return 0\nend",
        trecho_esperado: "operando de `&` precisa ser integer",
    },
    // `tipo_option` saiu desta tabela na T68, pelo mesmo movimento que
    // `indexacao_de_array` fez na T30: `integer?` deixou de ser rejeitado
    // em qualquer camada de front-end — o parser lê o sufixo `?`, o checker
    // tipa o `Option` e estreita `if x ~= nil then`. A T69 fechou a
    // **emissão**, e o caso virou caminho feliz em
    // `compila_e_executa_tipos_opcionais`: compila e executa de verdade.
    // `cast_as` saiu desta tabela na T70, pelo mesmo movimento que
    // `indexacao_de_array` fez na T30 e `bitwise` na T61: `1 as float` deixou
    // de ser erro de sintaxe e virou caminho feliz. O que sobrou da família é
    // o negativo abaixo — cast **não é parsing**, e é essa fronteira que
    // continua valendo.
    CasoNegativo {
        nome: "cast_de_string_para_numero",
        fonte: "function main(args: {string}): integer\n    local a = \"3\" as integer\n    return 0\nend",
        trecho_esperado: "não existe cast de string para integer",
    },
    CasoNegativo {
        // Mesmo movimento de `chamada_de_metodo_sobre_record_do_usuario`: a
        // T72 fez o parser aceitar `:`, então a rejeição passou a ser do
        // checker — aqui sobre um composto (`{string}`), que também não tem
        // métodos.
        nome: "metodo_com_dois_pontos_sobre_composto",
        fonte: "function main(args: {string}): integer\n    args:foo()\n    return 0\nend",
        trecho_esperado: "só é possível chamar um nome de função diretamente",
    },
    // `multi_assign` saiu desta tabela na T67: `a, b = b, a` é caminho
    // feliz, provado por execução real em
    // `compila_e_executa_multi_assign_e_declaracao_multipla`. O que resta
    // do assunto é a aridade — abaixo.
    CasoNegativo {
        nome: "multi_assign_aridade",
        fonte: "function main(args: {string}): integer\n    local a: integer = 1\n    local b: integer = 2\n    a, b = 1\n    return 0\nend",
        trecho_esperado: "atribuição múltipla com 2 alvo(s), mas 1 valor(es)",
    },
    CasoNegativo {
        nome: "nome_de_record_reservado",
        fonte: "record String\n    x: integer\nend\n\nfunction main(args: {string}): integer\n    return 0\nend",
        trecho_esperado: "nome reservado do Rust",
    },
    CasoNegativo {
        nome: "record_construtor_incompleto",
        fonte: "record Ponto\n    x: integer\n    y: integer\nend\n\nfunction main(args: {string}): integer\n    local p: Ponto = {x = 1}\n    return 0\nend",
        trecho_esperado: "falta o campo",
    },
    CasoNegativo {
        nome: "record_campo_extra",
        fonte: "record Ponto\n    x: integer\n    y: integer\nend\n\nfunction main(args: {string}): integer\n    local p: Ponto = {x = 1, y = 2, z = 3}\n    return 0\nend",
        trecho_esperado: "não existe no record",
    },
    CasoNegativo {
        nome: "map_com_chave_float",
        fonte: "function main(args: {string}): integer\n    local m: {float: integer} = {}\n    return 0\nend",
        trecho_esperado: "chave de `map` precisa ser",
    },
    CasoNegativo {
        nome: "duplo_emprestimo",
        fonte: "function soma(xs: {integer}, ys: {integer}): integer\n    return xs[1] + ys[1]\nend\n\nfunction main(args: {string}): integer\n    local xs: {integer} = {1, 2}\n    local r: integer = soma(xs, xs)\n    return 0\nend",
        trecho_esperado: "empréstimo mutável duplicado",
    },
    CasoNegativo {
        nome: "length_de_map",
        fonte: "function main(args: {string}): integer\n    local m: {string: integer} = {}\n    local n: integer = #m\n    return 0\nend",
        trecho_esperado: "espera um array ou string",
    },
    CasoNegativo {
        nome: "record_recursivo",
        fonte: "record No\n    valor: integer\n    proximo: No\nend\n\nfunction main(args: {string}): integer\n    return 0\nend",
        trecho_esperado: "é recursivo",
    },
];

#[test]
fn construcoes_fora_de_escopo_da_fase_2_produzem_erro_claro_sem_panic() {
    for caso in CASOS_FORA_DE_ESCOPO_FASE_2 {
        verifica_caso_negativo(caso, "fora-de-escopo");
    }
}

/// Critério de aceite da T69, pelo pipeline completo e em **execução real**:
/// uma função que devolve `integer?`, um chamador que testa, e Rust gerado
/// **sem warnings**.
///
/// Este teste é a conversão para caminho feliz do antigo
/// `tipo_option_tipa_mas_ainda_nao_emite` (T68) — o mesmo movimento que a
/// T30 fez com `indexacao_de_array`: o que era rejeição de codegen virou
/// programa que compila e roda.
///
/// Os três pontos conferidos no texto emitido são os que a T69 introduz e
/// que nenhum teste de valor pegaria sozinho: `T?` vira `Option<T>`, o teste
/// de presença vira `is_some()`/`is_none()` (o mapeamento direto sairia
/// `x != ()`, que nem compila) e o ramo estreitado abre a ligação que
/// desembrulha o nome. O acumulador `acc` cobre a armadilha que a T68
/// registrou — atribuir dentro do ramo estreitado tem de alcançar a
/// variável de fora, e não a ligação nova —, e só a execução a prova: se o
/// write-back sumisse, `acc` sairia 3 em vez de 6, calado.
#[test]
fn compila_e_executa_tipos_opcionais() {
    let out_dir = temp_dir("tipos-opcionais-execucao-real");

    let source = concat!(
        "function busca(v: {integer}, alvo: integer): integer?\n",
        "    local i: integer = 1\n",
        "    while i <= #v do\n",
        "        if v[i] == alvo then\n",
        "            return i\n",
        "        end\n",
        "        i = i + 1\n",
        "    end\n",
        "    return nil\n",
        "end\n",
        "function main(args: {string}): integer\n",
        "    local v: {integer} = {10, 20, 30}\n",
        "    local achou: integer? = busca(v, 20)\n",
        "    if achou ~= nil then\n",
        "        print(\"achou-\" .. achou)\n",
        "    end\n",
        "    local nao: integer? = busca(v, 99)\n",
        "    if nao == nil then\n",
        "        print(\"nao-achou\")\n",
        "    end\n",
        // `string` dentro de `Option`: o valor de dentro sai dono, senão o
        // de fora sairia movido.
        "    local s: string? = \"oi\"\n",
        "    if s ~= nil then\n",
        "        print(\"str-\" .. s)\n",
        "    end\n",
        // Atribuição dentro do ramo estreitado, repetida num laço: cada
        // iteração precisa enxergar o que a anterior escreveu.
        "    local acc: integer? = 0\n",
        "    local i: integer = 1\n",
        "    while i <= 3 do\n",
        "        if acc ~= nil then\n",
        "            acc = acc + i\n",
        "        end\n",
        "        i = i + 1\n",
        "    end\n",
        "    if acc ~= nil then\n",
        "        print(\"acc-\" .. acc)\n",
        "    end\n",
        "    return 0\n",
        "end",
    );
    let source_path = write_source(&out_dir, "opcionais.titan", source);

    // 1. `--emit-rust`: `Option<T>`, o teste de presença e o desembrulho.
    let emit_output = Command::new(titanc_bin())
        .arg("--emit-rust")
        .arg(&source_path)
        .output()
        .expect("invoca titanc --emit-rust");
    assert_never_panics(&emit_output);
    assert!(
        emit_output.status.success(),
        "titanc --emit-rust falhou para tipos opcionais: {}",
        String::from_utf8_lossy(&emit_output.stderr)
    );
    let rust = String::from_utf8_lossy(&emit_output.stdout);
    assert!(
        rust.contains("pub fn titan_busca(v: &mut Vec<i64>, alvo: i64) -> Option<i64> {"),
        "`integer?` não virou `Option<i64>` na assinatura:\n{rust}"
    );
    assert!(
        rust.contains("return Some(i);") && rust.contains("return None;"),
        "valor e `nil` não viraram `Some`/`None`:\n{rust}"
    );
    assert!(
        rust.contains("if achou.is_some() {") && rust.contains("if nao.is_none() {"),
        "teste de presença não virou `is_some`/`is_none`:\n{rust}"
    );
    assert!(
        rust.contains("let achou: i64 = achou.clone().unwrap();"),
        "ramo estreitado não desembrulhou o nome:\n{rust}"
    );
    assert!(
        rust.contains("*titan_opt_acc = Some(acc);"),
        "atribuição no ramo estreitado não volta para a variável externa:\n{rust}"
    );

    // 2. Execução real do mesmo programa.
    let compile_output = Command::new(titanc_bin())
        .arg("--out")
        .arg(&out_dir)
        .arg(&source_path)
        .output()
        .expect("invoca titanc");
    assert_never_panics(&compile_output);
    assert!(
        compile_output.status.success(),
        "titanc falhou ao compilar o caso de tipos opcionais: {}",
        String::from_utf8_lossy(&compile_output.stderr)
    );

    let binary = out_dir.join("opcionais");
    assert!(binary.exists(), "esperava executável em {binary:?}");

    let run_output = Command::new(&binary).output().expect("executa ./opcionais");
    assert_eq!(
        String::from_utf8_lossy(&run_output.stdout),
        "achou-2\nnao-achou\nstr-oi\nacc-6\n"
    );
    assert_eq!(run_output.status.code(), Some(0));

    let _ = std::fs::remove_dir_all(&out_dir);
}

/// T68, o outro lado: usar um `T?` sem testar para no **checker**, com a
/// mensagem que ensina o teste — e o estreitamento não vale depois do `if`.
#[test]
fn usar_tipo_option_sem_testar_produz_erro_claro() {
    verifica_caso_negativo(
        &CasoNegativo {
            nome: "option_sem_testar",
            fonte: "function main(args: {string}): integer\n\
                 \x20   local x: integer? = 10\n\
                 \x20   if x ~= nil then\n\
                 \x20       print(\"dentro\")\n\
                 \x20   end\n\
                 \x20   local fora: integer = x\n\
                 \x20   return 0\n\
                 end",
            trecho_esperado: "pode ser nil",
        },
        "t68-option-sem-testar",
    );
}

/// Fora de escopo da Fase 3 (PRD.md, T44): a Fase 3 ensinou o pipeline a
/// aceitar `import data` e as duas formas de chamada de capability
/// (`data.f(...)` e `df.f(...)`) — esta tabela garante que ela não afrouxou
/// nada além do pretendido. A T72 acrescentou o alias (`import data as d`)
/// e a chamada com dois-pontos (`df:soma(...)`) ao que é aceito; o que
/// sobra aqui das duas formas são só as suas malformações. Todos os casos falham no checker ou no parser,
/// antes de o driver chegar a invocar `cargo build`, então nenhum deles paga
/// o build do Polars.
const CASOS_FORA_DE_ESCOPO_FASE_3: &[CasoNegativo] = &[
    CasoNegativo {
        nome: "capability_inexistente",
        fonte: "import inexistente\n\nfunction main(args: {string}): integer\n    return 0\nend",
        trecho_esperado: "capability 'inexistente' não existe",
    },
    CasoNegativo {
        nome: "funcao_inexistente_no_modulo",
        fonte: "import data\n\nfunction main(args: {string}): integer\n    local df: data.DataFrame = data.foo(\"v.csv\")\n    return 0\nend",
        trecho_esperado: "o módulo 'data' não tem função 'foo'",
    },
    CasoNegativo {
        nome: "metodo_inexistente_no_opaco",
        fonte: "import data\n\nfunction main(args: {string}): integer\n    local df: data.DataFrame = data.read_csv(\"v.csv\")\n    local total: float = df.foo(\"valor\")\n    return 0\nend",
        trecho_esperado: "o tipo 'data.DataFrame' não tem método 'foo'",
    },
    CasoNegativo {
        nome: "acesso_a_campo_de_opaco",
        fonte: "import data\n\nfunction main(args: {string}): integer\n    local df: data.DataFrame = data.read_csv(\"v.csv\")\n    local x = df.campo\n    return 0\nend",
        trecho_esperado: "não tem campos acessíveis",
    },
    CasoNegativo {
        nome: "modulo_usado_como_valor",
        fonte: "import data\n\nfunction main(args: {string}): integer\n    local x = data\n    return 0\nend",
        trecho_esperado: "'data' é um módulo, não um valor",
    },
    CasoNegativo {
        nome: "atribuicao_a_modulo",
        fonte: "import data\n\nfunction main(args: {string}): integer\n    data = 1\n    return 0\nend",
        trecho_esperado: "não é possível atribuir ao módulo 'data'",
    },
    CasoNegativo {
        nome: "local_m_igual_import_data",
        // `import` é palavra-chave desde a T34 — não é mais um `Name` válido
        // à direita de `=`, então o parser falha ao tentar iniciar uma
        // expressão ali (mesmo mecanismo do antigo `import_de_modulo`, que
        // saiu desta tabela na T44 porque a forma de topo `import data` virou
        // caso positivo).
        fonte: "local m = import \"data\"\n\nfunction main(args: {string}): integer\n    return 0\nend",
        trecho_esperado: "Esperava uma expressão",
    },
    // T72: as duas rejeições que saíram desta tabela (`import data as d` e
    // `df:soma(...)`) viraram formas aceitas — cobertas pelos positivos em
    // `checker.rs` e por `alias_e_dois_pontos_compilam_e_rodam` abaixo. O
    // que permanece rejeitado é o alias que **colide** com um nome já
    // declarado.
    CasoNegativo {
        nome: "alias_de_import_colidindo_com_funcao",
        fonte: "import data as soma\n\nfunction soma(): integer\n    return 0\nend\n\nfunction main(args: {string}): integer\n    return 0\nend",
        trecho_esperado: "'soma' já foi declarado antes",
    },
    CasoNegativo {
        nome: "import_com_as_sem_nome_local",
        fonte: "import data as\n\nfunction main(args: {string}): integer\n    return 0\nend",
        trecho_esperado: "Esperava um nome local após 'as'",
    },
    CasoNegativo {
        nome: "metodo_com_dois_pontos_sem_nome",
        fonte: "import data\n\nfunction main(args: {string}): integer\n    local df: data.DataFrame = data.read_csv(\"v.csv\")\n    local total: float = df:(\"valor\")\n    return 0\nend",
        trecho_esperado: "Esperava um nome de método após ':'",
    },
];

#[test]
fn construcoes_fora_de_escopo_da_fase_3_produzem_erro_claro_sem_panic() {
    for caso in CASOS_FORA_DE_ESCOPO_FASE_3 {
        verifica_caso_negativo(caso, "fora-de-escopo-fase-3");
    }
}

/// Negativos da Fase 4 (PRD.md, T55 e T57): `break` é keyword e é aceito
/// dentro de `while`/`for`, mas continua rejeitado fora de laço — agora por
/// erro de tipos (`checker.rs`), não mais de sintaxe, já que o parser aceita
/// `break` em qualquer posição de comando. Desde a T63 (Fase 5) `continue`
/// segue exatamente o mesmo desenho: aceito dentro de laço, rejeitado fora
/// dele pelo checker, com a mesma mensagem em português. Os últimos três
/// casos são a curadoria da T57 (risco 5 à parte, coberto em
/// `compila_e_executa_hello_titan_conferindo_stdout_e_exit_code`): tipos soma
/// e `match` seguem sem sintaxe própria (Fase 5, pendente — PRD.md linha
/// 1889), `.titan` importando `.titan` cai na mesma rejeição de `import` com
/// string (T35) e `s[i]` é indexação de string, fora de escopo por decisão
/// explícita do checker.
const CASOS_FORA_DE_ESCOPO_FASE_4: &[CasoNegativo] = &[
    CasoNegativo {
        nome: "break_fora_de_laco",
        fonte: "function main(args: {string}): integer\n    break\n    return 0\nend",
        trecho_esperado: "`break` fora de um laço",
    },
    CasoNegativo {
        nome: "break_depois_do_laco",
        fonte: "function main(args: {string}): integer\n    while false do\n    end\n    break\n    return 0\nend",
        trecho_esperado: "`break` fora de um laço",
    },
    CasoNegativo {
        nome: "break_como_identificador",
        // `break` deixou de ser identificador válido (quebra compatível,
        // como `as` na T20 e `import` na T34): usá-lo como nome de variável
        // agora é erro de sintaxe, não mais uma declaração comum.
        fonte: "function main(args: {string}): integer\n    local break = 1\n    return 0\nend",
        trecho_esperado: "Esperava um nome de variável",
    },
    // `continue_em_while` e `continue_em_for` saíram desta tabela na T63:
    // viraram casos **positivos**, com execução real em
    // `compila_e_executa_continue_em_for_e_em_while` — o mesmo movimento que
    // `break` fez na T55. O que sobrou da família é o negativo de escopo,
    // logo abaixo, e ele mudou de camada: era erro de sintaxe (o parser
    // rejeitava `continue` em qualquer posição) e agora é erro de tipos, do
    // checker, exatamente como `break_fora_de_laco`.
    CasoNegativo {
        nome: "continue_fora_de_laco",
        fonte: "function main(args: {string}): integer\n    continue\n    return 0\nend",
        trecho_esperado: "`continue` fora de um laço",
    },
    CasoNegativo {
        nome: "continue_depois_do_laco",
        fonte: "function main(args: {string}): integer\n    while false do\n    end\n    continue\n    return 0\nend",
        trecho_esperado: "`continue` fora de um laço",
    },
    // `repeat`/`until` (T64) são positivos desde a fase — o que resta de
    // negativo é o `until` que falta (sintaxe) e a condição não-boolean
    // (tipos, ADR 0005: sem truthy/falsy), na mesma divisão de camadas de
    // `break` e `continue`.
    CasoNegativo {
        nome: "repeat_sem_until",
        fonte: "function main(args: {string}): integer\n    repeat\n        print(\"x\")\n    end\n    return 0\nend",
        trecho_esperado: "Esperava 'until' para fechar o 'repeat'",
    },
    CasoNegativo {
        nome: "until_com_condicao_nao_boolean",
        fonte: "function main(args: {string}): integer\n    repeat\n    until 1\n    return 0\nend",
        trecho_esperado: "condição do `until` precisa ser boolean",
    },
    CasoNegativo {
        nome: "local_do_repeat_nao_vaza",
        // O outro lado da armadilha de escopo da T64: o `until` enxerga os
        // `local` do corpo, mas depois do laço eles saem de escopo como em
        // qualquer bloco.
        fonte: "function main(args: {string}): integer\n    repeat\n        local x: integer = 1\n    until true\n    return x\nend",
        trecho_esperado: "'x' não foi declarado",
    },
    CasoNegativo {
        // `match` sobre o que não é `enum` (T76): não há exaustividade a
        // verificar sobre um `integer`, e um `if` já cobre o caso.
        nome: "tipo_soma_match_sobre_integer",
        fonte: "enum Cor\n    Vermelho\n    Verde\nend\n\nfunction main(args: {string}): integer\n    local x: integer = 1\n    match x with\n        Vermelho then\n            return 0\n    end\n    return 0\nend",
        trecho_esperado: "`match` só funciona sobre um `enum`, encontrado integer",
    },
    CasoNegativo {
        // A garantia que dá nome à T76 (decisão técnica 7 do PRD.md): a
        // exaustividade é conferida pelo checker, e a mensagem nomeia as
        // variantes que faltam — em português, sobre o código escrito, e
        // não em inglês sobre o Rust gerado.
        nome: "tipo_soma_match_nao_exaustivo",
        fonte: "enum Cor\n    Vermelho\n    Verde\n    Azul\nend\n\nfunction main(args: {string}): integer\n    local c: Cor = Vermelho\n    match c with\n        Vermelho then\n            return 0\n    end\n    return 0\nend",
        trecho_esperado: "não cobre todas as variantes de 'Cor': falta(m) Verde, Azul",
    },
    CasoNegativo {
        // Construção de variante com aridade errada (T76): o parser viu uma
        // chamada, o checker sabe que é construção e confere os campos.
        nome: "tipo_soma_construcao_com_aridade_errada",
        fonte: "enum Exp\n    ExpInteger(integer)\nend\n\nfunction main(args: {string}): integer\n    local e: Exp = ExpInteger(1, 2)\n    return 0\nend",
        trecho_esperado: "tem 1 campo(s), mas recebeu 2",
    },
    CasoNegativo {
        // A sintaxe do `match` é a do PRD.md (`with` + braços `padrão then`),
        // não a de seta do Rust/ML: `1 -> ...` é erro de sintaxe claro.
        nome: "tipo_soma_match_com_seta",
        fonte: "function main(args: {string}): integer\n    local x: integer = 1\n    match x\n        1 -> print(\"um\")\n    end\n    return 0\nend",
        trecho_esperado: "Esperava 'with' após a expressão do 'match'",
    },
    CasoNegativo {
        // `.titan` importando `.titan` cairia exatamente na forma `import`
        // com string (T35): não há mecanismo de módulo de usuário na Fase 4,
        // só as capabilities embutidas (`data`, `texto`, `io`).
        nome: "titan_importando_titan",
        fonte: "import \"outro.titan\"\n\nfunction main(args: {string}): integer\n    return 0\nend",
        trecho_esperado: "nome de string não é suportado",
    },
    CasoNegativo {
        nome: "indexacao_de_string",
        // `s[i]` é rejeitado por decisão explícita do checker (branch própria
        // para `Type::String` em `VarBracket`), distinta da rejeição
        // genérica de indexar um tipo não indexável.
        fonte: "function main(args: {string}): integer\n    local s: string = \"abc\"\n    local c: string = s[1]\n    return 0\nend",
        trecho_esperado: "não é possível indexar uma string",
    },
];

#[test]
fn construcoes_fora_de_escopo_da_fase_4_produzem_erro_claro_sem_panic() {
    for caso in CASOS_FORA_DE_ESCOPO_FASE_4 {
        verifica_caso_negativo(caso, "fora-de-escopo-fase-4");
    }
}

/// Quebra compatível registrada pela T59 (Fase 5): `enum`, `match`,
/// `continue`, `repeat`, `until`, `in` e `foreign` viraram palavras-chave e
/// deixaram de ser identificadores válidos — mesma mudança de `as` (T20),
/// `import` (T34) e `break` (T55). `with` entrou depois, na T75, quando a
/// sintaxe do `match` a exigiu. Um programa que usava qualquer uma delas
/// como nome de variável passa a ser erro de sintaxe claro, nunca panic.
const KEYWORDS_NOVAS_DA_T59: &[&str] = &[
    "enum", "match", "continue", "repeat", "until", "in", "foreign", "with",
];

#[test]
fn keywords_novas_da_t59_deixam_de_ser_identificadores_com_erro_claro() {
    for kw in KEYWORDS_NOVAS_DA_T59 {
        let out_dir = temp_dir(&format!("keyword-t59-{kw}"));
        let fonte = format!(
            "function main(args: {{string}}): integer\n    local {kw} = 1\n    return 0\nend"
        );
        let source_path = write_source(&out_dir, "caso.titan", &fonte);

        let output = Command::new(titanc_bin())
            .arg("--emit-rust")
            .arg("--out")
            .arg(&out_dir)
            .arg(&source_path)
            .output()
            .unwrap_or_else(|e| panic!("[{kw}] falha ao invocar titanc: {e}"));

        assert_never_panics(&output);
        assert!(
            !output.status.success(),
            "[{kw}] esperava falha (keyword como identificador), titanc reportou sucesso"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("Esperava um nome de variável"),
            "[{kw}] esperava erro de nome de variável, obteve: {stderr}"
        );

        let _ = std::fs::remove_dir_all(&out_dir);
    }
}

/// Critério de aceite da T55 (execução real): `break` sai de `while` e de
/// `for` de verdade — não é só aceito pelo checker, o `break;` do Rust
/// gerado realmente interrompe o laço no ponto certo. Compila fonte gerado
/// em memória (não paga nenhuma capability pesada) e confere stdout e exit
/// code do binário.
#[test]
fn compila_e_executa_break_saindo_de_while_e_de_for() {
    let out_dir = temp_dir("break-execucao-real");

    let source = concat!(
        "function main(args: {string}): integer\n",
        "    local i: integer = 1\n",
        "    while true do\n",
        "        if i > 3 then\n",
        "            break\n",
        "        end\n",
        "        print(texto_de_i(i))\n",
        "        i = i + 1\n",
        "    end\n",
        "    for j = 1, 10 do\n",
        "        if j > 2 then\n",
        "            break\n",
        "        end\n",
        "        print(texto_de_j(j))\n",
        "    end\n",
        "    return 0\n",
        "end\n",
        "\n",
        "function texto_de_i(i: integer): string\n",
        "    if i == 1 then\n",
        "        return \"while-1\"\n",
        "    elseif i == 2 then\n",
        "        return \"while-2\"\n",
        "    else\n",
        "        return \"while-3\"\n",
        "    end\n",
        "end\n",
        "\n",
        "function texto_de_j(j: integer): string\n",
        "    if j == 1 then\n",
        "        return \"for-1\"\n",
        "    else\n",
        "        return \"for-2\"\n",
        "    end\n",
        "end",
    );
    let source_path = write_source(&out_dir, "break_exec.titan", source);

    let compile_output = Command::new(titanc_bin())
        .arg("--out")
        .arg(&out_dir)
        .arg(&source_path)
        .output()
        .expect("invoca titanc");
    assert_never_panics(&compile_output);
    assert!(
        compile_output.status.success(),
        "titanc falhou ao compilar o caso de break: {}",
        String::from_utf8_lossy(&compile_output.stderr)
    );

    let binary = out_dir.join("break_exec");
    assert!(binary.exists(), "esperava executável em {binary:?}");

    let run_output = Command::new(&binary)
        .output()
        .expect("executa ./break_exec");
    let esperado = "while-1\nwhile-2\nwhile-3\nfor-1\nfor-2\n";
    assert_eq!(String::from_utf8_lossy(&run_output.stdout), esperado);
    assert_eq!(run_output.status.code(), Some(0));

    let _ = std::fs::remove_dir_all(&out_dir);
}

/// Critério de aceite da T63 — o ponto central da tarefa, e a prova de que o
/// bug que o ADR 0017 temia deixou de existir: `continue` dentro de um `for`
/// **avança o laço**. Com o template antigo (incremento no fim do corpo,
/// ADR 0004) este programa entraria em laço infinito na primeira iteração
/// par; com o incremento no topo do `loop` (ADR 0022, T62) ele imprime os
/// ímpares e **termina**. O `while` cobre o outro laço da linguagem: lá o
/// incremento é escrito pelo usuário, então o `continue` precisa vir depois
/// dele — é a mesma semântica do Lua, e do Rust.
#[test]
fn compila_e_executa_continue_em_for_e_em_while() {
    let out_dir = temp_dir("continue-execucao-real");

    let source = concat!(
        "function main(args: {string}): integer\n",
        // `for i = 1, 5` pulando os pares: imprime 1, 3, 5 e termina. Se o
        // `continue` pulasse o incremento, o programa travaria em i = 2.
        "    for i = 1, 5 do\n",
        "        if i % 2 == 0 then\n",
        "            continue\n",
        "        end\n",
        "        print(\"for-\" .. i)\n",
        "    end\n",
        // Decrescente com passo negativo: o `continue` também precisa passar
        // pelo `i += titan_for_inc` com `titan_for_asc` falso.
        "    for j = 5, 1, -2 do\n",
        "        if j == 3 then\n",
        "            continue\n",
        "        end\n",
        "        print(\"dec-\" .. j)\n",
        "    end\n",
        // `while`: o incremento é do usuário e vem antes do `continue`.
        "    local k: integer = 0\n",
        "    while k < 5 do\n",
        "        k = k + 1\n",
        "        if k % 2 == 1 then\n",
        "            continue\n",
        "        end\n",
        "        print(\"while-\" .. k)\n",
        "    end\n",
        // `continue` em laço aninhado afeta só o laço mais interno.
        "    for a = 1, 2 do\n",
        "        for b = 1, 3 do\n",
        "            if b == 2 then\n",
        "                continue\n",
        "            end\n",
        "            print(\"ani-\" .. a .. \"-\" .. b)\n",
        "        end\n",
        "    end\n",
        // `continue` convivendo com `break` no mesmo laço.
        "    for c = 1, 10 do\n",
        "        if c == 2 then\n",
        "            continue\n",
        "        end\n",
        "        if c == 4 then\n",
        "            break\n",
        "        end\n",
        "        print(\"mix-\" .. c)\n",
        "    end\n",
        "    return 0\n",
        "end",
    );
    let source_path = write_source(&out_dir, "continue_exec.titan", source);

    let compile_output = Command::new(titanc_bin())
        .arg("--out")
        .arg(&out_dir)
        .arg(&source_path)
        .output()
        .expect("invoca titanc");
    assert_never_panics(&compile_output);
    assert!(
        compile_output.status.success(),
        "titanc falhou ao compilar o caso de continue: {}",
        String::from_utf8_lossy(&compile_output.stderr)
    );

    let binary = out_dir.join("continue_exec");
    assert!(binary.exists(), "esperava executável em {binary:?}");

    let run_output = Command::new(&binary)
        .output()
        .expect("executa ./continue_exec");
    let esperado = concat!(
        "for-1\nfor-3\nfor-5\n",
        "dec-5\ndec-1\n",
        "while-2\nwhile-4\n",
        "ani-1-1\nani-1-3\nani-2-1\nani-2-3\n",
        "mix-1\nmix-3\n",
    );
    assert_eq!(String::from_utf8_lossy(&run_output.stdout), esperado);
    assert_eq!(run_output.status.code(), Some(0));

    let _ = std::fs::remove_dir_all(&out_dir);
}

/// Critério de aceite da T71 (execução real, os quatro pontos da tarefa):
/// soma dos elementos de um `{integer}`, iteração de um `{string: integer}`,
/// `break`/`continue` dentro, e mutar o container durante a iteração dando
/// erro claro em português (este último no teste seguinte, porque é um caso
/// negativo).
///
/// A iteração do map é conferida por **agregação**, nunca por ordem de
/// saída: `{K: V}` é `HashMap`, cuja ordem é não especificada (ADR 0024) —
/// um teste que fixasse a ordem passaria hoje e falharia amanhã sem nenhuma
/// mudança no compilador.
#[test]
fn compila_e_executa_for_in_sobre_array_e_map() {
    let out_dir = temp_dir("for-in-execucao-real");

    let source = concat!(
        "function soma_param(xs: {integer}): integer\n",
        // O container é um **parâmetro** composto, isto é, um `&mut Vec<i64>`
        // no Rust gerado: `.iter()` precisa atravessar a referência.
        "    local s: integer = 0\n",
        "    for x in xs do\n",
        "        s = s + x\n",
        "    end\n",
        "    return s\n",
        "end\n",
        "\n",
        "function nomes(): {string}\n",
        "    return {\"ana\", \"bia\"}\n",
        "end\n",
        "\n",
        "function main(args: {string}): integer\n",
        // 1. Soma dos elementos de um `{integer}`.
        "    local v: {integer} = {10, 20, 30}\n",
        "    local soma: integer = 0\n",
        "    for x in v do\n",
        "        soma = soma + x\n",
        "    end\n",
        "    print(\"soma: \" .. soma)\n",
        "    print(\"param: \" .. soma_param(v))\n",
        // 2. Iteração de um `{string: integer}`: as duas variáveis ligadas,
        //    agregadas em somas que independem da ordem.
        "    local m: {string: integer} = {[\"ana\"] = 30, [\"bia\"] = 25}\n",
        "    local idades: integer = 0\n",
        "    local letras: integer = 0\n",
        "    for nome, idade in m do\n",
        "        idades = idades + idade\n",
        "        letras = letras + #nome\n",
        "    end\n",
        "    print(\"idades: \" .. idades)\n",
        "    print(\"letras: \" .. letras)\n",
        // Só o valor usado: a chave sai como `_` no padrão do iterador, e o
        // Rust gerado precisa compilar **sem warning** de variável não usada.
        "    local so_valores: integer = 0\n",
        "    for chave, idade in m do\n",
        "        so_valores = so_valores + idade\n",
        "    end\n",
        "    print(\"so-valores: \" .. so_valores)\n",
        // 3. `break` e `continue` (T63) dentro do `for`-in.
        "    for y in v do\n",
        "        if y == 20 then\n",
        "            continue\n",
        "        end\n",
        "        if y == 30 then\n",
        "            break\n",
        "        end\n",
        "        print(\"bc: \" .. y)\n",
        "    end\n",
        // Container que é uma **chamada**, não um nome: itera um temporário.
        "    for nome in nomes() do\n",
        "        print(\"nome: \" .. nome)\n",
        "    end\n",
        // Aninhado, sobre `{{integer}}`: a variável do laço externo é ela
        // própria um container, e vira o container do laço interno.
        "    local matriz: {{integer}} = {{1, 2}, {3, 4}}\n",
        "    for linha in matriz do\n",
        "        local parcial: integer = 0\n",
        "        for c in linha do\n",
        "            parcial = parcial + c\n",
        "        end\n",
        "        print(\"linha: \" .. parcial)\n",
        "    end\n",
        // A variável do laço é uma **cópia** (ADR 0006/0024): escrever nela
        // não alcança o container.
        "    print(\"intacto: \" .. v[1])\n",
        "    return 0\n",
        "end",
    );
    let source_path = write_source(&out_dir, "for_in_exec.titan", source);

    let compile_output = Command::new(titanc_bin())
        .arg("--out")
        .arg(&out_dir)
        .arg(&source_path)
        .output()
        .expect("invoca titanc");
    assert_never_panics(&compile_output);
    assert!(
        compile_output.status.success(),
        "titanc falhou ao compilar o caso de for-in: {}",
        String::from_utf8_lossy(&compile_output.stderr)
    );
    // Critério herdado da T69: Rust gerado **sem warnings**. O `cargo` do
    // titanc escreve os avisos do rustc no stderr do próprio titanc.
    let compile_stderr = String::from_utf8_lossy(&compile_output.stderr);
    assert!(
        !compile_stderr.contains("warning:"),
        "o Rust gerado para o for-in saiu com warning: {compile_stderr}"
    );

    let binary = out_dir.join("for_in_exec");
    assert!(binary.exists(), "esperava executável em {binary:?}");

    let run_output = Command::new(&binary)
        .output()
        .expect("executa ./for_in_exec");
    let esperado = concat!(
        "soma: 60\n",
        "param: 60\n",
        "idades: 55\n",
        "letras: 6\n",
        "so-valores: 55\n",
        "bc: 10\n",
        "nome: ana\n",
        "nome: bia\n",
        "linha: 3\n",
        "linha: 7\n",
        "intacto: 10\n",
    );
    assert_eq!(String::from_utf8_lossy(&run_output.stdout), esperado);
    assert_eq!(run_output.status.code(), Some(0));

    let _ = std::fs::remove_dir_all(&out_dir);
}

/// O quarto ponto do critério de aceite da T71: mutar o container durante a
/// iteração dá erro **claro, em português, do checker** — e não `cannot
/// borrow as mutable` do `rustc`, que é o que sairia sem esta checagem
/// (ADR 0024).
///
/// As três formas de mutação são conferidas separadamente porque cada uma
/// chega ao detector por um caminho diferente da AST: alvo de atribuição
/// simples, alvo dentro de uma cadeia de índice, e argumento de chamada (que
/// é uso mutável porque parâmetro composto é `&mut`, ADR 0007).
#[test]
fn mutar_o_container_durante_o_for_in_produz_erro_claro_em_portugues() {
    let casos: &[(&str, &str)] = &[
        (
            "atribuicao_ao_container",
            "function main(args: {string}): integer\n\
             \x20   local v: {integer} = {1, 2}\n\
             \x20   for x in v do\n\
             \x20       v = {3}\n\
             \x20   end\n\
             \x20   return 0\n\
             end",
        ),
        (
            "escrita_em_elemento",
            "function main(args: {string}): integer\n\
             \x20   local v: {integer} = {1, 2}\n\
             \x20   for x in v do\n\
             \x20       v[1] = 99\n\
             \x20   end\n\
             \x20   return 0\n\
             end",
        ),
        (
            "passado_como_argumento",
            "function zera(xs: {integer}): integer\n\
             \x20   xs[1] = 0\n\
             \x20   return 0\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   local v: {integer} = {1, 2}\n\
             \x20   for x in v do\n\
             \x20       zera(v)\n\
             \x20   end\n\
             \x20   return 0\n\
             end",
        ),
        // Aninhado: mutar o container do laço **externo** de dentro do
        // interno é a mesma ofensa, e a varredura tem de alcançá-la.
        (
            "mutacao_em_laco_aninhado",
            "function main(args: {string}): integer\n\
             \x20   local v: {integer} = {1, 2}\n\
             \x20   local w: {integer} = {3}\n\
             \x20   for x in v do\n\
             \x20       for y in w do\n\
             \x20           v[1] = 0\n\
             \x20       end\n\
             \x20   end\n\
             \x20   return 0\n\
             end",
        ),
    ];

    for (nome, fonte) in casos {
        let out_dir = temp_dir(&format!("for-in-mutacao-{nome}"));
        let source_path = write_source(&out_dir, "mutacao.titan", fonte);
        let output = Command::new(titanc_bin())
            .arg("--out")
            .arg(&out_dir)
            .arg(&source_path)
            .output()
            .expect("invoca titanc");
        assert_never_panics(&output);
        assert!(
            !output.status.success(),
            "[{nome}] esperava falha, titanc reportou sucesso"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("não é possível modificar 'v' dentro do `for`-in"),
            "[{nome}] mensagem inesperada: {stderr}"
        );
        // A convenção do projeto: nunca o erro do rustc em inglês.
        assert!(
            !stderr.contains("cannot borrow"),
            "[{nome}] vazou erro do rustc: {stderr}"
        );
        let _ = std::fs::remove_dir_all(&out_dir);
    }
}

/// Critério de aceite da T64 (execução real, os três pontos da tarefa): o
/// `repeat` roda ao menos uma vez **mesmo** com a condição de saída já
/// verdadeira; o `until` referencia um `local` declarado no corpo — a
/// armadilha de escopo herdada do Lua, que obriga o checker a fechar o bloco
/// só depois de tipar a condição; e `break`/`continue` (T63) funcionam lá
/// dentro sem caso especial, porque `repeat` também é emitido como um `loop`
/// do Rust (ADR 0023).
#[test]
fn compila_e_executa_repeat_until() {
    let out_dir = temp_dir("repeat-execucao-real");

    let source = concat!(
        "function main(args: {string}): integer\n",
        // 1. Condição de saída já verdadeira na entrada: o corpo roda uma
        //    vez. Com um `while` no lugar, não imprimiria nada.
        "    local n: integer = 0\n",
        "    repeat\n",
        "        n = n + 1\n",
        "        print(\"uma-vez-\" .. n)\n",
        "    until true\n",
        // 2. O `until` lê `dobro`, um `local` do corpo.
        "    local m: integer = 0\n",
        "    repeat\n",
        "        local dobro: integer = m * 2\n",
        "        m = m + 1\n",
        "        print(\"dobro-\" .. dobro)\n",
        "    until dobro >= 6\n",
        // 3. `break` e `continue` no mesmo `repeat`. A condição do `until`
        //    nunca fica verdadeira: quem termina o laço é o `break`.
        "    local k: integer = 0\n",
        "    repeat\n",
        "        k = k + 1\n",
        "        if k == 2 then\n",
        "            continue\n",
        "        end\n",
        "        if k == 5 then\n",
        "            break\n",
        "        end\n",
        "        print(\"mix-\" .. k)\n",
        "    until k > 100\n",
        // 4. Aninhado: o `break` interno fecha só o laço de dentro.
        "    local a: integer = 0\n",
        "    repeat\n",
        "        a = a + 1\n",
        "        local b: integer = 0\n",
        "        repeat\n",
        "            b = b + 1\n",
        "            print(\"ani-\" .. a .. \"-\" .. b)\n",
        "        until b >= 2\n",
        "    until a >= 2\n",
        // 5. `repeat` dentro de `for` e `for` dentro de `repeat`: os dois
        //    laços convivem sem o template de um atrapalhar o do outro.
        "    for i = 1, 2 do\n",
        "        local c: integer = 0\n",
        "        repeat\n",
        "            c = c + 1\n",
        "        until c >= i\n",
        "        print(\"for-rep-\" .. i .. \"-\" .. c)\n",
        "    end\n",
        "    return 0\n",
        "end",
    );
    let source_path = write_source(&out_dir, "repeat_exec.titan", source);

    let compile_output = Command::new(titanc_bin())
        .arg("--out")
        .arg(&out_dir)
        .arg(&source_path)
        .output()
        .expect("invoca titanc");
    assert_never_panics(&compile_output);
    assert!(
        compile_output.status.success(),
        "titanc falhou ao compilar o caso de repeat: {}",
        String::from_utf8_lossy(&compile_output.stderr)
    );

    let binary = out_dir.join("repeat_exec");
    assert!(binary.exists(), "esperava executável em {binary:?}");

    let run_output = Command::new(&binary)
        .output()
        .expect("executa ./repeat_exec");
    let esperado = concat!(
        "uma-vez-1\n",
        "dobro-0\ndobro-2\ndobro-4\ndobro-6\n",
        "mix-1\nmix-3\nmix-4\n",
        "ani-1-1\nani-1-2\nani-2-1\nani-2-2\n",
        "for-rep-1-1\nfor-rep-2-2\n",
    );
    assert_eq!(String::from_utf8_lossy(&run_output.stdout), esperado);
    assert_eq!(run_output.status.code(), Some(0));

    let _ = std::fs::remove_dir_all(&out_dir);
}

/// Critério de aceite da T66, nos dois pontos que a tarefa pede: o
/// `--emit-rust` confere a **tupla** — na assinatura (`-> (i64, i64)`) e no
/// `return` — e a `divmod` roda de verdade, imprimindo os dois valores.
///
/// A divisão entre os dois trechos do teste não é acidental. Até a T67, o
/// fonte só alcança o **primeiro** valor de retorno: a chamada em posição de
/// expressão ajusta para ele (o `Adjust` da T65), e `local q, r = divmod(...)`
/// — a forma que lê o segundo — é justamente a rejeição que a T67 remove.
/// Então os dois valores são impressos de dentro da própria `divmod`, que é
/// onde a execução os alcança hoje, e o que o chamador confere é que o `.0`
/// da tupla chega correto do outro lado da fronteira de função. Que a tupla
/// transporta o segundo valor com o mesmo rigor está no texto emitido, aqui
/// e no teste de `Extra` no codegen.
#[test]
fn compila_e_executa_retornos_multiplos_como_tupla() {
    let out_dir = temp_dir("retornos-multiplos-execucao-real");

    let source = concat!(
        "function divmod(a: integer, b: integer): integer, integer\n",
        "    local q: integer = a // b\n",
        "    local r: integer = a % b\n",
        "    print(\"divmod-\" .. a .. \"-\" .. b .. \"=\" .. q .. \",\" .. r)\n",
        "    return q, r\n",
        "end\n",
        // Dois retornos de tipos **diferentes**, com um composto e uma
        // `string` na mesma tupla: o componente composto sai por valor e
        // cada um segue a regra de slot do ADR 0006 (`.clone()` no que
        // sobrevive ao `return`).
        "function rotula(n: integer): {integer}, string\n",
        "    local v: {integer} = {n, n + 1}\n",
        "    local s: string = \"rot\"\n",
        "    return v, s\n",
        "end\n",
        "function main(args: {string}): integer\n",
        // O ajuste para o primeiro valor atravessa a fronteira de função: o
        // `.0` da tupla é o quociente, não o resto.
        "    local q: integer = divmod(7, 2)\n",
        "    print(\"q-\" .. q)\n",
        // Negativo, onde `//` diverge do `/` do Rust (T61): a tupla não
        // muda essa conta, e o teste garante que não passou a mudar.
        "    local qn: integer = divmod(-7, 2)\n",
        "    print(\"qn-\" .. qn)\n",
        "    local v: {integer} = rotula(10)\n",
        "    print(\"v-\" .. #v .. \"-\" .. v[1])\n",
        "    return 0\n",
        "end",
    );
    let source_path = write_source(&out_dir, "retornos_multiplos.titan", source);

    // 1. `--emit-rust`: a tupla aparece na assinatura e no `return`, e o
    //    retorno único continua sem tupla de um elemento.
    let emit_output = Command::new(titanc_bin())
        .arg("--emit-rust")
        .arg(&source_path)
        .output()
        .expect("invoca titanc --emit-rust");
    assert_never_panics(&emit_output);
    assert!(
        emit_output.status.success(),
        "titanc --emit-rust falhou para retornos múltiplos: {}",
        String::from_utf8_lossy(&emit_output.stderr)
    );
    let rust = String::from_utf8_lossy(&emit_output.stdout);
    assert!(
        rust.contains("pub fn titan_divmod(a: i64, b: i64) -> (i64, i64) {"),
        "assinatura sem tupla:\n{rust}"
    );
    assert!(rust.contains("return (q, r);"), "return sem tupla:\n{rust}");
    assert!(
        rust.contains("pub fn titan_rotula(n: i64) -> (Vec<i64>, String) {"),
        "composto e string na tupla saem por valor:\n{rust}"
    );
    assert!(
        rust.contains("pub fn titan_main(_args: &mut Vec<String>) -> i64 {"),
        "retorno único não deveria virar tupla:\n{rust}"
    );
    assert!(
        rust.contains("titan_divmod(7, 2).0"),
        "ajuste não indexou a tupla:\n{rust}"
    );

    // 2. Execução real do mesmo programa.
    let compile_output = Command::new(titanc_bin())
        .arg("--out")
        .arg(&out_dir)
        .arg(&source_path)
        .output()
        .expect("invoca titanc");
    assert_never_panics(&compile_output);
    assert!(
        compile_output.status.success(),
        "titanc falhou ao compilar o caso de retornos múltiplos: {}",
        String::from_utf8_lossy(&compile_output.stderr)
    );

    let binary = out_dir.join("retornos_multiplos");
    assert!(binary.exists(), "esperava executável em {binary:?}");

    let run_output = Command::new(&binary)
        .output()
        .expect("executa ./retornos_multiplos");
    let esperado = concat!(
        "divmod-7-2=3,1\n",
        "q-3\n",
        // `-7 // 2` é -4 (piso, via `idiv` — o `/` do Rust daria -3),
        // enquanto `%` sai como o resto do Rust, -1. Os dois valores
        // atravessam a tupla sem serem tocados por ela.
        "divmod--7-2=-4,-1\n",
        "qn--4\n",
        "v-2-10\n",
    );
    assert_eq!(String::from_utf8_lossy(&run_output.stdout), esperado);
    assert_eq!(run_output.status.code(), Some(0));

    let _ = std::fs::remove_dir_all(&out_dir);
}

/// Critério de aceite da T67, pelo pipeline completo e em **execução real**,
/// nos três pontos que a tarefa pede: o swap `a, b = b, a` troca de verdade,
/// `local q, r = divmod(7, 2)` dá 3 e 1, e a aridade errada dá erro claro.
///
/// O swap é o ponto central. A semântica do Lua — herdada pelo Titan — avalia
/// **todo** o lado direito antes de escrever em qualquer alvo; emitir as
/// atribuições em sequência (`a = b; b = a;`) daria `a == b`, um bug
/// silencioso que nenhum teste de tipo pegaria. Por isso o teste confere o
/// texto emitido (os temporários aparecem antes das escritas) **e** o valor
/// impresso pelo binário.
#[test]
fn compila_e_executa_multi_assign_e_declaracao_multipla() {
    let out_dir = temp_dir("multi-assign-execucao-real");

    let source = concat!(
        "record Ponto\n",
        "    x: integer\n",
        "    y: integer\n",
        "end\n",
        "function divmod(a: integer, b: integer): integer, integer\n",
        "    return a // b, a % b\n",
        "end\n",
        // Dois retornos de tipos diferentes, com um composto e uma `string`:
        // a desestruturação segue as mesmas regras de slot do ADR 0006/0007
        // que o `return` da T66 segue.
        "function rotula(n: integer): string, {integer}\n",
        "    return \"rot\", {n, n + 1}\n",
        "end\n",
        "function main(args: {string}): integer\n",
        // 1. O swap de escalares.
        "    local a: integer = 1\n",
        "    local b: integer = 2\n",
        "    a, b = b, a\n",
        "    print(\"swap-\" .. a .. \"-\" .. b)\n",
        // 2. A desestruturação da tupla da T66.
        "    local q, r = divmod(7, 2)\n",
        "    print(\"divmod-\" .. q .. \"-\" .. r)\n",
        // 3. Declaração múltipla por lista, com anotação de tipo em cada
        //    nome.
        "    local x: integer, y: integer = 10, 20\n",
        "    print(\"lista-\" .. x .. \"-\" .. y)\n",
        // 4. O swap alcança lugares compostos, não só nomes: `v[i]` passa
        //    por `array_set` e `p.campo` por escrita direta, cada um pelo
        //    mesmo caminho do single-target.
        "    local v: {integer} = {10, 20}\n",
        "    v[1], v[2] = v[2], v[1]\n",
        "    print(\"vetor-\" .. v[1] .. \"-\" .. v[2])\n",
        "    local p: Ponto = {x = 1, y = 2}\n",
        "    p.x, p.y = p.y, p.x\n",
        "    print(\"ponto-\" .. p.x .. \"-\" .. p.y)\n",
        // 5. `string` e composto desestruturados da mesma chamada.
        "    local s, w = rotula(5)\n",
        "    print(\"rot-\" .. s .. \"-\" .. #w .. \"-\" .. w[2])\n",
        // 6. Um alvo que nunca mais é atribuído sai imutável, e um que é
        //    sai `mut` — o fix-up marca **todos** os alvos, não só o
        //    primeiro (armadilha explícita da tarefa).
        "    local m: integer, n: integer = 1, 2\n",
        "    m = m + n\n",
        "    print(\"mut-\" .. m .. \"-\" .. n)\n",
        "    return 0\n",
        "end",
    );
    let source_path = write_source(&out_dir, "multi_assign.titan", source);

    // 1. `--emit-rust`: os temporários vêm **antes** de qualquer escrita, e
    //    a desestruturação é um `let` de tupla só.
    let emit_output = Command::new(titanc_bin())
        .arg("--emit-rust")
        .arg(&source_path)
        .output()
        .expect("invoca titanc --emit-rust");
    assert_never_panics(&emit_output);
    assert!(
        emit_output.status.success(),
        "titanc --emit-rust falhou para multi-assign: {}",
        String::from_utf8_lossy(&emit_output.stderr)
    );
    let rust = String::from_utf8_lossy(&emit_output.stdout);
    assert!(
        rust.contains("let titan_multi_0 = b;\n    let titan_multi_1 = a;\n    a = titan_multi_0;\n    b = titan_multi_1;"),
        "o swap não passou por temporários — atribuição em sequência daria a == b:\n{rust}"
    );
    assert!(
        rust.contains("let (titan_multi_0, titan_multi_1) = titan_divmod(7, 2);"),
        "a chamada não foi desestruturada da tupla:\n{rust}"
    );
    // O alvo atribuído depois sai `mut`; o que não é, não sai — e um não
    // contamina o outro.
    assert!(
        rust.contains("let mut m: i64 = titan_multi_0;"),
        "alvo reatribuído devia sair `mut`:\n{rust}"
    );
    assert!(
        rust.contains("let n: i64 = titan_multi_1;"),
        "alvo nunca reatribuído não devia sair `mut`:\n{rust}"
    );

    // 2. Execução real: os valores, que é onde a ordem de avaliação aparece.
    let compile_output = Command::new(titanc_bin())
        .arg("--out")
        .arg(&out_dir)
        .arg(&source_path)
        .output()
        .expect("invoca titanc");
    assert_never_panics(&compile_output);
    assert!(
        compile_output.status.success(),
        "titanc falhou ao compilar o caso de multi-assign: {}",
        String::from_utf8_lossy(&compile_output.stderr)
    );

    let binary = out_dir.join("multi_assign");
    assert!(binary.exists(), "esperava executável em {binary:?}");

    let run_output = Command::new(&binary)
        .output()
        .expect("executa ./multi_assign");
    let esperado = concat!(
        "swap-2-1\n",
        "divmod-3-1\n",
        "lista-10-20\n",
        "vetor-20-10\n",
        "ponto-2-1\n",
        "rot-rot-2-6\n",
        "mut-3-2\n",
    );
    assert_eq!(String::from_utf8_lossy(&run_output.stdout), esperado);
    assert_eq!(run_output.status.code(), Some(0));

    // 3. Aridade errada, nas duas formas, com erro claro e sem panic.
    for (nome, fonte, trecho) in [
        (
            "aridade_chamada",
            "function divmod(a: integer, b: integer): integer, integer\n    return a // b, a % b\nend\nfunction main(args: {string}): integer\n    local a, b, c = divmod(7, 2)\n    return 0\nend",
            "declaração múltipla com 3 alvo(s), mas a chamada produz 2 valor(es)",
        ),
        (
            "aridade_lista",
            "function main(args: {string}): integer\n    local a: integer = 1\n    local b: integer = 2\n    a, b = 1\n    return 0\nend",
            "atribuição múltipla com 2 alvo(s), mas 1 valor(es)",
        ),
    ] {
        let caminho = write_source(&out_dir, &format!("{nome}.titan"), fonte);
        let saida = Command::new(titanc_bin())
            .arg("--emit-rust")
            .arg(&caminho)
            .output()
            .expect("invoca titanc --emit-rust");
        assert_never_panics(&saida);
        assert!(!saida.status.success(), "{nome} devia falhar");
        let stderr = String::from_utf8_lossy(&saida.stderr);
        assert!(
            stderr.contains(trecho),
            "{nome}: erro pouco claro:\n{stderr}"
        );
    }

    let _ = std::fs::remove_dir_all(&out_dir);
}

/// Critério de aceite da T61 pelo pipeline completo (`titanc` → Rust →
/// `cargo build` → executável): bitwise e `//` percorrem lexer, parser,
/// checker e codegen e produzem os valores certos em execução real. O caso
/// que dá nome à tarefa é `-7 // 2`: o `/` do Rust truncaria para -3, e o
/// `//` do Titan/Lua arredonda para baixo, dando -4.
#[test]
fn compila_e_executa_bitwise_e_divisao_inteira() {
    let out_dir = temp_dir("bitwise-execucao-real");

    // Os parênteses em volta dos bitwise são necessários: na cascata de
    // precedência do Titan (T60) `..` liga mais forte que `&`/`|`/`~`, então
    // sem eles a string entraria como operando do operador bitwise.
    let source = concat!(
        "function main(args: {string}): integer\n",
        "    print(\"7//2=\" .. 7 // 2)\n",
        "    print(\"-7//2=\" .. -7 // 2)\n",
        "    print(\"and=\" .. (5 & 3))\n",
        "    print(\"or=\" .. (5 | 3))\n",
        "    print(\"xor=\" .. (5 ~ 3))\n",
        "    print(\"shl=\" .. (1 << 10))\n",
        // Deslocamento fora de `0..64` é legal no Titan (zera) e overflow no
        // Rust — com constantes, o rustc recusaria a compilação em inglês.
        "    print(\"shl64=\" .. (1 << 64))\n",
        "    print(\"shlneg=\" .. (1024 << -10))\n",
        "    print(\"not=\" .. ~0)\n",
        "    return 0\n",
        "end",
    );
    let source_path = write_source(&out_dir, "bitwise_exec.titan", source);

    let compile_output = Command::new(titanc_bin())
        .arg("--out")
        .arg(&out_dir)
        .arg(&source_path)
        .output()
        .expect("invoca titanc");
    assert_never_panics(&compile_output);
    assert!(
        compile_output.status.success(),
        "titanc falhou ao compilar o caso de bitwise: {}",
        String::from_utf8_lossy(&compile_output.stderr)
    );

    let binary = out_dir.join("bitwise_exec");
    assert!(binary.exists(), "esperava executável em {binary:?}");

    let run_output = Command::new(&binary)
        .output()
        .expect("executa ./bitwise_exec");
    let esperado = "7//2=3\n-7//2=-4\nand=1\nor=7\nxor=6\nshl=1024\nshl64=0\nshlneg=1\nnot=-1\n";
    assert_eq!(String::from_utf8_lossy(&run_output.stdout), esperado);
    assert_eq!(run_output.status.code(), Some(0));

    let _ = std::fs::remove_dir_all(&out_dir);
}

/// `1.5 & 2` é o caso que o PRD (T61) destaca: o rustc recusaria em inglês,
/// sobre código que o usuário não escreveu — aqui o erro chega em português,
/// do checker, antes de qualquer `cargo build`.
#[test]
fn bitwise_com_float_produz_erro_em_portugues_sem_panic() {
    let out_dir = temp_dir("bitwise-float");
    let source_path = write_source(
        &out_dir,
        "bitwise_float.titan",
        "function main(args: {string}): integer\n    local a = 1.5 & 2\n    return 0\nend",
    );

    let output = Command::new(titanc_bin())
        .arg("--emit-rust")
        .arg(&source_path)
        .output()
        .expect("invoca titanc");
    assert_never_panics(&output);
    assert!(!output.status.success(), "esperava falha de checagem");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("operando de `&` precisa ser integer"),
        "stderr inesperado: {stderr}"
    );

    let _ = std::fs::remove_dir_all(&out_dir);
}

/// Nomes de arquivos `.titan` **reais** do Titan original (relativos a
/// `../titan/`) que a Fase 2 **espera compilar** sem alterações — usam
/// exclusivamente arrays e o idioma central de referência
/// (`selection_sort.titan` só é aceito porque parâmetros de array são
/// passados por `&mut`, decisão 4 da Fase 2). Nenhum desses arquivos declara
/// `main(args: {string}): integer` — o titanc exige essa assinatura em todo
/// programa —, então a prova de que "compilam" é feita com o corpo do
/// arquivo real mais um `main` mínimo apenso, não com o arquivo cru.
const ARQUIVOS_QUE_A_FASE_2_ESPERA_COMPILAR: &[&str] =
    &["testfiles/sieve.titan", "testfiles/selection_sort.titan"];

/// Regressão com arquivos `.titan` **reais** do Titan original (PRD.md,
/// T16/T31) — a referência somente leitura em `../titan/`. A propriedade que
/// sempre importou (e que o nome do teste enuncia) não é "todo arquivo real
/// é rejeitado" — isso deixou de valer quando arrays passaram a ter suporte
/// real no codegen (T30) — e sim: o titanc **nunca panica** e **nunca
/// produz stderr vazio** ao processá-los, compilando com sucesso ou falhando
/// com uma mensagem de erro clara.
#[test]
fn arquivos_reais_do_titan_original_nunca_panicam() {
    let titan_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../titan");
    if !titan_dir.exists() {
        // A referência é um repositório de terceiros fora deste workspace;
        // sem ela presente não há o que exercitar.
        eprintln!("aviso: {} ausente — regressão pulada", titan_dir.display());
        return;
    }

    for relativo in [
        "examples/artisanal.titan",
        "testfiles/sieve.titan",
        "testfiles/selection_sort.titan",
    ] {
        let source_path = titan_dir.join(relativo);
        assert!(
            source_path.exists(),
            "esperava arquivo de referência em {source_path:?}"
        );
        let out_dir = temp_dir(&format!(
            "titan-original-{}",
            relativo.replace(['/', '.'], "-")
        ));

        let output = Command::new(titanc_bin())
            .arg("--out")
            .arg(&out_dir)
            .arg(&source_path)
            .output()
            .unwrap_or_else(|e| panic!("[{relativo}] falha ao invocar titanc: {e}"));

        assert_never_panics(&output);
        if !output.status.success() {
            assert!(
                !String::from_utf8_lossy(&output.stderr).trim().is_empty(),
                "[{relativo}] falhou sem mensagem de erro em stderr"
            );
        }

        let _ = std::fs::remove_dir_all(&out_dir);
    }
}

/// Medida de progresso da Fase 2: os arquivos listados em
/// `ARQUIVOS_QUE_A_FASE_2_ESPERA_COMPILAR` compilam e executam de verdade
/// quando envolvidos por um `main` mínimo — prova viva de que o idioma de
/// arrays do Titan original (incluindo `selection_sort.titan`, que muta o
/// array do chamador via `&mut`) já é suportado ponta a ponta.
#[test]
fn arquivos_que_a_fase_2_espera_compilar_compilam_de_verdade() {
    let titan_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../titan");
    if !titan_dir.exists() {
        eprintln!("aviso: {} ausente — regressão pulada", titan_dir.display());
        return;
    }

    for relativo in ARQUIVOS_QUE_A_FASE_2_ESPERA_COMPILAR {
        let source_path = titan_dir.join(relativo);
        assert!(
            source_path.exists(),
            "esperava arquivo de referência em {source_path:?}"
        );
        let corpo = std::fs::read_to_string(&source_path)
            .unwrap_or_else(|e| panic!("[{relativo}] falha ao ler arquivo de referência: {e}"));

        let fonte = format!(
            "{corpo}\n\nfunction main(args: {{string}}): integer\n    local xs: {{integer}} = {{5, 3, 1, 4, 2}}\n    print(\"ok: \" .. #xs)\n    return 0\nend"
        );

        let out_dir = temp_dir(&format!(
            "titan-original-compila-{}",
            relativo.replace(['/', '.'], "-")
        ));
        let source_path = write_source(&out_dir, "caso.titan", &fonte);

        let compile_output = Command::new(titanc_bin())
            .arg("--out")
            .arg(&out_dir)
            .arg(&source_path)
            .output()
            .unwrap_or_else(|e| panic!("[{relativo}] falha ao invocar titanc: {e}"));
        assert_never_panics(&compile_output);
        assert!(
            compile_output.status.success(),
            "[{relativo}] esperava compilar, titanc falhou: {}",
            String::from_utf8_lossy(&compile_output.stderr)
        );

        let binary_name = source_path.file_stem().unwrap().to_str().unwrap();
        let binary = out_dir.join(binary_name);
        assert!(
            binary.exists(),
            "[{relativo}] esperava executável em {binary:?}"
        );

        let run_output = Command::new(&binary)
            .output()
            .unwrap_or_else(|e| panic!("[{relativo}] falha ao executar binário: {e}"));
        assert_eq!(
            run_output.status.code(),
            Some(0),
            "[{relativo}] binário terminou com código diferente de 0"
        );

        let _ = std::fs::remove_dir_all(&out_dir);
    }
}

/// Um arquivo `.titan` do Titan original (com `foreign import`/records) é
/// exatamente o cenário citado no PRD.md (T5) como caso negativo de
/// "construção não suportada" — usamos um trecho representativo em vez do
/// arquivo real do Titan (que depende de módulos externos não relevantes
/// aqui). Desde a T29 o `record` em si é aceito pelo checker; a T73 abriu a
/// porta de FFI, mas com a grafia `foreign function` (ADR 0025) — a forma do
/// original, que nomeia um header C, segue recusada, e agora com uma
/// mensagem que aponta a grafia que existe em vez de só dizer "não
/// suportado".
#[test]
fn arquivo_com_foreign_import_do_original_produz_erro_que_aponta_a_grafia_nova() {
    let out_dir = temp_dir("foreign-import-record");
    let source = r#"foreign import stdio "stdio.h"

record Ponto
    x: integer
    y: integer
end

function main(args: {string}): integer
    return 0
end"#;
    let source_path = write_source(&out_dir, "titan_original.titan", source);

    let output = Command::new(titanc_bin())
        .arg("--out")
        .arg(&out_dir)
        .arg(&source_path)
        .output()
        .expect("invoca titanc");

    assert_never_panics(&output);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("foreign function"),
        "o erro devia apontar a grafia que existe: {stderr}"
    );

    let _ = std::fs::remove_dir_all(&out_dir);
}

/// T73, o critério de aceite ponta a ponta: um `.titan` que chama funções da
/// libc (`abs`, `strlen`, `getenv`) compila pelo `titanc` de verdade — cargo
/// incluso — e roda com os valores corretos.
///
/// `abs` prova o escalar; `strlen` prova a `string` na ida (`CString`);
/// `getenv` prova a `string` na volta (`ffi_string`). Nenhum crate extra
/// entra no `Cargo.toml` gerado: a libc já vem linkada com a std.
#[test]
fn t73_compila_e_executa_chamada_a_libc_por_foreign_function() {
    let out_dir = temp_dir("t73-ffi-libc");
    let source = r#"foreign function abs(n: integer): integer
foreign function strlen(s: string): integer
foreign function getenv(nome: string): string

function main(args: {string}): integer
    print("abs=" .. abs(-7))
    print("strlen=" .. strlen("titan"))
    print("var=" .. getenv("TITAN_T73"))
    return 0
end"#;
    let source_path = write_source(&out_dir, "ffi.titan", source);

    let compile_output = Command::new(titanc_bin())
        .arg("--out")
        .arg(&out_dir)
        .arg(&source_path)
        .output()
        .expect("invoca titanc");
    assert_never_panics(&compile_output);
    assert!(
        compile_output.status.success(),
        "titanc falhou ao compilar ffi.titan: {}",
        String::from_utf8_lossy(&compile_output.stderr)
    );

    let binary = out_dir.join("ffi");
    assert!(binary.exists(), "esperava executável em {binary:?}");

    let run_output = Command::new(&binary)
        .env("TITAN_T73", "ok")
        .output()
        .expect("executa ./ffi");
    assert_eq!(
        String::from_utf8_lossy(&run_output.stdout),
        "abs=7\nstrlen=5\nvar=ok\n"
    );
    assert_eq!(run_output.status.code(), Some(0));

    // A FFI não arrasta dependência nenhuma: a libc já vem com a std.
    let cargo_toml = std::fs::read_to_string(out_dir.join("build").join("ffi").join("Cargo.toml"))
        .expect("lê o Cargo.toml gerado");
    assert!(
        cargo_toml.contains("titan-runtime"),
        "Cargo.toml gerado deveria depender de titan-runtime:\n{cargo_toml}"
    );
    assert!(
        !cargo_toml.contains("libc"),
        "`foreign function` não deveria acrescentar o crate libc:\n{cargo_toml}"
    );

    let _ = std::fs::remove_dir_all(&out_dir);
}

/// O outro lado do critério de aceite: tipo composto na fronteira dá erro
/// claro em português, do checker — nunca do rustc, e nunca com panic.
#[test]
fn t73_tipo_composto_na_fronteira_produz_erro_claro_do_checker() {
    let out_dir = temp_dir("t73-fronteira-composta");
    let source = r#"record Ponto
    x: integer
    y: integer
end

foreign function dist(p: Ponto): float

function main(args: {string}): integer
    return 0
end"#;
    let source_path = write_source(&out_dir, "fronteira.titan", source);

    let output = Command::new(titanc_bin())
        .arg("--out")
        .arg(&out_dir)
        .arg(&source_path)
        .output()
        .expect("invoca titanc");

    assert_never_panics(&output);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("fronteira de FFI") && stderr.contains("Ponto"),
        "esperava erro de fronteira citando o record: {stderr}"
    );

    let _ = std::fs::remove_dir_all(&out_dir);
}

#[test]
fn arquivo_de_entrada_inexistente_produz_erro_claro_sem_panic() {
    let out_dir = temp_dir("arquivo-inexistente");

    let output = Command::new(titanc_bin())
        .arg("--out")
        .arg(&out_dir)
        .arg(out_dir.join("nao_existe.titan"))
        .output()
        .expect("invoca titanc");

    assert_never_panics(&output);
    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stderr).trim().is_empty());

    let _ = std::fs::remove_dir_all(&out_dir);
}

#[test]
fn nenhum_argumento_produz_uso_sem_panic() {
    let output = Command::new(titanc_bin())
        .output()
        .expect("invoca titanc sem argumentos");

    assert_never_panics(&output);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("uso:"));
}

// ---- T72: `import` com alias e `df:metodo()` ----------------------------

/// Execução real do alias (PRD.md, T72): `import texto as t` seguido de
/// `t.funcao(...)` compila e roda com o resultado correto. Usa `texto`, e
/// não `data`, exatamente para provar o alias ponta a ponta sem pagar o
/// build do Polars — o mecanismo do alias é o mesmo para qualquer
/// capability (o nome local chaveia a tabela de módulos; o nome real sai de
/// `Capability::titan_name`).
#[test]
fn compila_e_executa_import_com_alias() {
    let out_dir = temp_dir("import-com-alias");

    let source = concat!(
        "import texto as t\n\n",
        "function main(args: {string}): integer\n",
        "    local n: integer = t.para_inteiro(\"42\")\n",
        "    print(\"n=\" .. t.de_inteiro(n + 1))\n",
        "    print(\"sub=\" .. t.sub(\"abcdef\", 2, 4))\n",
        "    print(\"tam=\" .. t.tamanho(\"abcdef\"))\n",
        "    return 0\n",
        "end",
    );
    let source_path = write_source(&out_dir, "alias_exec.titan", source);

    let compile_output = Command::new(titanc_bin())
        .arg("--out")
        .arg(&out_dir)
        .arg(&source_path)
        .output()
        .expect("invoca titanc");
    assert_never_panics(&compile_output);
    assert!(
        compile_output.status.success(),
        "titanc falhou ao compilar `import texto as t`: {}",
        String::from_utf8_lossy(&compile_output.stderr)
    );

    let binary = out_dir.join("alias_exec");
    assert!(binary.exists(), "esperava executável em {binary:?}");

    let run_output = Command::new(&binary)
        .output()
        .expect("executa ./alias_exec");
    assert_eq!(
        String::from_utf8_lossy(&run_output.stdout),
        "n=43\nsub=bcd\ntam=6\n"
    );
    assert_eq!(run_output.status.code(), Some(0));

    let _ = std::fs::remove_dir_all(&out_dir);
}

/// O critério central da T72: `df:soma("valor")` produz **o mesmo
/// resultado** de `df.soma("valor")`. Provado no Rust gerado em vez de por
/// execução: as duas formas só existem sobre o módulo `data`, e comparar o
/// `--emit-rust` das duas prova a equivalência sem pagar o build do Polars
/// (mesmo precedente de `emit_rust_de_import_data_compila_sem_erro`).
///
/// O par de testes em `checker.rs`
/// (`metodo_com_dois_pontos_produz_o_mesmo_typedexp_que_com_ponto`) prova a
/// mesma igualdade um nível acima, no `TypedExp`.
#[test]
fn emit_rust_de_dois_pontos_e_identico_ao_de_ponto() {
    let out_dir = temp_dir("emit-rust-dois-pontos");

    let com_ponto = concat!(
        "import data\n\n",
        "function main(args: {string}): integer\n",
        "    local df: data.DataFrame = data.read_csv(\"v.csv\")\n",
        "    local total: float = df.soma(\"valor\")\n",
        "    return 0\n",
        "end",
    );
    let com_dois_pontos = com_ponto.replace("df.soma", "df:soma");

    let emitir = |nome: &str, fonte: &str| {
        let source_path = write_source(&out_dir, nome, fonte);
        let output = Command::new(titanc_bin())
            .arg("--emit-rust")
            .arg("--out")
            .arg(&out_dir)
            .arg(&source_path)
            .output()
            .expect("invoca titanc --emit-rust");
        assert_never_panics(&output);
        assert!(
            output.status.success(),
            "titanc --emit-rust falhou para {nome}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    };

    let rust_ponto = emitir("ponto.titan", com_ponto);
    let rust_dois_pontos = emitir("dois_pontos.titan", &com_dois_pontos);

    assert_eq!(
        rust_ponto, rust_dois_pontos,
        "`df:soma(...)` deve gerar exatamente o mesmo Rust que `df.soma(...)`"
    );
    // Sanidade: o Rust gerado é mesmo a chamada do método, não um stub vazio.
    assert!(
        rust_ponto.contains("titan_data::"),
        "Rust gerado: {rust_ponto}"
    );
    assert!(
        !out_dir.join("build").exists(),
        "--emit-rust não deveria gerar build/"
    );

    let _ = std::fs::remove_dir_all(&out_dir);
}

/// Um projeto multi-módulo no disco: `titan.toml` mais os `.titan` dados.
fn write_projeto(dir: &Path, arquivos: &[(&str, &str)]) {
    std::fs::create_dir_all(dir.join("src")).expect("cria src/ do projeto");
    for (nome, conteudo) in arquivos {
        std::fs::write(dir.join(nome), conteudo).expect("escreve arquivo do projeto");
    }
}

/// Critério de aceite da T82 pela CLI: um programa de três módulos — `main`
/// importa `parser`, `parser` importa `lexer` — compila a um **único**
/// executável e roda com a saída certa, com o `record` de `lexer`
/// atravessando os dois saltos e a `local function` ficando de fora.
///
/// A prova ponta a ponta da Parte B: manifesto (T79), grafo (T80), checagem
/// entre módulos (T81) e a emissão de um `mod` Rust por módulo Titan (T82),
/// tudo pela mesma linha de comando que o usuário digita.
#[test]
fn t82_programa_multi_modulo_compila_a_um_binario_e_roda() {
    let dir = temp_dir("t82-multi-modulo");
    write_projeto(
        &dir,
        &[
            (
                "src/lexer.titan",
                "record Token\n    linha: integer\nend\n\
                 local function interna(): integer\n    return 1\nend\n\
                 function novo(linha: integer): Token\n\
                 \x20   local t: Token = {linha = linha + interna()}\n    return t\nend\n",
            ),
            (
                "src/parser.titan",
                "import lexer\n\
                 function primeiro(): lexer.Token\n    return lexer.novo(7)\nend\n",
            ),
            (
                "src/main.titan",
                "import parser\n\
                 function main(args: {string}): integer\n\
                 \x20   local n: integer = parser.primeiro().linha\n\
                 \x20   print(\"linha: \" .. n)\n    return 0\nend\n",
            ),
            (
                "titan.toml",
                "[pacote]\nnome = \"prog\"\nprincipal = \"src/main.titan\"\n\n\
                 [modulos]\nlexer = \"src/lexer.titan\"\nparser = \"src/parser.titan\"\n",
            ),
        ],
    );

    // Primeiro o `--emit-rust`, que é o que o critério de aceite pede ver:
    // os `mod` e as referências qualificadas.
    let emitido = Command::new(titanc_bin())
        .arg("--manifesto")
        .arg(&dir)
        .arg("--emit-rust")
        .output()
        .expect("invoca titanc --emit-rust --manifesto");
    assert_never_panics(&emitido);
    let rust = String::from_utf8_lossy(&emitido.stdout);
    for modulo in ["pub mod lexer {", "pub mod parser {", "pub mod prog {"] {
        assert!(rust.contains(modulo), "faltou '{modulo}' no Rust:\n{rust}");
    }
    assert!(
        rust.contains("-> crate::lexer::Token"),
        "o tipo de outro módulo tem de sair qualificado:\n{rust}"
    );
    assert!(
        rust.contains("crate::lexer::titan_novo("),
        "a chamada entre módulos tem de sair qualificada:\n{rust}"
    );
    // A `local function` continua sem `pub` — dentro de um `mod`, isso passa
    // a ser privacidade de verdade, conferida pelo rustc.
    assert!(
        rust.contains("fn titan_interna()") && !rust.contains("pub fn titan_interna()"),
        "a `local function` não pode sair `pub`:\n{rust}"
    );

    // Depois a compilação de verdade: **um** executável, com os três
    // módulos dentro (decisão 5 da fase — um crate Cargo por programa).
    let output = Command::new(titanc_bin())
        .arg("--manifesto")
        .arg(&dir)
        .arg("--out")
        .arg(&dir)
        .output()
        .expect("invoca titanc --manifesto");
    assert_never_panics(&output);
    assert!(
        output.status.success(),
        "titanc falhou no programa de três módulos: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let binario = dir.join("prog");
    assert!(binario.exists(), "esperava executável em {binario:?}");
    let execucao = Command::new(&binario).output().expect("executa ./prog");
    assert_eq!(String::from_utf8_lossy(&execucao.stdout), "linha: 8\n");
    assert_eq!(execucao.status.code(), Some(0));

    let _ = std::fs::remove_dir_all(&dir);
}

/// O contraponto: chamar uma `local function` de outro módulo tem de dar
/// erro **claro**, dizendo a regra de visibilidade em vez de mandar procurar
/// um erro de digitação (PRD.md, T81).
#[test]
fn t81_local_function_de_outro_modulo_da_erro_claro_na_cli() {
    let dir = temp_dir("t81-local-function");
    write_projeto(
        &dir,
        &[
            (
                "src/lexer.titan",
                "local function interna(): integer\n    return 1\nend\n\
                 function publica(): integer\n    return interna()\nend\n",
            ),
            (
                "src/main.titan",
                "import lexer\n\
                 function main(args: {string}): integer\n\
                 \x20   local n: integer = lexer.interna()\n    return n\nend\n",
            ),
            (
                "titan.toml",
                "[pacote]\nnome = \"prog\"\nprincipal = \"src/main.titan\"\n\n\
                 [modulos]\nlexer = \"src/lexer.titan\"\n",
            ),
        ],
    );

    let output = Command::new(titanc_bin())
        .arg("--manifesto")
        .arg(&dir)
        .arg("--out")
        .arg(&dir)
        .output()
        .expect("invoca titanc --manifesto");
    assert_never_panics(&output);
    assert!(!output.status.success(), "a compilação deveria falhar");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("é uma `local function` do módulo 'lexer'"),
        "mensagem inesperada: {stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// O `import` que não resolve contra fonte nenhuma lista as **duas** fontes
/// — as capabilities do compilador e os módulos do manifesto (PRD.md, T81).
#[test]
fn t81_import_desconhecido_lista_capabilities_e_modulos_do_manifesto() {
    let dir = temp_dir("t81-import-sumido");
    write_projeto(
        &dir,
        &[
            (
                "src/lexer.titan",
                "function novo(): integer\n    return 0\nend\n",
            ),
            (
                "src/main.titan",
                "import sumido\n\
                 function main(args: {string}): integer\n    return 0\nend\n",
            ),
            (
                "titan.toml",
                "[pacote]\nnome = \"prog\"\nprincipal = \"src/main.titan\"\n\n\
                 [modulos]\nlexer = \"src/lexer.titan\"\n",
            ),
        ],
    );

    let output = Command::new(titanc_bin())
        .arg("--manifesto")
        .arg(&dir)
        .arg("--out")
        .arg(&dir)
        .output()
        .expect("invoca titanc --manifesto");
    assert_never_panics(&output);
    assert!(!output.status.success(), "a compilação deveria falhar");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("capabilities: data, texto, io"),
        "faltou listar as capabilities: {stderr}"
    );
    assert!(
        stderr.contains("declarados no manifesto: lexer"),
        "faltou listar os módulos do manifesto: {stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Diretório do projeto auto-hospedado (`selfhost/`), que a Parte C preenche
/// módulo a módulo — ao contrário dos projetos das T81/T82, este mora no
/// repositório, e não num diretório temporário.
fn selfhost_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../selfhost")
}

/// Critério de aceite da T84: `selfhost/ast.titan` — a AST do Titan escrita em
/// Titan — compila como módulo do projeto `selfhost/`, e um programa constrói
/// uma `Exp` **recursiva** (`1 + 2 * 3`) e a percorre com `match`.
///
/// É a prova de que os tipos soma da Parte A (T74-T77) resolveram o problema
/// que o ADR 0020 registrou: `ExpBinop(Loc, string, Exp, Exp)` carrega os dois
/// operandos como `Exp` de verdade, sem a tag inteira de `examples/lexer.titan`
/// e sem record gordo. O `Box` que fecha a recursão em Rust (ADR 0026) é do
/// codegen — não aparece no fonte Titan.
#[test]
fn t84_selfhost_ast_titan_compila_e_percorre_exp_recursiva_com_match() {
    let out_dir = temp_dir("t84-selfhost-ast");

    // Primeiro o `--emit-rust`: o que interessa ver é a recursão encaixotada
    // pelo codegen e o `enum` saindo como `enum` Rust, dentro do `mod ast`.
    let emitido = Command::new(titanc_bin())
        .arg("--manifesto")
        .arg(selfhost_dir())
        .arg("--emit-rust")
        .output()
        .expect("invoca titanc --emit-rust --manifesto selfhost");
    assert_never_panics(&emitido);
    assert!(
        emitido.status.success(),
        "titanc falhou ao emitir o projeto selfhost: {}",
        String::from_utf8_lossy(&emitido.stderr)
    );
    let rust = String::from_utf8_lossy(&emitido.stdout);
    assert!(
        rust.contains("pub mod ast {"),
        "faltou o módulo ast:\n{rust}"
    );
    // A variante recursiva, com o `Box` posto pela emissão (ADR 0026) — a
    // prova de que `Exp` é recursivo de verdade, e não uma tag mais campos.
    assert!(
        rust.contains("ExpBinop(Loc, String, Box<Exp>, Box<Exp>)"),
        "esperava a variante recursiva encaixotada:\n{rust}"
    );
    // `Loc` continua `record` → `struct`: tipo soma não é martelo universal.
    assert!(
        rust.contains("pub struct Loc {"),
        "Loc deveria continuar um record:\n{rust}"
    );
    // E a travessia do outro módulo chega qualificada.
    assert!(
        rust.contains("crate::ast::titan_exp_para_texto("),
        "a travessia entre módulos tem de sair qualificada:\n{rust}"
    );

    // Depois a compilação de verdade e a execução: a `Exp` de `1 + 2 * 3`
    // montada com os construtores da ast e percorrida por `match`.
    let output = Command::new(titanc_bin())
        .arg("--manifesto")
        .arg(selfhost_dir())
        .arg("--out")
        .arg(&out_dir)
        .output()
        .expect("invoca titanc --manifesto selfhost");
    assert_never_panics(&output);
    assert!(
        output.status.success(),
        "titanc falhou ao compilar o projeto selfhost: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let binario = out_dir.join("titanself");
    assert!(binario.exists(), "esperava executável em {binario:?}");
    let execucao = Command::new(&binario)
        .output()
        .expect("executa ./titanself");
    // A árvore reparentizada prova a precedência na *forma* da árvore (o `*`
    // é filho do `+`, não irmão); `nos: 5` prova que a travessia recursiva
    // desceu nos dois lados do `ExpBinop`.
    assert_eq!(
        String::from_utf8_lossy(&execucao.stdout),
        "arvore: (1 + (2 * 3))\nnos: 5\nlinha: 1\n"
    );
    assert_eq!(execucao.status.code(), Some(0));

    let _ = std::fs::remove_dir_all(&out_dir);
}

/// O contraste que a T84 pede medir: `examples/lexer.titan` — o registro
/// histórico que o ADR 0020 cita — continua **intocado**, com a tag inteira e
/// as constantes-como-função que a Fase 5 tornou desnecessárias. Apagá-lo
/// destruiria a evidência empírica que justifica esta fase (T85).
#[test]
fn t84_lexer_titan_da_fase_4_continua_com_o_estilo_antigo_como_registro() {
    let antigo = std::fs::read_to_string(examples_dir().join("lexer.titan"))
        .expect("lê examples/lexer.titan");
    assert!(
        antigo.contains("function TK_NAME(): integer return 1 end"),
        "o registro histórico da Fase 4 não pode ser 'consertado'"
    );
    assert!(
        !antigo.contains("enum "),
        "examples/lexer.titan tem de permanecer sem tipo soma"
    );

    // E o novo faz o oposto: `enum` de verdade, com a variante recursiva
    // escrita sem nenhum encaixotamento à vista.
    let novo =
        std::fs::read_to_string(selfhost_dir().join("ast.titan")).expect("lê selfhost/ast.titan");
    assert!(
        novo.contains("ExpBinop(Loc, string, Exp, Exp)"),
        "a variante recursiva é o resultado mensurável da fase"
    );
}
