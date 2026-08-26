//! Teste de integração ponta a ponta (PRD.md, T8): invoca o binário `titanc`
//! de verdade (não as funções internas do pipeline) via `Command`, exercendo
//! a CLI como um usuário faria — `cargo test` aciona o `cargo build` do
//! binário automaticamente antes de rodar este arquivo.
//!
//! Três frentes:
//! - caminho feliz: compila `examples/hello.titan` e confere **stdout e exit
//!   code** do executável gerado (critério de aceite do PRD.md, T8);
//! - caminho feliz da Fase 1 (PRD.md, T17): compila `examples/nucleo.titan`
//!   — aritmética, `if`, `while`, `for` e atribuição em funções reais
//!   (fatorial e fibonacci) — e confere stdout completo e exit code;
//! - suíte de negativos de T4/T5 rodando pelo pipeline completo: cada
//!   construção fora do subconjunto da Fase 0 precisa produzir uma mensagem
//!   de erro clara na saída do `titanc`, nunca um panic (sem "thread
//!   'main' panicked");
//! - suíte consolidada da Fase 1 (PRD.md, T16): tudo que **segue** fora de
//!   escopo após `if`/`while`/`for`/atribuição/operadores serem aceitos —
//!   `v[i]`, `{...}`, retornos múltiplos, métodos, `import`, `repeat`,
//!   `break`, bitwise, `//`, `#`, `Option` — continua rejeitado com erro
//!   claro, incluindo arquivos `.titan` reais do Titan original.

use std::path::{Path, PathBuf};
use std::process::Command;

fn titanc_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_titanc"))
}

fn examples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples")
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

/// Fora de escopo da Fase 1 (PRD.md, T16): a fase ensinou o pipeline a
/// aceitar `if`/`while`/`for`/atribuição e operadores — esta tabela garante
/// que ela não afrouxou nada além do pretendido. Cada construção que segue
/// fora do subconjunto é rejeitada em alguma etapa (léxica, sintática ou de
/// tipos) com erro claro, nunca panic.
const CASOS_FORA_DE_ESCOPO_FASE_1: &[CasoNegativo] = &[
    CasoNegativo {
        nome: "indexacao_de_array",
        fonte: "function main(args: {string}): integer\n    print(args[1])\n    return 0\nend",
        // Desde a T23 (PRD.md), o parser tem sufixo de indexação (`v[i]` vira
        // `VarBracket`) — a rejeição deixou de ser sintática. O checker desta
        // fase ainda não sabe verificar indexação (isso é T25+/T29), então o
        // erro agora vem dele.
        trecho_esperado: "indexação",
    },
    CasoNegativo {
        nome: "construtor_de_array",
        fonte: "function main(args: {string}): integer\n    local t = {1, 2}\n    return 0\nend",
        // Desde a T28 (PRD.md), o parser produz `ExpInitList` para `{...}` —
        // a rejeição deixou de ser sintática. O checker desta fase ainda não
        // sabe tipar arrays/records/maps (isso é T29), então o erro agora
        // vem dele.
        trecho_esperado: "inicializador de array/record",
    },
    CasoNegativo {
        nome: "retornos_multiplos",
        fonte: "function main(args: {string}): integer\n    return 1, 2\nend",
        trecho_esperado: "erro de sintaxe",
    },
    CasoNegativo {
        nome: "chamada_de_metodo",
        fonte: "function main(args: {string}): integer\n    local p = ponto:dist()\n    return 0\nend",
        trecho_esperado: "erro de sintaxe",
    },
    CasoNegativo {
        nome: "import_de_modulo",
        fonte: "local m = import \"foo\"\n\nfunction main(args: {string}): integer\n    return 0\nend",
        trecho_esperado: "declaração de topo",
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
        nome: "operador_length",
        fonte: "function main(args: {string}): integer\n    local a = #args\n    return 0\nend",
        // Desde a T20 (PRD.md), `#` é um token válido (`Hash`) — a rejeição
        // deixou de ser léxica. O operador `#` unário é T25 em diante; por ora
        // o parser não sabe iniciar uma expressão com `#` e produz este erro.
        trecho_esperado: "Esperava uma expressão",
    },
    CasoNegativo {
        nome: "tipo_option",
        fonte: "function main(args: {string}): integer\n    local a: integer? = nil\n    return 0\nend",
        trecho_esperado: "caractere inesperado '?'",
    },
];

#[test]
fn construcoes_fora_de_escopo_da_fase_1_produzem_erro_claro_sem_panic() {
    for caso in CASOS_FORA_DE_ESCOPO_FASE_1 {
        verifica_caso_negativo(caso, "fora-de-escopo");
    }
}

/// Regressão com arquivos `.titan` **reais** do Titan original (PRD.md,
/// T16) — a referência somente leitura em `../titan/`. Eles usam `import`,
/// `#`, `v[i]`, `{}` e records, tudo fora do subconjunto: o titanc precisa
/// rejeitá-los com erro claro, nunca panic.
#[test]
fn arquivos_reais_do_titan_original_produzem_erro_claro_sem_panic() {
    let titan_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../titan");
    if !titan_dir.exists() {
        // A referência é um repositório de terceiros fora deste workspace;
        // sem ela presente não há o que exercitar.
        eprintln!("aviso: {} ausente — regressão pulada", titan_dir.display());
        return;
    }

    for relativo in ["examples/artisanal.titan", "testfiles/sieve.titan"] {
        let source_path = titan_dir.join(relativo);
        assert!(
            source_path.exists(),
            "esperava arquivo de referência em {source_path:?}"
        );
        let out_dir = temp_dir(&format!(
            "titan-original-{}",
            relativo.replace('/', "-").replace('.', "-")
        ));

        let output = Command::new(titanc_bin())
            .arg("--out")
            .arg(&out_dir)
            .arg(&source_path)
            .output()
            .unwrap_or_else(|e| panic!("[{relativo}] falha ao invocar titanc: {e}"));

        assert_never_panics(&output);
        assert!(
            !output.status.success(),
            "[{relativo}] esperava falha, titanc reportou sucesso"
        );
        assert!(
            !String::from_utf8_lossy(&output.stderr).trim().is_empty(),
            "[{relativo}] esperava mensagem de erro em stderr, veio vazio"
        );

        let _ = std::fs::remove_dir_all(&out_dir);
    }
}

/// Um arquivo `.titan` do Titan original (com `foreign import`/records) é
/// exatamente o cenário citado no PRD.md (T5) como caso negativo de
/// "construção não suportada" — usamos um trecho representativo em vez do
/// arquivo real do Titan (que depende de módulos externos não relevantes
/// aqui).
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
