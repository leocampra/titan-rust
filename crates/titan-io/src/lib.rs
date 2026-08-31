//! IO Runtime da linguagem Titan (PRD.md, T54).
//!
//! Implementa a capability `io` (`import io`): o mínimo para um compilador
//! alcançar o próprio fonte — ler um arquivo do disco. Mesmo molde de
//! `titan-data`/`titan-texto` (`crates/titan-data/src/lib.rs`,
//! `crates/titan-texto/src/lib.rs`): nenhum tipo opaco, só funções de
//! módulo, cada uma com o par `*_checked -> Result<_, String>` (mensagem em
//! português) mais o wrapper que aborta o processo — nunca `panic!` cru.
//!
//! `texto` é puro; `io` toca o sistema (decisão 3 do PRD.md, Fase 4).

/// Aborta a execução com uma mensagem em português, sem `panic!` cru — a
/// única forma de erro fatal em tempo de execução do Titan.
fn abortar(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(1);
}

/// Lê o conteúdo de `caminho` como string, devolvendo o erro em português em
/// vez de abortar. Base para [`ler_arquivo`].
pub fn ler_arquivo_checked(caminho: &str) -> Result<String, String> {
    std::fs::read_to_string(caminho)
        .map_err(|e| format!("não foi possível ler o arquivo '{caminho}': {e}"))
}

/// Lê o conteúdo de `caminho` como string. Aborta com mensagem em português
/// se o arquivo não existir, não puder ser lido ou não for UTF-8 válido.
pub fn ler_arquivo(caminho: &str) -> String {
    match ler_arquivo_checked(caminho) {
        Ok(conteudo) => conteudo,
        Err(msg) => abortar(&msg),
    }
}

/// Escreve `conteudo` em `caminho`, substituindo o arquivo se já existir,
/// devolvendo o erro em português em vez de abortar. Base para
/// [`escrever_arquivo`].
pub fn escrever_arquivo_checked(caminho: &str, conteudo: &str) -> Result<(), String> {
    std::fs::write(caminho, conteudo)
        .map_err(|e| format!("não foi possível escrever o arquivo '{caminho}': {e}"))
}

/// Escreve `conteudo` em `caminho`, substituindo o arquivo se já existir.
/// Aborta com mensagem em português se a escrita falhar.
pub fn escrever_arquivo(caminho: &str, conteudo: &str) {
    if let Err(msg) = escrever_arquivo_checked(caminho, conteudo) {
        abortar(&msg);
    }
}
