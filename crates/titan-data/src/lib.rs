//! Data Runtime da linguagem Titan (PRD.md, T41).
//!
//! Implementa a capability `data` (`import data`): leitura de CSV, inspeção
//! (linhas, colunas, coluna→array Titan) e agregação (soma, média, mín,
//! máx), sobre o Polars. A API `data.*` é o contrato; o Polars é detalhe
//! interno, trocável sem mudar o programa Titan (decisão 5 do PRD.md).
//!
//! Segue o mesmo padrão de erro do `titan-runtime` (`lib.rs:35-43`): cada
//! operação que pode falhar tem o par `*_checked -> Result<_, String>`
//! (mensagem em português) mais o wrapper que aborta o processo — nunca
//! `panic!` cru, nunca erro do Polars em inglês vazando para o usuário.

use polars::prelude::*;

/// Tipo opaco `data.DataFrame`: o programa Titan carrega e passa adiante,
/// mas não inspeciona os campos. Precisa de `Clone` porque `Type::Opaque`
/// entra em `is_composite` (decisão 8 do PRD.md) e herda `clone()` na
/// atribuição.
#[derive(Clone, Debug)]
pub struct DataFrame(polars::frame::DataFrame);

/// Aborta a execução com uma mensagem em português, sem `panic!` cru — a
/// única forma de erro fatal em tempo de execução do Titan.
fn abortar(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(1);
}

/// Lê um CSV em `caminho`, com cabeçalho, devolvendo o erro em português em
/// vez de abortar. Base para [`read_csv`].
pub fn read_csv_checked(caminho: &str) -> Result<DataFrame, String> {
    let df = CsvReadOptions::default()
        .with_has_header(true)
        .try_into_reader_with_file_path(Some(caminho.into()))
        .map_err(|_| format!("não foi possível abrir o arquivo CSV '{caminho}'"))?
        .finish()
        .map_err(|e| format!("falha ao ler o CSV '{caminho}': {e}"))?;
    Ok(DataFrame(df))
}

/// Lê um CSV em `caminho`, com cabeçalho. Aborta com mensagem em português se
/// o arquivo não existir ou não puder ser lido como CSV.
pub fn read_csv(caminho: &str) -> DataFrame {
    match read_csv_checked(caminho) {
        Ok(df) => df,
        Err(msg) => abortar(&msg),
    }
}

/// Número de linhas do DataFrame. Nunca falha.
pub fn linhas(df: &mut DataFrame) -> i64 {
    df.0.height() as i64
}

/// Nomes das colunas do DataFrame, na ordem em que aparecem no CSV. Nunca
/// falha.
pub fn colunas(df: &mut DataFrame) -> Vec<String> {
    df.0.get_column_names()
        .into_iter()
        .map(|s| s.to_string())
        .collect()
}

/// Busca a coluna `nome`, devolvendo o erro em português em vez de abortar
/// se ela não existir.
fn buscar_coluna<'a>(df: &'a DataFrame, nome: &str) -> Result<&'a Column, String> {
    df.0.column(nome)
        .map_err(|_| format!("coluna '{nome}' não existe no DataFrame"))
}

/// Extrai `nome` como array de inteiros, devolvendo o erro em português em
/// vez de abortar. Base para [`coluna_integer`].
pub fn coluna_integer_checked(df: &mut DataFrame, nome: &str) -> Result<Vec<i64>, String> {
    let coluna = buscar_coluna(df, nome)?;
    let serie = coluna
        .i64()
        .map_err(|_| format!("coluna '{nome}' não é do tipo integer"))?;
    Ok(serie.into_no_null_iter().collect())
}

/// Extrai `nome` como array de inteiros. Aborta com mensagem em português se
/// a coluna não existir ou não for do tipo integer.
pub fn coluna_integer(df: &mut DataFrame, nome: &str) -> Vec<i64> {
    match coluna_integer_checked(df, nome) {
        Ok(vs) => vs,
        Err(msg) => abortar(&msg),
    }
}

/// Extrai `nome` como array de ponto flutuante, devolvendo o erro em
/// português em vez de abortar. Base para [`coluna_float`].
pub fn coluna_float_checked(df: &mut DataFrame, nome: &str) -> Result<Vec<f64>, String> {
    let coluna = buscar_coluna(df, nome)?;
    let serie = coluna
        .f64()
        .map_err(|_| format!("coluna '{nome}' não é do tipo float"))?;
    Ok(serie.into_no_null_iter().collect())
}

/// Extrai `nome` como array de ponto flutuante. Aborta com mensagem em
/// português se a coluna não existir ou não for do tipo float.
pub fn coluna_float(df: &mut DataFrame, nome: &str) -> Vec<f64> {
    match coluna_float_checked(df, nome) {
        Ok(vs) => vs,
        Err(msg) => abortar(&msg),
    }
}

/// Soma `nome`, devolvendo o erro em português em vez de abortar. Base para
/// [`soma`]. Agregações devolvem sempre `f64` (decisão 9 do PRD.md).
pub fn soma_checked(df: &mut DataFrame, nome: &str) -> Result<f64, String> {
    let coluna = buscar_coluna(df, nome)?;
    coluna
        .sum_reduce()
        .map_err(|_| format!("não foi possível somar a coluna '{nome}'"))?
        .value()
        .try_extract::<f64>()
        .map_err(|_| format!("coluna '{nome}' não é numérica"))
}

/// Soma `nome`. Aborta com mensagem em português se a coluna não existir ou
/// não for numérica.
pub fn soma(df: &mut DataFrame, nome: &str) -> f64 {
    match soma_checked(df, nome) {
        Ok(v) => v,
        Err(msg) => abortar(&msg),
    }
}

/// Média de `nome`, devolvendo o erro em português em vez de abortar. Base
/// para [`media`].
pub fn media_checked(df: &mut DataFrame, nome: &str) -> Result<f64, String> {
    let coluna = buscar_coluna(df, nome)?;
    coluna
        .mean_reduce()
        .value()
        .try_extract::<f64>()
        .map_err(|_| format!("coluna '{nome}' não é numérica"))
}

/// Média de `nome`. Aborta com mensagem em português se a coluna não existir
/// ou não for numérica.
pub fn media(df: &mut DataFrame, nome: &str) -> f64 {
    match media_checked(df, nome) {
        Ok(v) => v,
        Err(msg) => abortar(&msg),
    }
}

/// Mínimo de `nome`, devolvendo o erro em português em vez de abortar. Base
/// para [`minimo`].
pub fn minimo_checked(df: &mut DataFrame, nome: &str) -> Result<f64, String> {
    let coluna = buscar_coluna(df, nome)?;
    coluna
        .min_reduce()
        .map_err(|_| format!("não foi possível calcular o mínimo da coluna '{nome}'"))?
        .value()
        .try_extract::<f64>()
        .map_err(|_| format!("coluna '{nome}' não é numérica"))
}

/// Mínimo de `nome`. Aborta com mensagem em português se a coluna não
/// existir ou não for numérica.
pub fn minimo(df: &mut DataFrame, nome: &str) -> f64 {
    match minimo_checked(df, nome) {
        Ok(v) => v,
        Err(msg) => abortar(&msg),
    }
}

/// Máximo de `nome`, devolvendo o erro em português em vez de abortar. Base
/// para [`maximo`].
pub fn maximo_checked(df: &mut DataFrame, nome: &str) -> Result<f64, String> {
    let coluna = buscar_coluna(df, nome)?;
    coluna
        .max_reduce()
        .map_err(|_| format!("não foi possível calcular o máximo da coluna '{nome}'"))?
        .value()
        .try_extract::<f64>()
        .map_err(|_| format!("coluna '{nome}' não é numérica"))
}

/// Máximo de `nome`. Aborta com mensagem em português se a coluna não
/// existir ou não for numérica.
pub fn maximo(df: &mut DataFrame, nome: &str) -> f64 {
    match maximo_checked(df, nome) {
        Ok(v) => v,
        Err(msg) => abortar(&msg),
    }
}
