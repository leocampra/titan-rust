//! Testes de integração do IO Runtime (PRD.md, T54).

fn temp_path(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "titan-io-test-{label}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ))
}

#[test]
fn ler_arquivo_le_o_conteudo_de_um_arquivo_existente() {
    let path = temp_path("ler-existente");
    std::fs::write(&path, "conteudo de teste\n").expect("escreve arquivo de teste");

    let conteudo = titan_io::ler_arquivo(path.to_str().expect("caminho utf-8"));
    assert_eq!(conteudo, "conteudo de teste\n");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn ler_arquivo_inexistente_devolve_erro_em_portugues() {
    let path = temp_path("ler-inexistente");
    let _ = std::fs::remove_file(&path);

    let erro = titan_io::ler_arquivo_checked(path.to_str().expect("caminho utf-8"))
        .expect_err("arquivo inexistente deveria falhar");
    assert!(erro.contains("não foi possível ler"), "erro: {erro}");
}

#[test]
fn escrever_arquivo_grava_o_conteudo_no_disco() {
    let path = temp_path("escrever");
    let _ = std::fs::remove_file(&path);

    titan_io::escrever_arquivo(path.to_str().expect("caminho utf-8"), "novo conteudo");
    let lido = std::fs::read_to_string(&path).expect("le arquivo escrito");
    assert_eq!(lido, "novo conteudo");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn escrever_arquivo_em_diretorio_inexistente_devolve_erro_em_portugues() {
    let path = temp_path("escrever-dir-inexistente")
        .join("subdir")
        .join("arquivo.txt");

    let erro = titan_io::escrever_arquivo_checked(path.to_str().expect("caminho utf-8"), "x")
        .expect_err("diretório inexistente deveria falhar");
    assert!(erro.contains("não foi possível escrever"), "erro: {erro}");
}
