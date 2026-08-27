//! Testes de integração do Data Runtime (PRD.md, T41), sobre o CSV de
//! fixture `tests/fixtures/vendas.csv` (colunas `cidade: string`,
//! `valor: integer`, `preco: float`).

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/vendas.csv");

#[test]
fn le_csv_e_conta_linhas_e_colunas() {
    let mut df = titan_data::read_csv(FIXTURE);
    assert_eq!(titan_data::linhas(&mut df), 3);
    assert_eq!(
        titan_data::colunas(&mut df),
        vec!["cidade".to_string(), "valor".to_string(), "preco".to_string()]
    );
}

#[test]
fn extrai_coluna_integer() {
    let mut df = titan_data::read_csv(FIXTURE);
    assert_eq!(titan_data::coluna_integer(&mut df, "valor"), vec![10, 20, 5]);
}

#[test]
fn extrai_coluna_float() {
    let mut df = titan_data::read_csv(FIXTURE);
    assert_eq!(titan_data::coluna_float(&mut df, "preco"), vec![1.5, 2.5, 3.0]);
}

#[test]
fn agregacoes_devolvem_f64() {
    let mut df = titan_data::read_csv(FIXTURE);
    assert_eq!(titan_data::soma(&mut df, "valor"), 35.0);
    assert!((titan_data::media(&mut df, "valor") - 11.666666666666666).abs() < 1e-9);
    assert_eq!(titan_data::minimo(&mut df, "valor"), 5.0);
    assert_eq!(titan_data::maximo(&mut df, "valor"), 20.0);
}

#[test]
fn agregacoes_sobre_coluna_float() {
    let mut df = titan_data::read_csv(FIXTURE);
    assert_eq!(titan_data::soma(&mut df, "preco"), 7.0);
    assert_eq!(titan_data::minimo(&mut df, "preco"), 1.5);
    assert_eq!(titan_data::maximo(&mut df, "preco"), 3.0);
}

// --- Os três casos de erro exigidos pelo critério de aceite da T41 ---

#[test]
fn arquivo_inexistente_devolve_erro_em_portugues() {
    let erro = titan_data::read_csv_checked("caminho/que/nao/existe.csv")
        .expect_err("arquivo inexistente deveria falhar");
    assert!(erro.contains("caminho/que/nao/existe.csv"), "erro: {erro}");
}

#[test]
fn coluna_inexistente_devolve_erro_em_portugues() {
    let mut df = titan_data::read_csv(FIXTURE);
    let erro = titan_data::soma_checked(&mut df, "coluna_fantasma")
        .expect_err("coluna inexistente deveria falhar");
    assert!(erro.contains("coluna_fantasma"), "erro: {erro}");
}

#[test]
fn coluna_de_tipo_errado_devolve_erro_em_portugues() {
    let mut df = titan_data::read_csv(FIXTURE);
    let erro = titan_data::coluna_integer_checked(&mut df, "cidade")
        .expect_err("coluna de string não é integer");
    assert!(erro.contains("cidade"), "erro: {erro}");

    let erro = titan_data::coluna_float_checked(&mut df, "cidade")
        .expect_err("coluna de string não é float");
    assert!(erro.contains("cidade"), "erro: {erro}");
}
