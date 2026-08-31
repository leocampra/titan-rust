//! Testes de integração do Texto Runtime (PRD.md, T53).

#[test]
fn byte_le_o_byte_no_indice_1_based() {
    assert_eq!(titan_texto::byte("abc", 1), b'a' as i64);
    assert_eq!(titan_texto::byte("abc", 3), b'c' as i64);
}

#[test]
fn byte_indice_fora_da_faixa_devolve_erro_em_portugues() {
    let erro = titan_texto::byte_checked("abc", 0).expect_err("índice 0 deveria falhar");
    assert!(erro.contains('0'), "erro: {erro}");

    let erro = titan_texto::byte_checked("abc", -1).expect_err("índice negativo deveria falhar");
    assert!(erro.contains("-1"), "erro: {erro}");

    let erro = titan_texto::byte_checked("abc", 4).expect_err("índice além do fim deveria falhar");
    assert!(erro.contains('4'), "erro: {erro}");
}

#[test]
fn sub_fatia_com_i_e_j_inclusivos() {
    assert_eq!(titan_texto::sub("hello world", 1, 5), "hello");
    assert_eq!(titan_texto::sub("hello world", 7, 11), "world");
    assert_eq!(titan_texto::sub("hello world", 1, 11), "hello world");
}

#[test]
fn sub_j_alem_do_fim_trunca_no_fim_da_string() {
    assert_eq!(titan_texto::sub("abc", 1, 99), "abc");
}

#[test]
fn sub_j_menor_que_i_devolve_string_vazia() {
    assert_eq!(titan_texto::sub("abc", 3, 1), "");
}

#[test]
fn sub_indice_invalido_devolve_erro_em_portugues() {
    let erro = titan_texto::sub_checked("abc", 0, 2).expect_err("índice 0 deveria falhar");
    assert!(erro.contains('0'), "erro: {erro}");

    let erro = titan_texto::sub_checked("abc", 5, 6)
        .expect_err("índice inicial fora da faixa deveria falhar");
    assert!(erro.contains('5'), "erro: {erro}");
}

#[test]
fn para_inteiro_converte_string_valida() {
    assert_eq!(titan_texto::para_inteiro("42"), 42);
    assert_eq!(titan_texto::para_inteiro("-7"), -7);
}

#[test]
fn para_inteiro_string_invalida_devolve_erro_em_portugues() {
    let erro = titan_texto::para_inteiro_checked("abc").expect_err("'abc' não é inteiro");
    assert!(erro.contains("abc"), "erro: {erro}");

    let erro = titan_texto::para_inteiro_checked("").expect_err("string vazia não é inteiro");
    assert!(!erro.is_empty());
}

#[test]
fn de_inteiro_converte_para_string() {
    assert_eq!(titan_texto::de_inteiro(42), "42");
    assert_eq!(titan_texto::de_inteiro(-7), "-7");
    assert_eq!(titan_texto::de_inteiro(0), "0");
}

#[test]
fn tamanho_conta_bytes_como_o_operador_length() {
    assert_eq!(titan_texto::tamanho("abc"), 3);
    assert_eq!(titan_texto::tamanho(""), 0);
    // "á" em UTF-8 são 2 bytes — a limitação documentada: bytes, não
    // caracteres.
    assert_eq!(titan_texto::tamanho("á"), 2);
}
