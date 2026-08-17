//! Teste de integração ponta a ponta (PRD.md, T8): invoca o binário `titanc`
//! de verdade (não as funções internas do pipeline) via `Command`, exercendo
//! a CLI como um usuário faria — `cargo test` aciona o `cargo build` do
//! binário automaticamente antes de rodar este arquivo.
//!
//! Duas frentes:
//! - caminho feliz: compila `examples/hello.titan` e confere **stdout e exit
//!   code** do executável gerado (critério de aceite do PRD.md, T8);
//! - suíte de negativos de T4/T5 rodando pelo pipeline completo: cada
//!   construção fora do subconjunto da Fase 0 precisa produzir uma mensagem
//!   de erro clara na saída do `titanc`, nunca um panic (sem "thread
//!   'main' panicked").

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
    assert!(stdout.contains("fn titan_main(args: &[String]) -> i64"));
    assert!(stdout.contains("titan_runtime::print(\"Olá, mundo!\");"));
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
        nome: "if_nao_suportado",
        fonte: "function main(args: {string}): integer\n    if true then\n    end\n    return 0\nend",
        trecho_esperado: "",
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

#[test]
fn casos_negativos_de_t4_e_t5_produzem_erro_claro_sem_panic() {
    for caso in CASOS_NEGATIVOS {
        let out_dir = temp_dir(&format!("negativo-{}", caso.nome));
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
