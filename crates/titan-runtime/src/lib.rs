//! Runtime da linguagem Titan.
//!
//! Este crate reúne as funções que o código Rust gerado pelo `titanc` chama em
//! tempo de execução. Na Fase 0 eram apenas duas: `print` (que o Titan original
//! não possui — aqui ela vem da stdlib, não é palavra-chave) e `concat`, que dá
//! suporte ao operador `..`. A Fase 2 acrescenta a superfície de arrays,
//! records e maps: indexação checada, sem nunca expor o `panic!` cru do Rust.
//!
//! Arrays em Titan são **1-based** (`coder.lua:1994`); a conversão para o
//! 0-based do `Vec` acontece só aqui dentro, num lugar só.

/// Escreve `s` na saída padrão seguido de uma quebra de linha.
///
/// Equivale ao `print` da stdlib do Titan: recebe uma `string` e devolve `nil`.
///
/// ```
/// titan_runtime::print("Olá, mundo!");
/// ```
pub fn print(s: &str) {
    println!("{s}");
}

/// Concatena duas strings, implementando o operador `..` do Titan.
///
/// ```
/// assert_eq!(titan_runtime::concat("Olá, ", "mundo!"), "Olá, mundo!");
/// ```
pub fn concat(a: &str, b: &str) -> String {
    let mut out = String::with_capacity(a.len() + b.len());
    out.push_str(a);
    out.push_str(b);
    out
}

/// Aborta a execução com uma mensagem em português, sem `panic!` cru.
///
/// Nunca retorna: imprime em `stderr` e encerra o processo com código 1. É a
/// única forma de erro fatal em tempo de execução do Titan — nunca
/// `thread 'main' panicked`, nunca backtrace.
fn abortar(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(1);
}

/// Lê `v[indice]` (1-based) checando a faixa; devolve o erro em português em
/// vez de abortar. Base para [`array_get`].
pub fn array_get_checked<T: Clone>(v: &[T], indice: i64) -> Result<T, String> {
    if indice == 0 {
        return Err("índice 0 inválido: arrays em Titan começam em 1".to_string());
    }
    if indice < 0 || (indice as usize) > v.len() {
        return Err(format!(
            "índice {indice} fora da faixa (array tem {} elementos)",
            v.len()
        ));
    }
    Ok(v[(indice - 1) as usize].clone())
}

/// Lê `v[indice]` (1-based). Aborta com mensagem em português se `indice` for
/// 0, negativo ou além do fim do array.
pub fn array_get<T: Clone>(v: &[T], indice: i64) -> T {
    match array_get_checked(v, indice) {
        Ok(val) => val,
        Err(msg) => abortar(&msg),
    }
}

/// Referência mutável a `v[indice]` (1-based) checando a faixa. Base para
/// [`array_get_mut`].
pub fn array_get_mut_checked<T>(v: &mut [T], indice: i64) -> Result<&mut T, String> {
    if indice == 0 {
        return Err("índice 0 inválido: arrays em Titan começam em 1".to_string());
    }
    if indice < 0 || (indice as usize) > v.len() {
        return Err(format!(
            "índice {indice} fora da faixa (array tem {} elementos)",
            v.len()
        ));
    }
    Ok(&mut v[(indice - 1) as usize])
}

/// Referência mutável a `v[indice]` (1-based). Aborta com mensagem em
/// português se `indice` for 0, negativo ou além do fim do array.
pub fn array_get_mut<T>(v: &mut [T], indice: i64) -> &mut T {
    match array_get_mut_checked(v, indice) {
        Ok(val) => val,
        Err(msg) => abortar(&msg),
    }
}

/// Escreve `v[indice] = valor` (1-based) checando a faixa; devolve o erro em
/// português em vez de abortar. Base para [`array_set`].
///
/// Implementa a decisão 5 do plano: escreve em `1..#v`, faz **push** em
/// `#v + 1`, rejeita o resto.
pub fn array_set_checked<T>(v: &mut Vec<T>, indice: i64, valor: T) -> Result<(), String> {
    if indice == 0 {
        return Err("índice 0 inválido: arrays em Titan começam em 1".to_string());
    }
    if indice < 0 || (indice as usize) > v.len() + 1 {
        return Err(format!(
            "índice {indice} fora da faixa (array tem {} elementos; só é possível escrever em \
             1..{} ou fazer append em {})",
            v.len(),
            v.len(),
            v.len() + 1
        ));
    }
    if indice as usize == v.len() + 1 {
        v.push(valor);
    } else {
        v[(indice - 1) as usize] = valor;
    }
    Ok(())
}

/// Escreve `v[indice] = valor` (1-based). Aborta com mensagem em português se
/// `indice` for 0, negativo ou for além de `#v + 1`.
pub fn array_set<T>(v: &mut Vec<T>, indice: i64, valor: T) {
    if let Err(msg) = array_set_checked(v, indice, valor) {
        abortar(&msg);
    }
}

/// Tamanho do array — implementa o operador `#` do Titan sobre arrays.
pub fn array_len<T>(v: &[T]) -> i64 {
    v.len() as i64
}

/// Tamanho da string em bytes — implementa o operador `#` do Titan sobre
/// strings.
pub fn string_len(s: &str) -> i64 {
    s.len() as i64
}

/// Lê `m[chave]`, devolvendo o erro em português em vez de abortar. Base para
/// [`map_get`].
pub fn map_get_checked<K, V>(m: &std::collections::HashMap<K, V>, chave: &K) -> Result<V, String>
where
    K: std::hash::Hash + Eq,
    V: Clone,
{
    m.get(chave)
        .cloned()
        .ok_or_else(|| "chave não encontrada no map".to_string())
}

/// Lê `m[chave]`. Aborta com mensagem em português se a chave não existir.
pub fn map_get<K, V>(m: &std::collections::HashMap<K, V>, chave: &K) -> V
where
    K: std::hash::Hash + Eq,
    V: Clone,
{
    match map_get_checked(m, chave) {
        Ok(val) => val,
        Err(msg) => abortar(&msg),
    }
}

/// Escreve `m[chave] = valor`. Maps em Titan não têm noção de faixa — sempre
/// insere ou substitui, nunca falha.
pub fn map_set<K, V>(m: &mut std::collections::HashMap<K, V>, chave: K, valor: V)
where
    K: std::hash::Hash + Eq,
{
    m.insert(chave, valor);
}

/// Referência mutável a `m[chave]`, devolvendo o erro em português em vez de
/// abortar. Base para [`map_get_mut`] — mesmo papel de
/// [`array_get_mut_checked`], necessário para escrever através de um `v` que
/// é ele mesmo o valor de outro composto (`m[chave][i] = x`,
/// `f(m[chave])` quando o parâmetro é composto): sem uma referência real ao
/// lugar dentro do `HashMap`, a escrita só alcançaria uma cópia.
pub fn map_get_mut_checked<'a, K, V>(
    m: &'a mut std::collections::HashMap<K, V>,
    chave: &K,
) -> Result<&'a mut V, String>
where
    K: std::hash::Hash + Eq,
{
    m.get_mut(chave)
        .ok_or_else(|| "chave não encontrada no map".to_string())
}

/// Referência mutável a `m[chave]`. Aborta com mensagem em português se a
/// chave não existir.
pub fn map_get_mut<'a, K, V>(m: &'a mut std::collections::HashMap<K, V>, chave: &K) -> &'a mut V
where
    K: std::hash::Hash + Eq,
{
    match map_get_mut_checked(m, chave) {
        Ok(val) => val,
        Err(msg) => abortar(&msg),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn concat_junta_as_duas_partes() {
        assert_eq!(concat("Olá, ", "mundo!"), "Olá, mundo!");
    }

    #[test]
    fn concat_preserva_utf8_multibyte() {
        // `len()` é em bytes: "ção" ocupa 5 bytes, não 3.
        let s = concat("informa", "ção");
        assert_eq!(s, "informação");
        assert_eq!(s.chars().count(), 10);
    }

    #[test]
    fn concat_com_string_vazia_e_identidade() {
        assert_eq!(concat("", "titan"), "titan");
        assert_eq!(concat("titan", ""), "titan");
    }

    #[test]
    fn print_aceita_str_e_nao_entra_em_panico() {
        print("linha de teste do runtime");
    }

    // --- array_get_checked ---------------------------------------------

    #[test]
    fn array_get_checked_faixa_valida() {
        let v = vec![10, 20, 30];
        assert_eq!(array_get_checked(&v, 1), Ok(10));
        assert_eq!(array_get_checked(&v, 2), Ok(20));
        assert_eq!(array_get_checked(&v, 3), Ok(30));
    }

    #[test]
    fn array_get_checked_indice_zero() {
        let v = vec![10, 20, 30];
        assert_eq!(
            array_get_checked(&v, 0),
            Err("índice 0 inválido: arrays em Titan começam em 1".to_string())
        );
    }

    #[test]
    fn array_get_checked_indice_negativo() {
        let v = vec![10, 20, 30];
        assert_eq!(
            array_get_checked(&v, -1),
            Err("índice -1 fora da faixa (array tem 3 elementos)".to_string())
        );
    }

    #[test]
    fn array_get_checked_alem_do_fim() {
        let v = vec![10, 20, 30];
        assert_eq!(
            array_get_checked(&v, 99),
            Err("índice 99 fora da faixa (array tem 3 elementos)".to_string())
        );
    }

    #[test]
    fn array_get_checked_no_limite_do_fim_falha() {
        // `#v` é válido para leitura, `#v + 1` não é (isso é só para `array_set`).
        let v = vec![10, 20, 30];
        assert_eq!(
            array_get_checked(&v, 4),
            Err("índice 4 fora da faixa (array tem 3 elementos)".to_string())
        );
    }

    // --- array_get_mut_checked ------------------------------------------

    #[test]
    fn array_get_mut_checked_permite_escrever() {
        let mut v = vec![10, 20, 30];
        *array_get_mut_checked(&mut v, 2).unwrap() = 99;
        assert_eq!(v, vec![10, 99, 30]);
    }

    #[test]
    fn array_get_mut_checked_indice_zero() {
        let mut v = vec![10, 20, 30];
        assert_eq!(
            array_get_mut_checked(&mut v, 0),
            Err("índice 0 inválido: arrays em Titan começam em 1".to_string())
        );
    }

    #[test]
    fn array_get_mut_checked_alem_do_fim() {
        let mut v = vec![10, 20, 30];
        assert_eq!(
            array_get_mut_checked(&mut v, 4),
            Err("índice 4 fora da faixa (array tem 3 elementos)".to_string())
        );
    }

    // --- array_set_checked ------------------------------------------------

    #[test]
    fn array_set_checked_escreve_em_faixa_valida() {
        let mut v = vec![10, 20, 30];
        assert_eq!(array_set_checked(&mut v, 1, 100), Ok(()));
        assert_eq!(v, vec![100, 20, 30]);
    }

    #[test]
    fn array_set_checked_escreve_no_limite_do_fim() {
        let mut v = vec![10, 20, 30];
        assert_eq!(array_set_checked(&mut v, 3, 300), Ok(()));
        assert_eq!(v, vec![10, 20, 300]);
    }

    #[test]
    fn array_set_checked_append_em_len_mais_um() {
        let mut v = vec![10, 20, 30];
        assert_eq!(array_set_checked(&mut v, 4, 40), Ok(()));
        assert_eq!(v, vec![10, 20, 30, 40]);
    }

    #[test]
    fn array_set_checked_indice_zero() {
        let mut v = vec![10, 20, 30];
        assert_eq!(
            array_set_checked(&mut v, 0, 1),
            Err("índice 0 inválido: arrays em Titan começam em 1".to_string())
        );
    }

    #[test]
    fn array_set_checked_indice_negativo() {
        let mut v = vec![10, 20, 30];
        assert_eq!(
            array_set_checked(&mut v, -1, 1),
            Err(
                "índice -1 fora da faixa (array tem 3 elementos; só é possível escrever em \
                 1..3 ou fazer append em 4)"
                    .to_string()
            )
        );
    }

    #[test]
    fn array_set_checked_alem_do_fim() {
        let mut v = vec![10, 20, 30];
        assert_eq!(
            array_set_checked(&mut v, 5, 1),
            Err(
                "índice 5 fora da faixa (array tem 3 elementos; só é possível escrever em \
                 1..3 ou fazer append em 4)"
                    .to_string()
            )
        );
    }

    #[test]
    fn array_set_checked_em_array_vazio_faz_append_no_indice_1() {
        let mut v: Vec<i64> = vec![];
        assert_eq!(array_set_checked(&mut v, 1, 42), Ok(()));
        assert_eq!(v, vec![42]);
    }

    // --- array_len / string_len -------------------------------------------

    #[test]
    fn array_len_conta_elementos() {
        assert_eq!(array_len(&[1, 2, 3]), 3);
        assert_eq!(array_len::<i64>(&[]), 0);
    }

    #[test]
    fn string_len_conta_bytes_nao_caracteres() {
        assert_eq!(string_len("abc"), 3);
        assert_eq!(string_len("ção"), 5); // 5 bytes, 3 caracteres.
    }

    // --- map_get_checked / map_set -----------------------------------------

    #[test]
    fn map_set_e_get_com_chave_string() {
        let mut m: HashMap<String, i64> = HashMap::new();
        map_set(&mut m, "a".to_string(), 1);
        assert_eq!(map_get_checked(&m, &"a".to_string()), Ok(1));
    }

    #[test]
    fn map_get_checked_chave_ausente() {
        let m: HashMap<String, i64> = HashMap::new();
        assert_eq!(
            map_get_checked(&m, &"faltando".to_string()),
            Err("chave não encontrada no map".to_string())
        );
    }

    #[test]
    fn map_set_sobrescreve_valor_existente() {
        let mut m: HashMap<String, i64> = HashMap::new();
        map_set(&mut m, "a".to_string(), 1);
        map_set(&mut m, "a".to_string(), 2);
        assert_eq!(map_get_checked(&m, &"a".to_string()), Ok(2));
    }

    #[test]
    fn map_com_chave_integer() {
        let mut m: HashMap<i64, String> = HashMap::new();
        map_set(&mut m, 1, "um".to_string());
        assert_eq!(map_get_checked(&m, &1), Ok("um".to_string()));
        assert_eq!(
            map_get_checked(&m, &2),
            Err("chave não encontrada no map".to_string())
        );
    }

    // --- map_get_mut_checked ------------------------------------------------

    #[test]
    fn map_get_mut_checked_permite_escrever() {
        let mut m: HashMap<String, i64> = HashMap::new();
        map_set(&mut m, "a".to_string(), 1);
        *map_get_mut_checked(&mut m, &"a".to_string()).unwrap() = 99;
        assert_eq!(map_get_checked(&m, &"a".to_string()), Ok(99));
    }

    #[test]
    fn map_get_mut_checked_chave_ausente() {
        let mut m: HashMap<String, i64> = HashMap::new();
        assert_eq!(
            map_get_mut_checked(&mut m, &"faltando".to_string()),
            Err("chave não encontrada no map".to_string())
        );
    }
}
