//! Texto Runtime da linguagem Titan (PRD.md, T53).
//!
//! Implementa a capability `texto` (`import texto`): o mínimo que um lexer
//! precisa para andar sobre o fonte — indexação por byte, fatiamento,
//! conversão para/de inteiro e tamanho. Mesmo molde de `titan-data`
//! (`crates/titan-data/src/lib.rs`): nenhum tipo opaco, só funções de
//! módulo, cada uma com o par `*_checked -> Result<_, String>` (mensagem em
//! português) mais o wrapper que aborta o processo — nunca `panic!` cru.
//!
//! Todas as operações trabalham sobre **bytes**, coerente com `#s`
//! (`titan-runtime/src/lib.rs:134-136`, que já conta bytes) e com a
//! indexação de arrays (1-based). Isso é uma limitação deliberada: o fonte é
//! assumido ASCII; uma string com acento (UTF-8 multi-byte) tem
//! comportamento definido — os bytes são indexados individualmente — mas não
//! corresponde a "caracteres".

/// Aborta a execução com uma mensagem em português, sem `panic!` cru — a
/// única forma de erro fatal em tempo de execução do Titan.
fn abortar(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(1);
}

/// Lê o byte de `s` no índice 1-based `i`, devolvendo o erro em português em
/// vez de abortar. Base para [`byte`].
pub fn byte_checked(s: &str, i: i64) -> Result<i64, String> {
    if i == 0 {
        return Err("índice 0 inválido: strings em Titan começam em 1".to_string());
    }
    let bytes = s.as_bytes();
    if i < 0 || (i as usize) > bytes.len() {
        return Err(format!(
            "índice {i} fora da faixa (string tem {} bytes)",
            bytes.len()
        ));
    }
    Ok(bytes[(i - 1) as usize] as i64)
}

/// Byte de `s` no índice 1-based `i`. Aborta com mensagem em português se
/// `i` for 0, negativo ou além do fim da string.
pub fn byte(s: &str, i: i64) -> i64 {
    match byte_checked(s, i) {
        Ok(b) => b,
        Err(msg) => abortar(&msg),
    }
}

/// Fatia `s` do byte 1-based `i` ao byte `j`, ambos inclusivos (como o
/// `string.sub` do Lua), devolvendo o erro em português em vez de abortar.
/// Base para [`sub`].
pub fn sub_checked(s: &str, i: i64, j: i64) -> Result<String, String> {
    if i == 0 || j == 0 {
        return Err("índice 0 inválido: strings em Titan começam em 1".to_string());
    }
    let bytes = s.as_bytes();
    if i < 0 || j < 0 {
        return Err(format!("índices negativos não são aceitos ({i}, {j})"));
    }
    if (i as usize) > bytes.len() {
        return Err(format!(
            "índice inicial {i} fora da faixa (string tem {} bytes)",
            bytes.len()
        ));
    }
    if j < i {
        return Ok(String::new());
    }
    let fim = (j as usize).min(bytes.len());
    let fatia = &bytes[(i - 1) as usize..fim];
    String::from_utf8(fatia.to_vec())
        .map_err(|_| format!("texto.sub({i}, {j}) corta um caractere UTF-8 multi-byte ao meio"))
}

/// Fatia `s` do byte 1-based `i` ao byte `j`, ambos inclusivos. Aborta com
/// mensagem em português se os índices forem inválidos ou a fatia cortar um
/// caractere UTF-8 ao meio.
pub fn sub(s: &str, i: i64, j: i64) -> String {
    match sub_checked(s, i, j) {
        Ok(v) => v,
        Err(msg) => abortar(&msg),
    }
}

/// Converte `s` para `integer`, devolvendo o erro em português em vez de
/// abortar. Base para [`para_inteiro`].
pub fn para_inteiro_checked(s: &str) -> Result<i64, String> {
    s.trim()
        .parse::<i64>()
        .map_err(|_| format!("'{s}' não é um inteiro válido"))
}

/// Converte `s` para `integer`. Aborta com mensagem em português se `s` não
/// for um inteiro válido — sem `Option`: a conversão sempre produz um
/// `integer` ou encerra o processo.
pub fn para_inteiro(s: &str) -> i64 {
    match para_inteiro_checked(s) {
        Ok(v) => v,
        Err(msg) => abortar(&msg),
    }
}

/// Converte `n` para `string`. Nunca falha — o `tostring` que falta na
/// linguagem.
pub fn de_inteiro(n: i64) -> String {
    n.to_string()
}

/// Tamanho de `s` em bytes. Espelha `#s`, para uso explícito quando o
/// programa Titan já tem um valor de outro tipo em mãos. Nunca falha.
pub fn tamanho(s: &str) -> i64 {
    s.len() as i64
}
