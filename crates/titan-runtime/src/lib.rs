//! Runtime da linguagem Titan.
//!
//! Este crate reúne as funções que o código Rust gerado pelo `titanc` chama em
//! tempo de execução. Na Fase 0 são apenas duas: `print` (que o Titan original
//! não possui — aqui ela vem da stdlib, não é palavra-chave) e `concat`, que dá
//! suporte ao operador `..`.

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
