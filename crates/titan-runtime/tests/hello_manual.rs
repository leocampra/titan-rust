//! Verifica o critério de aceite da T0: o programa escrito à mão em
//! `examples/hello_manual.rs` compila, imprime `Olá, mundo!` e sai com 0.

use std::path::PathBuf;
use std::process::Command;

/// Compila `examples/hello_manual.rs` e devolve o caminho do executável.
///
/// O binário do próprio teste vive em `<target>/<perfil>/deps/`; os exemplos
/// ficam em `<target>/<perfil>/examples/`, então o caminho é derivado daí — sem
/// depender do perfil de compilação em uso.
fn compilar_exemplo() -> PathBuf {
    let saida = Command::new(env!("CARGO"))
        .args([
            "build",
            "--quiet",
            "-p",
            "titan-runtime",
            "--example",
            "hello_manual",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("falha ao invocar o cargo");

    assert!(
        saida.status.success(),
        "cargo build do exemplo falhou:\n{}",
        String::from_utf8_lossy(&saida.stderr)
    );

    let mut caminho = std::env::current_exe().expect("caminho do binário de teste");
    caminho.pop(); // .../deps
    if caminho.file_name().is_some_and(|nome| nome == "deps") {
        caminho.pop();
    }
    caminho.push("examples");
    caminho.push(if cfg!(windows) {
        "hello_manual.exe"
    } else {
        "hello_manual"
    });
    caminho
}

#[test]
fn hello_manual_imprime_e_sai_com_zero() {
    let executavel = compilar_exemplo();
    let saida = Command::new(&executavel)
        .output()
        .unwrap_or_else(|e| panic!("falha ao executar {}: {e}", executavel.display()));

    assert_eq!(String::from_utf8_lossy(&saida.stdout), "Olá, mundo!\n");
    assert_eq!(saida.status.code(), Some(0));
}
