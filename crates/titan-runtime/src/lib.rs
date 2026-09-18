//! Runtime da linguagem Titan.
//!
//! Este crate reúne as funções que o código Rust gerado pelo `titanc` chama em
//! tempo de execução. Na Fase 0 eram apenas duas: `print` (que o Titan original
//! não possui — aqui ela vem da stdlib, não é palavra-chave) e `concat`, que dá
//! suporte ao operador `..`. A Fase 2 acrescenta a superfície de arrays,
//! records e maps: indexação checada, sem nunca expor o `panic!` cru do Rust.
//! A Fase 5 acrescenta [`idiv`], a divisão com piso do operador `//`, e
//! [`shl`]/[`shr`], os deslocamentos com a semântica do Lua (quantidade de
//! deslocamento arbitrária, inclusive negativa).
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

/// Divisão inteira com piso — o `//` do Titan (PRD.md, T61).
///
/// **Não** é o `/` do Rust: o Rust trunca em direção a zero e o Titan/Lua
/// arredonda para menos infinito, então `-7 // 2` é `-4`, não `-3`
/// (`coder.lua:1716-1746`, que inlineia o `luaV_div` do Lua).
///
/// Três detalhes, todos vindos do original:
///
/// - **Divisão por zero aborta** com mensagem em português (o original lança
///   "divide by zero"), em vez de deixar o `panic!` do Rust escapar.
/// - **`b == -1` sai por negação com wrap**, porque `i64::MIN / -1`
///   transborda.
/// - **`div_euclid` não serve**: para divisor negativo ele mantém o resto
///   não-negativo em vez de arredondar para baixo — `(-7).div_euclid(-2)` é
///   `4`, e `-7 // -2` no Titan é `3`. Daí a correção explícita abaixo.
///
/// ```
/// assert_eq!(titan_runtime::idiv(7, 2), 3);
/// assert_eq!(titan_runtime::idiv(-7, 2), -4);
/// ```
pub fn idiv(a: i64, b: i64) -> i64 {
    match idiv_checked(a, b) {
        Ok(q) => q,
        Err(msg) => abortar(&msg),
    }
}

/// `a // b` devolvendo o erro em português em vez de abortar — base
/// testável de [`idiv`], mesmo par `*_checked`/aborta de
/// [`array_get_checked`]/[`array_get`].
pub fn idiv_checked(a: i64, b: i64) -> Result<i64, String> {
    if b == 0 {
        return Err("divisão inteira por zero".to_string());
    }
    if b == -1 {
        // `i64::MIN / -1` transborda; `wrapping_neg` reproduz o
        // `intop(-, 0, m)` do original, que é aritmética com wrap-around.
        return Ok(a.wrapping_neg());
    }
    let q = a / b;
    // Sinais opostos e divisão não exata: o truncamento do Rust ficou uma
    // unidade acima do piso.
    if (a % b != 0) && ((a < 0) != (b < 0)) {
        Ok(q - 1)
    } else {
        Ok(q)
    }
}

/// `a << b` com a semântica do Titan (PRD.md, T61).
///
/// **Não** é o `<<` do Rust, que exige `0 <= b < 64` e transborda fora
/// disso — `1 << 64` sequer compila em Rust, e o erro chegaria em inglês
/// sobre código que o usuário não escreveu. No Titan/Lua a quantidade de
/// deslocamento é um inteiro qualquer (`coder.lua:1670-1710`, que reordena o
/// `luaV_shiftl`): deslocamento **negativo inverte a direção** e
/// deslocamento de 64 ou mais zera o resultado.
///
/// O deslocamento é sempre **lógico**, não aritmético: o bit de sinal não se
/// propaga, daí a conta passar por `u64` (`>>` sobre `i64` no Rust
/// propagaria o sinal, divergindo do Lua).
///
/// ```
/// assert_eq!(titan_runtime::shl(1, 10), 1024);
/// assert_eq!(titan_runtime::shl(1, 64), 0);
/// assert_eq!(titan_runtime::shl(1024, -10), 1);
/// ```
pub fn shl(a: i64, b: i64) -> i64 {
    if b <= -64 || b >= 64 {
        return 0;
    }
    if b >= 0 {
        ((a as u64) << b) as i64
    } else {
        ((a as u64) >> -b) as i64
    }
}

/// `a >> b` com a semântica do Titan — [`shl`] com a direção invertida
/// (`coder.lua:1916`), incluindo o deslocamento lógico e o zero fora da
/// faixa.
///
/// ```
/// assert_eq!(titan_runtime::shr(1024, 10), 1);
/// assert_eq!(titan_runtime::shr(1, 64), 0);
/// assert_eq!(titan_runtime::shr(-1, 63), 1);
/// ```
pub fn shr(a: i64, b: i64) -> i64 {
    // `-b` transbordaria só para `i64::MIN`, que já saiu como 0 acima por
    // estar fora da faixa — mas o `wrapping_neg` deixa isso explícito.
    if b <= -64 || b >= 64 {
        return 0;
    }
    shl(a, b.wrapping_neg())
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

// ---- `value`: o tipo dinâmico do gradual typing (T70) -------------------

/// O tipo `value` do Titan — o topo do gradual typing, para onde **qualquer**
/// tipo pode ser convertido com `as` (PRD.md, T70).
///
/// É um `enum` boxado, e não um ponteiro cru ou um `Box<dyn Any>`, por três
/// razões que se sustentam juntas:
///
/// - **`PartialEq` estrutural sai de graça** — `value` precisa comparar
///   valores, não endereços, e `dyn Any` não dá isso sem downcast manual
///   variante a variante.
/// - **A conversão de volta erra com mensagem em português.** Um downcast de
///   `Any` falha com `None` e sem contexto; aqui a variante errada vira texto
///   que diz qual tipo estava guardado.
/// - **O conjunto de tipos é fechado.** Titan não tem tipos abertos em tempo
///   de execução, então enumerar as variantes é fiel à linguagem e ainda
///   deixa o `match` do runtime exaustivo.
///
/// Os compostos entram **por valor** (`Vec<Value>`, `HashMap<..>`,
/// `Box<Value>`), homogeneizados para `Value` em vez de genéricos sobre `T`:
/// um `{integer}` e um `{string}` precisam caber no mesmo `value`, e um
/// `Value` genérico não seria um tipo só. Isso é coerente com o ADR 0006 —
/// converter para `value` **copia** o composto, não o aliasa.
///
/// `Record` guarda o nome do tipo ao lado dos campos porque o `equals` de
/// records é nominal (`types.rs`), então dois records de campos iguais e
/// nomes diferentes não podem sair iguais aqui.
#[derive(Clone, Debug)]
pub enum Value {
    Nil,
    Boolean(bool),
    Integer(i64),
    Float(f64),
    String(String),
    Array(Vec<Value>),
    Map(Vec<(Value, Value)>),
    Record {
        nome: String,
        campos: Vec<(String, Value)>,
    },
    /// `T?` já preenchido. O `nil` de um opcional vazio é [`Value::Nil`], e
    /// não `Option(None)`: `value` já tem um "ausente" só, e ter dois
    /// faria `nil as value` diferir de `(nil as integer?) as value`.
    Option(Box<Value>),
}

/// Igualdade **estrutural**, com um cuidado que o `derive` não teria:
/// `Value::Map` guarda os pares num `Vec` e a ordem em que eles saíram do
/// `HashMap` de origem é não especificada, então dois maps iguais podem ter
/// vetores em ordens diferentes. A comparação é feita como **conjunto de
/// pares** — mesmo tamanho e cada par do primeiro presente no segundo.
///
/// É quadrática, e de propósito: a chave é um `Value`, que não é `Hash` (um
/// `f64` dentro impediria), e maps convertidos para `value` são pequenos por
/// natureza — comparar dois deles não é caminho quente.
///
/// `Record` compara **nominalmente** primeiro (o `nome`), fiel ao `equals` de
/// records do checker: dois records de campos idênticos e nomes diferentes
/// não são iguais.
impl PartialEq for Value {
    fn eq(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Nil, Value::Nil) => true,
            (Value::Boolean(a), Value::Boolean(b)) => a == b,
            (Value::Integer(a), Value::Integer(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::String(a), Value::String(b)) => a == b,
            (Value::Array(a), Value::Array(b)) => a == b,
            (Value::Map(a), Value::Map(b)) => {
                a.len() == b.len() && a.iter().all(|par| b.contains(par))
            }
            (
                Value::Record {
                    nome: n1,
                    campos: c1,
                },
                Value::Record {
                    nome: n2,
                    campos: c2,
                },
            ) => n1 == n2 && c1 == c2,
            (Value::Option(a), Value::Option(b)) => a == b,
            _ => false,
        }
    }
}

/// Nome Titan do tipo guardado — o mesmo texto que `checker::type_name`
/// usa, para a mensagem de erro falar a língua do programa e não a do Rust.
pub fn value_type_name(v: &Value) -> &'static str {
    match v {
        Value::Nil => "nil",
        Value::Boolean(_) => "boolean",
        Value::Integer(_) => "integer",
        Value::Float(_) => "float",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Map(_) => "map",
        Value::Record { .. } => "record",
        Value::Option(_) => "opcional",
    }
}

/// Erro padrão de descida de `value`: diz o tipo pedido e o guardado, em
/// português, como toda falha de execução do Titan.
fn erro_de_value(esperado: &str, v: &Value) -> String {
    format!(
        "`value` não guarda um {esperado}: guarda um {}",
        value_type_name(v)
    )
}

/// `v as boolean` sobre um `value`. Base checada de [`value_to_boolean`].
pub fn value_to_boolean_checked(v: &Value) -> Result<bool, String> {
    match v {
        Value::Boolean(b) => Ok(*b),
        outro => Err(erro_de_value("boolean", outro)),
    }
}

/// `v as boolean`. Aborta com mensagem em português se o `value` guardar
/// outro tipo.
pub fn value_to_boolean(v: &Value) -> bool {
    match value_to_boolean_checked(v) {
        Ok(b) => b,
        Err(msg) => abortar(&msg),
    }
}

/// `v as integer` sobre um `value`. Base checada de [`value_to_integer`].
///
/// **Não** aceita um `Value::Float` guardado: `value` preserva o tipo que
/// entrou, e converter float→integer aqui em silêncio esconderia a truncagem
/// atrás de um cast que o programador escreveu como se fosse seguro. Quem
/// quer os dois passos escreve os dois: `v as float as integer`.
pub fn value_to_integer_checked(v: &Value) -> Result<i64, String> {
    match v {
        Value::Integer(i) => Ok(*i),
        outro => Err(erro_de_value("integer", outro)),
    }
}

/// `v as integer`. Aborta com mensagem em português se o `value` guardar
/// outro tipo.
pub fn value_to_integer(v: &Value) -> i64 {
    match value_to_integer_checked(v) {
        Ok(i) => i,
        Err(msg) => abortar(&msg),
    }
}

/// `v as float` sobre um `value`. Base checada de [`value_to_float`].
///
/// Simétrico a [`value_to_integer_checked`]: um `Value::Integer` guardado
/// **não** vira float sozinho.
pub fn value_to_float_checked(v: &Value) -> Result<f64, String> {
    match v {
        Value::Float(f) => Ok(*f),
        outro => Err(erro_de_value("float", outro)),
    }
}

/// `v as float`. Aborta com mensagem em português se o `value` guardar outro
/// tipo.
pub fn value_to_float(v: &Value) -> f64 {
    match value_to_float_checked(v) {
        Ok(f) => f,
        Err(msg) => abortar(&msg),
    }
}

/// `v as string` sobre um `value`. Base checada de [`value_to_string`].
///
/// **Não** formata o valor guardado: `value` não é `tostring`. Um
/// `Value::Integer` aqui é erro, não `"42"` — converter em silêncio faria o
/// cast mentir sobre o que aconteceu (ADR 0010: `string` é sempre `string`).
pub fn value_to_string_checked(v: &Value) -> Result<String, String> {
    match v {
        Value::String(s) => Ok(s.clone()),
        outro => Err(erro_de_value("string", outro)),
    }
}

/// `v as string`. Aborta com mensagem em português se o `value` guardar outro
/// tipo.
pub fn value_to_string(v: &Value) -> String {
    match value_to_string_checked(v) {
        Ok(s) => s,
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

    // --- idiv ---------------------------------------------------------------

    #[test]
    fn idiv_com_operandos_positivos_e_a_divisao_usual() {
        assert_eq!(idiv(7, 2), 3);
        assert_eq!(idiv(6, 3), 2);
        assert_eq!(idiv(0, 5), 0);
    }

    /// A propriedade que separa `//` do `/` do Rust: sinais opostos
    /// arredondam **para baixo**, não em direção a zero.
    #[test]
    fn idiv_com_sinais_opostos_arredonda_para_baixo() {
        assert_eq!(idiv(-7, 2), -4);
        assert_eq!(idiv(7, -2), -4);
        // Divisão exata não tem o que arredondar, mesmo com sinais opostos.
        assert_eq!(idiv(-6, 2), -3);
        assert_eq!(idiv(6, -2), -3);
    }

    #[test]
    fn idiv_com_dois_negativos_da_quociente_positivo() {
        assert_eq!(idiv(-7, -2), 3);
        assert_eq!(idiv(-6, -3), 2);
    }

    /// `div_euclid` seria a escolha óbvia e está errada para divisor
    /// negativo — este teste fixa a divergência.
    #[test]
    fn idiv_diverge_de_div_euclid_quando_o_divisor_e_negativo() {
        assert_eq!(idiv(-7, -2), 3);
        assert_eq!((-7i64).div_euclid(-2), 4);
    }

    #[test]
    fn idiv_por_menos_um_nao_transborda() {
        assert_eq!(idiv(10, -1), -10);
        assert_eq!(idiv(i64::MIN, -1), i64::MIN);
    }

    // --- shl / shr ---------------------------------------------------------

    #[test]
    fn shift_com_deslocamento_usual() {
        assert_eq!(shl(1, 10), 1024);
        assert_eq!(shr(1024, 10), 1);
        assert_eq!(shl(5, 0), 5);
        assert_eq!(shr(5, 0), 5);
    }

    /// No Titan/Lua um deslocamento negativo inverte a direção — no Rust
    /// nem sequer é representável.
    #[test]
    fn shift_com_deslocamento_negativo_inverte_a_direcao() {
        assert_eq!(shl(1024, -10), 1);
        assert_eq!(shr(1, -10), 1024);
    }

    /// Deslocar 64 ou mais zera; no Rust isso seria overflow — e o rustc
    /// recusa a compilação quando consegue provar, com mensagem em inglês.
    #[test]
    fn shift_fora_da_faixa_zera_em_vez_de_transbordar() {
        assert_eq!(shl(1, 64), 0);
        assert_eq!(shl(1, 1000), 0);
        assert_eq!(shr(1, 64), 0);
        assert_eq!(shl(1, -64), 0);
        assert_eq!(shr(-1, -1000), 0);
        assert_eq!(shl(1, i64::MIN), 0);
        assert_eq!(shr(1, i64::MIN), 0);
    }

    /// O deslocamento do Lua é lógico: o bit de sinal não se propaga, ao
    /// contrário do `>>` do Rust sobre `i64`.
    #[test]
    fn shift_a_direita_e_logico_nao_aritmetico() {
        assert_eq!(shr(-1, 63), 1);
        // O `>>` do Rust sobre i64 propagaria o sinal e daria -1.
        assert_eq!(-1i64 >> 63, -1);
    }

    /// O original lança "divide by zero" em vez de deixar a UB acontecer;
    /// aqui a mensagem chega em português, e [`idiv`] a transforma em
    /// aborto sem `panic!` cru.
    #[test]
    fn idiv_por_zero_devolve_erro_em_portugues() {
        assert_eq!(
            idiv_checked(1, 0),
            Err("divisão inteira por zero".to_string())
        );
    }

    // --- `value` (T70) ----------------------------------------------------

    #[test]
    fn value_desce_para_a_variante_guardada() {
        assert_eq!(value_to_integer_checked(&Value::Integer(42)), Ok(42));
        assert_eq!(value_to_float_checked(&Value::Float(2.5)), Ok(2.5));
        assert_eq!(value_to_boolean_checked(&Value::Boolean(true)), Ok(true));
        assert_eq!(
            value_to_string_checked(&Value::String("oi".to_string())),
            Ok("oi".to_string())
        );
    }

    /// A mensagem nomeia os dois tipos em português — é o que o programa
    /// imprime antes de abortar, e sem ela o usuário só saberia que "deu
    /// errado".
    #[test]
    fn value_de_variante_errada_erra_em_portugues() {
        assert_eq!(
            value_to_integer_checked(&Value::String("oi".to_string())),
            Err("`value` não guarda um integer: guarda um string".to_string())
        );
        assert_eq!(
            value_to_string_checked(&Value::Nil),
            Err("`value` não guarda um string: guarda um nil".to_string())
        );
    }

    /// Descer não converte: um `integer` guardado **não** sai como float, nem
    /// vice-versa. Quem quer a conversão escreve os dois passos, e aí a
    /// truncagem fica visível no fonte.
    #[test]
    fn value_nao_converte_numero_na_descida() {
        assert!(value_to_float_checked(&Value::Integer(3)).is_err());
        assert!(value_to_integer_checked(&Value::Float(3.0)).is_err());
    }

    /// `value` não é `tostring`: um número guardado não vira texto sozinho
    /// (ADR 0010).
    #[test]
    fn value_nao_formata_numero_como_string() {
        assert_eq!(
            value_to_string_checked(&Value::Integer(42)),
            Err("`value` não guarda um string: guarda um integer".to_string())
        );
    }

    /// A ordem do `Vec` de pares de um `Value::Map` vem da iteração de um
    /// `HashMap`, que é não especificada — comparar como lista daria falsos
    /// negativos dependentes do acaso.
    #[test]
    fn value_map_compara_como_conjunto_ignorando_a_ordem() {
        let a = Value::Map(vec![
            (Value::String("a".to_string()), Value::Integer(1)),
            (Value::String("b".to_string()), Value::Integer(2)),
        ]);
        let b = Value::Map(vec![
            (Value::String("b".to_string()), Value::Integer(2)),
            (Value::String("a".to_string()), Value::Integer(1)),
        ]);
        assert_eq!(a, b);
    }

    #[test]
    fn value_map_de_tamanhos_diferentes_nao_e_igual() {
        let a = Value::Map(vec![(Value::String("a".to_string()), Value::Integer(1))]);
        let b = Value::Map(vec![
            (Value::String("a".to_string()), Value::Integer(1)),
            (Value::String("b".to_string()), Value::Integer(2)),
        ]);
        assert_ne!(a, b);
    }

    /// Record é **nominal**, como o `equals` do checker: mesmos campos com
    /// nome de tipo diferente não são o mesmo valor.
    #[test]
    fn value_record_compara_pelo_nome_do_tipo() {
        let campos = vec![("x".to_string(), Value::Integer(1))];
        let p = Value::Record {
            nome: "Ponto".to_string(),
            campos: campos.clone(),
        };
        let q = Value::Record {
            nome: "Outro".to_string(),
            campos,
        };
        assert_ne!(p, q);
    }

    /// Array continua **ordenado** — diferente do map, a ordem é parte do
    /// valor.
    #[test]
    fn value_array_respeita_a_ordem() {
        assert_ne!(
            Value::Array(vec![Value::Integer(1), Value::Integer(2)]),
            Value::Array(vec![Value::Integer(2), Value::Integer(1)])
        );
    }

    #[test]
    fn value_type_name_cobre_as_variantes() {
        assert_eq!(value_type_name(&Value::Nil), "nil");
        assert_eq!(value_type_name(&Value::Integer(1)), "integer");
        assert_eq!(
            value_type_name(&Value::Option(Box::new(Value::Integer(1)))),
            "opcional"
        );
    }
}
