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
//!   escopo após arrays/records/maps serem aceitos — retornos múltiplos,
//!   métodos, `import`, `repeat`, `break`, bitwise, `//`, `Option`, `as`,
//!   multi-assign e as regras de tipos de record/map — continua rejeitado
//!   com erro claro. `v[i]`, `{...}` e `#` saíram desta lista: têm suporte
//!   real no codegen desde a T30;
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
//!   atribuído, `import ... as ...`, `import` como expressão e método com
//!   dois-pontos — é rejeitado com erro claro, sem pagar o build do Polars
//!   (`--emit-rust` em todo caso negativo);
//! - a prova ponta a ponta da Fase 3 (PRD.md, T45): compila e executa
//!   `examples/dados.titan` — único caminho feliz desta suíte que paga o
//!   build do Polars de propósito — conferindo stdout completo e exit code.

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
/// `indexacao_de_array`, `construtor_de_array` e `operador_length` saíram
/// desta tabela na T30/T31: viraram casos positivos (arrays têm suporte real
/// no codegen). `chamada_de_metodo` e `tipo_option` continuam rejeitados,
/// mas por outra camada: com `.` e `[` lexados e o parser sabendo indexação,
/// a rejeição de `chamada_de_metodo` já não vem do lexer, e sim do parser não
/// reconhecer `:` como início de chamada de método.
const CASOS_FORA_DE_ESCOPO_FASE_2: &[CasoNegativo] = &[
    CasoNegativo {
        nome: "retornos_multiplos",
        fonte: "function main(args: {string}): integer\n    return 1, 2\nend",
        trecho_esperado: "erro de sintaxe",
    },
    CasoNegativo {
        nome: "chamada_de_metodo",
        // Com `.` lexado e sufixos de acesso a campo suportados desde a T23,
        // a rejeição já não vem do lexer: o parser reconhece `ponto` e para
        // ao encontrar `:`, que não inicia nem sufixo nem expressão válida.
        fonte: "function main(args: {string}): integer\n    local p = ponto:dist()\n    return 0\nend",
        trecho_esperado: "Esperava um nome ou '(' seguido de expressão",
    },
    CasoNegativo {
        nome: "repeat_until",
        fonte: "function main(args: {string}): integer\n    repeat print(\"x\") until true\n    return 0\nend",
        trecho_esperado: "Esperava um comando",
    },
    CasoNegativo {
        nome: "break_fora_de_escopo",
        fonte: "function main(args: {string}): integer\n    while true do\n        break\n    end\n    return 0\nend",
        trecho_esperado: "Esperava um comando",
    },
    CasoNegativo {
        nome: "bitwise_and",
        fonte: "function main(args: {string}): integer\n    local a = 1 & 2\n    return 0\nend",
        trecho_esperado: "caractere inesperado '&'",
    },
    CasoNegativo {
        nome: "bitwise_or",
        fonte: "function main(args: {string}): integer\n    local a = 1 | 2\n    return 0\nend",
        trecho_esperado: "caractere inesperado '|'",
    },
    CasoNegativo {
        nome: "bitwise_not_isolado",
        fonte: "function main(args: {string}): integer\n    local a = ~2\n    return 0\nend",
        trecho_esperado: "'~' isolado",
    },
    CasoNegativo {
        nome: "shift_esquerda",
        fonte: "function main(args: {string}): integer\n    local a = 1 << 2\n    return 0\nend",
        trecho_esperado: "Esperava uma expressão",
    },
    CasoNegativo {
        nome: "shift_direita",
        fonte: "function main(args: {string}): integer\n    local a = 1 >> 2\n    return 0\nend",
        trecho_esperado: "Esperava uma expressão",
    },
    CasoNegativo {
        nome: "divisao_inteira",
        fonte: "function main(args: {string}): integer\n    local a = 1 // 2\n    return 0\nend",
        trecho_esperado: "Esperava uma expressão",
    },
    CasoNegativo {
        nome: "tipo_option",
        fonte: "function main(args: {string}): integer\n    local a: integer? = nil\n    return 0\nend",
        trecho_esperado: "caractere inesperado '?'",
    },
    CasoNegativo {
        nome: "cast_as",
        fonte: "function main(args: {string}): integer\n    local a = 1 as float\n    return 0\nend",
        trecho_esperado: "erro de sintaxe",
    },
    CasoNegativo {
        nome: "metodo_com_dois_pontos",
        fonte: "function main(args: {string}): integer\n    args:foo()\n    return 0\nend",
        trecho_esperado: "Esperava um comando",
    },
    CasoNegativo {
        nome: "multi_assign",
        fonte: "function main(args: {string}): integer\n    local a: integer = 1\n    local b: integer = 2\n    a, b = b, a\n    return 0\nend",
        trecho_esperado: "Esperava um comando",
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

/// Fora de escopo da Fase 3 (PRD.md, T44): a Fase 3 ensinou o pipeline a
/// aceitar `import data` e as duas formas de chamada de capability
/// (`data.f(...)` e `df.f(...)`) — esta tabela garante que ela não afrouxou
/// nada além do pretendido. Todos os casos falham no checker ou no parser,
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
        nome: "import_data_as_d",
        fonte: "import data as d\n\nfunction main(args: {string}): integer\n    return 0\nend",
        trecho_esperado: "'import ... as ...' não é suportado",
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
    CasoNegativo {
        nome: "metodo_com_dois_pontos_em_df",
        fonte: "import data\n\nfunction main(args: {string}): integer\n    local df: data.DataFrame = data.read_csv(\"v.csv\")\n    local total: float = df:soma(\"valor\")\n    return 0\nend",
        trecho_esperado: "Esperava um nome ou '(' seguido de expressão",
    },
];

#[test]
fn construcoes_fora_de_escopo_da_fase_3_produzem_erro_claro_sem_panic() {
    for caso in CASOS_FORA_DE_ESCOPO_FASE_3 {
        verifica_caso_negativo(caso, "fora-de-escopo-fase-3");
    }
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
/// aqui). Desde a T29 o `record` em si é aceito pelo checker; `foreign
/// import` segue fora de escopo e garante a falha aqui.
#[test]
fn arquivo_com_foreign_import_e_record_produz_erro_de_construcao_nao_suportada() {
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
    assert!(!stderr.trim().is_empty());

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
