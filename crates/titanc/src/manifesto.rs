//! Leitura do manifesto `titan.toml` (PRD.md, T79).
//!
//! Até a Fase 3 o conjunto de fontes de um programa era **inferido**: um
//! arquivo `.titan` era o programa inteiro (`driver.rs`, `compile`). Com
//! módulos de usuário (decisão 4 da fase), o conjunto passa a ser
//! **declarado** — um mapa nome → caminho, não uma convenção de nome de
//! arquivo:
//!
//! ```toml
//! [pacote]
//! nome = "meucompilador"
//! principal = "src/main.titan"
//!
//! [modulos]
//! lexer  = "src/lexer.titan"
//! parser = "src/parser.titan"
//! ```
//!
//! O nome do módulo **pode diferir** do nome do arquivo (`lexer` poderia
//! apontar para `src/varredura.titan`), e todo caminho é relativo ao
//! diretório do manifesto.
//!
//! **Sobre a dependência `toml`:** ela é do workspace do compilador e
//! **nunca** entra no `Cargo.toml` gerado por programa (decisão técnica 9
//! da fase, mesma disciplina que o [ADR 0019] impôs às deps do LSP).
//! `collect_deps` (`driver.rs`) só itera `checker::imported_capabilities`,
//! e o manifesto não é uma capability — não há caminho para o vazamento.
//!
//! **Sobre o parsing:** o crate entra com `default-features = false` e só a
//! feature `parse`, o que dá [`toml::de::DeTable`] sem arrastar `serde`.
//! Desserializar via `derive` seria mais curto, mas as mensagens de campo
//! ausente e de tipo errado chegariam em inglês, sobre nomes de campo do
//! Rust — violando a convenção mais antiga do projeto. A validação aqui é
//! manual justamente para que todo erro saia em português, nomeando o campo
//! como o usuário o escreveu.
//!
//! [ADR 0019]: ../../../docs/adr/0019-lsp-sobre-tower-lsp.md

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::capabilities;

/// Nome da seção obrigatória com os metadados do pacote.
const SECAO_PACOTE: &str = "pacote";
/// Nome da seção opcional com o mapa nome → caminho dos módulos.
const SECAO_MODULOS: &str = "modulos";
/// Nome do arquivo de manifesto procurado quando se aponta um diretório.
pub const NOME_ARQUIVO: &str = "titan.toml";

/// Um módulo de usuário declarado em `[modulos]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Modulo {
    /// Nome pelo qual o programa Titan o importa (`lexer` em `import lexer`).
    pub nome: String,
    /// Caminho do arquivo `.titan`, já resolvido contra o diretório do
    /// manifesto.
    pub caminho: PathBuf,
}

/// O `titan.toml` de um programa, já validado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifesto {
    /// Diretório onde o `titan.toml` mora — a raiz contra a qual todo
    /// caminho relativo foi resolvido.
    pub diretorio: PathBuf,
    /// `pacote.nome`: o nome do programa, que o driver usa para nomear o
    /// crate gerado e o executável.
    pub nome: String,
    /// `pacote.principal`, já resolvido: o módulo que tem a `main`.
    pub principal: PathBuf,
    /// Os módulos de `[modulos]`, em ordem alfabética de nome — a mesma
    /// ordem em que o `BTreeMap` do `toml` os entrega, para que erro e
    /// emissão sejam reprodutíveis.
    pub modulos: Vec<Modulo>,
}

impl Manifesto {
    /// Busca um módulo declarado pelo nome com que o programa o importa.
    pub fn modulo(&self, nome: &str) -> Option<&Modulo> {
        self.modulos.iter().find(|m| m.nome == nome)
    }

    /// Lista os nomes dos módulos declarados, para compor mensagens de erro
    /// ("declarados: ...") no mesmo molde de
    /// [`capabilities::available_module_names`].
    pub fn nomes_de_modulo(&self) -> Vec<&str> {
        self.modulos.iter().map(|m| m.nome.as_str()).collect()
    }
}

/// Todo erro de manifesto vira uma mensagem em português, nunca panic
/// (PRD.md, convenções de trabalho).
///
/// `linha`/`coluna` só existem para o erro de sintaxe TOML e para os erros
/// que o parser localiza no fonte; um campo ausente não tem posição útil —
/// a ausência não está em lugar nenhum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestoError {
    /// Caminho do manifesto que falhou, como o usuário o vê.
    pub caminho: PathBuf,
    pub message: String,
    pub linha: Option<usize>,
    pub coluna: Option<usize>,
}

impl std::fmt::Display for ManifestoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.linha, self.coluna) {
            (Some(linha), Some(coluna)) => write!(
                f,
                "erro no manifesto '{}' (linha {}, coluna {}): {}",
                self.caminho.display(),
                linha,
                coluna,
                self.message
            ),
            _ => write!(
                f,
                "erro no manifesto '{}': {}",
                self.caminho.display(),
                self.message
            ),
        }
    }
}

impl std::error::Error for ManifestoError {}

/// Carrega o manifesto a partir de um caminho que pode ser o próprio
/// `titan.toml` **ou** o diretório que o contém — é o que deixa
/// `--manifesto DIR|ARQUIVO` (T80) ser um parâmetro só.
pub fn carregar(caminho: &Path) -> Result<Manifesto, ManifestoError> {
    let arquivo = if caminho.is_dir() {
        caminho.join(NOME_ARQUIVO)
    } else {
        caminho.to_path_buf()
    };

    let fonte = std::fs::read_to_string(&arquivo).map_err(|e| ManifestoError {
        caminho: arquivo.clone(),
        message: format!("não foi possível ler o manifesto: {e}"),
        linha: None,
        coluna: None,
    })?;

    analisar(&fonte, &arquivo)
}

/// Analisa um manifesto já lido. `arquivo` serve para as mensagens de erro e
/// para descobrir o diretório contra o qual os caminhos são resolvidos —
/// separar isso da leitura é o que permite testar todo caso de erro sem
/// tocar o disco.
pub fn analisar(fonte: &str, arquivo: &Path) -> Result<Manifesto, ManifestoError> {
    let diretorio = arquivo
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    let erro = |message: String| ManifestoError {
        caminho: arquivo.to_path_buf(),
        message,
        linha: None,
        coluna: None,
    };

    // O parser do `toml` já detecta chave duplicada, mas reporta em inglês.
    // A checagem própria vem antes justamente para que
    // `lexer = ...` duas vezes em `[modulos]` — o engano mais provável de
    // quem edita a lista à mão — saia em português.
    if let Some(dup) = chave_duplicada_em_modulos(fonte) {
        return Err(ManifestoError {
            caminho: arquivo.to_path_buf(),
            message: format!(
                "o módulo '{}' foi declarado duas vezes em [{SECAO_MODULOS}]; \
                 cada nome de módulo pode aparecer uma vez só.",
                dup.nome
            ),
            linha: Some(dup.linha),
            coluna: Some(dup.coluna),
        });
    }

    let documento = toml::de::DeTable::parse(fonte).map_err(|e| {
        let (linha, coluna) = e
            .span()
            .map(|s| posicao(fonte, s.start))
            .map_or((None, None), |(l, c)| (Some(l), Some(c)));
        ManifestoError {
            caminho: arquivo.to_path_buf(),
            message: format!("TOML malformado: {}", primeira_linha(&e.to_string())),
            linha,
            coluna,
        }
    })?;
    let documento = documento.get_ref();

    let pacote = documento
        .get(SECAO_PACOTE)
        .ok_or_else(|| erro(format!("falta a seção obrigatória [{SECAO_PACOTE}].")))?;
    let pacote = pacote.get_ref().as_table().ok_or_else(|| {
        erro(format!(
            "[{SECAO_PACOTE}] precisa ser uma seção, não {}.",
            tipo_em_portugues(pacote.get_ref())
        ))
    })?;

    let nome = campo_texto(pacote, SECAO_PACOTE, "nome", arquivo)?;
    let principal = campo_texto(pacote, SECAO_PACOTE, "principal", arquivo)?;

    let mut modulos = Vec::new();
    if let Some(secao) = documento.get(SECAO_MODULOS) {
        let tabela = secao.get_ref().as_table().ok_or_else(|| {
            erro(format!(
                "[{SECAO_MODULOS}] precisa ser uma seção, não {}.",
                tipo_em_portugues(secao.get_ref())
            ))
        })?;
        for (chave, valor) in tabela.iter() {
            let nome_modulo = chave.get_ref().as_ref();
            let caminho = valor.get_ref().as_str().ok_or_else(|| {
                erro(format!(
                    "o módulo '{nome_modulo}' precisa de um caminho em texto, não {}.",
                    tipo_em_portugues(valor.get_ref())
                ))
            })?;
            if let Some(capability) = capabilities::lookup_module(nome_modulo) {
                return Err(erro(format!(
                    "'{}' é o nome de uma capability do compilador e não pode nomear \
                     um módulo de usuário; escolha outro nome (capabilities: {}).",
                    capability.titan_name,
                    capabilities::available_module_names().join(", ")
                )));
            }
            modulos.push(Modulo {
                nome: nome_modulo.to_string(),
                caminho: diretorio.join(caminho),
            });
        }
    }

    let manifesto = Manifesto {
        diretorio: diretorio.clone(),
        nome,
        principal: diretorio.join(&principal),
        modulos,
    };

    // A existência dos arquivos é conferida **depois** de o manifesto estar
    // montado: um `titan.toml` com dois problemas reporta primeiro o
    // estrutural, que costuma ser a causa do outro.
    conferir_existe(&manifesto.principal, "o arquivo principal", arquivo)?;
    for modulo in &manifesto.modulos {
        conferir_existe(
            &modulo.caminho,
            &format!("o módulo '{}'", modulo.nome),
            arquivo,
        )?;
    }

    Ok(manifesto)
}

/// Lê um campo de texto obrigatório de uma seção, com erro em português
/// tanto para a ausência quanto para o tipo errado.
fn campo_texto(
    tabela: &toml::de::DeTable<'_>,
    secao: &str,
    campo: &str,
    arquivo: &Path,
) -> Result<String, ManifestoError> {
    let valor = tabela.get(campo).ok_or_else(|| ManifestoError {
        caminho: arquivo.to_path_buf(),
        message: format!("falta o campo obrigatório '{campo}' em [{secao}]."),
        linha: None,
        coluna: None,
    })?;
    let texto = valor.get_ref().as_str().ok_or_else(|| ManifestoError {
        caminho: arquivo.to_path_buf(),
        message: format!(
            "o campo '{campo}' em [{secao}] precisa ser um texto, não {}.",
            tipo_em_portugues(valor.get_ref())
        ),
        linha: None,
        coluna: None,
    })?;
    if texto.trim().is_empty() {
        return Err(ManifestoError {
            caminho: arquivo.to_path_buf(),
            message: format!("o campo '{campo}' em [{secao}] não pode ser vazio."),
            linha: None,
            coluna: None,
        });
    }
    Ok(texto.to_string())
}

/// Confere que um caminho declarado existe de fato, nomeando o papel dele
/// ("o módulo 'lexer'", "o arquivo principal") na mensagem.
fn conferir_existe(caminho: &Path, papel: &str, arquivo: &Path) -> Result<(), ManifestoError> {
    if caminho.is_file() {
        return Ok(());
    }
    Err(ManifestoError {
        caminho: arquivo.to_path_buf(),
        message: format!(
            "{papel} aponta para '{}', que não existe.",
            caminho.display()
        ),
        linha: None,
        coluna: None,
    })
}

/// Um nome de módulo declarado mais de uma vez, com a posição da segunda
/// ocorrência.
struct Duplicata {
    nome: String,
    linha: usize,
    coluna: usize,
}

/// Varre o fonte procurando uma chave repetida dentro de `[modulos]`.
///
/// É uma varredura de linhas, não um parser: basta para o caso que interessa
/// (`lexer = "..."` escrito duas vezes) e, se escapar algo mais exótico — uma
/// chave entre aspas, uma tabela inline —, o parser do `toml` ainda pega o
/// caso logo depois. O preço de escapar é uma mensagem em inglês, não um
/// manifesto inválido aceito.
fn chave_duplicada_em_modulos(fonte: &str) -> Option<Duplicata> {
    let mut dentro = false;
    let mut vistos: BTreeMap<&str, ()> = BTreeMap::new();
    for (indice, linha) in fonte.lines().enumerate() {
        let limpa = linha.trim();
        if limpa.starts_with('[') {
            dentro = limpa == format!("[{SECAO_MODULOS}]");
            continue;
        }
        if !dentro || limpa.is_empty() || limpa.starts_with('#') {
            continue;
        }
        let Some(igual) = limpa.find('=') else {
            continue;
        };
        let chave = limpa[..igual].trim();
        if chave.is_empty() {
            continue;
        }
        if vistos.insert(chave, ()).is_some() {
            let coluna = linha.len() - linha.trim_start().len() + 1;
            return Some(Duplicata {
                nome: chave.to_string(),
                linha: indice + 1,
                coluna,
            });
        }
    }
    None
}

/// Converte um deslocamento em bytes na linha/coluna 1-indexada que o
/// usuário enxerga — a coluna conta **caracteres**, como `ast::Loc` (ADR
/// 0019).
fn posicao(fonte: &str, deslocamento: usize) -> (usize, usize) {
    let ate = &fonte[..deslocamento.min(fonte.len())];
    let linha = ate.matches('\n').count() + 1;
    let inicio = ate.rfind('\n').map_or(0, |i| i + 1);
    let coluna = fonte[inicio..deslocamento.min(fonte.len())].chars().count() + 1;
    (linha, coluna)
}

/// O erro do `toml` vem multilinha, com um trecho do fonte desenhado em
/// ASCII; a primeira linha é o que interessa, já que a posição sai no
/// prefixo em português.
fn primeira_linha(texto: &str) -> String {
    texto
        .lines()
        .next_back()
        .filter(|l| !l.trim().is_empty())
        .unwrap_or(texto)
        .trim()
        .to_string()
}

/// Nome em português do tipo de um valor TOML, para as mensagens de tipo
/// errado.
fn tipo_em_portugues(valor: &toml::de::DeValue<'_>) -> &'static str {
    match valor {
        toml::de::DeValue::String(_) => "um texto",
        toml::de::DeValue::Integer(_) => "um inteiro",
        toml::de::DeValue::Float(_) => "um número com ponto",
        toml::de::DeValue::Boolean(_) => "um booleano",
        toml::de::DeValue::Datetime(_) => "uma data",
        toml::de::DeValue::Array(_) => "uma lista",
        toml::de::DeValue::Table(_) => "uma seção",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cria um diretório temporário próprio do teste e escreve nele os
    /// arquivos `.titan` que o manifesto vai declarar — a validação de
    /// existência toca o disco, então os fontes precisam existir de verdade.
    fn cenario(rotulo: &str, arquivos: &[&str]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "titanc-manifesto-test-{rotulo}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).expect("cria diretório temporário de teste");
        for arquivo in arquivos {
            std::fs::write(dir.join(arquivo), "-- fonte de teste\n").expect("escreve fonte");
        }
        dir
    }

    /// Analisa um fonte de manifesto contra o diretório do cenário.
    fn analisar_em(dir: &Path, fonte: &str) -> Result<Manifesto, ManifestoError> {
        analisar(fonte, &dir.join(NOME_ARQUIVO))
    }

    const FELIZ: &str = r#"
[pacote]
nome = "meucompilador"
principal = "src/main.titan"

[modulos]
lexer  = "src/lexer.titan"
parser = "src/parser.titan"
"#;

    #[test]
    fn parse_feliz_devolve_nome_principal_e_modulos() {
        let dir = cenario(
            "feliz",
            &["src/main.titan", "src/lexer.titan", "src/parser.titan"],
        );
        let manifesto = analisar_em(&dir, FELIZ).expect("manifesto válido");

        assert_eq!(manifesto.nome, "meucompilador");
        assert_eq!(manifesto.principal, dir.join("src/main.titan"));
        assert_eq!(manifesto.diretorio, dir);
        assert_eq!(manifesto.nomes_de_modulo(), vec!["lexer", "parser"]);
        assert_eq!(
            manifesto.modulo("lexer").map(|m| m.caminho.clone()),
            Some(dir.join("src/lexer.titan"))
        );
        assert_eq!(manifesto.modulo("inexistente"), None);
    }

    #[test]
    fn nome_do_modulo_pode_diferir_do_nome_do_arquivo() {
        let dir = cenario("alias", &["src/main.titan", "src/varredura.titan"]);
        let manifesto = analisar_em(
            &dir,
            r#"
[pacote]
nome = "p"
principal = "src/main.titan"

[modulos]
lexer = "src/varredura.titan"
"#,
        )
        .expect("manifesto válido");

        assert_eq!(
            manifesto.modulo("lexer").map(|m| m.caminho.clone()),
            Some(dir.join("src/varredura.titan"))
        );
    }

    #[test]
    fn secao_modulos_e_opcional() {
        let dir = cenario("sem-modulos", &["src/main.titan"]);
        let manifesto = analisar_em(
            &dir,
            r#"
[pacote]
nome = "p"
principal = "src/main.titan"
"#,
        )
        .expect("manifesto válido");

        assert!(manifesto.modulos.is_empty());
    }

    #[test]
    fn manifesto_malformado_da_erro_em_portugues_com_posicao() {
        let dir = cenario("malformado", &["src/main.titan"]);
        let erro = analisar_em(&dir, "[pacote\nnome = \"p\"\n").expect_err("TOML inválido");

        assert!(
            erro.message.starts_with("TOML malformado:"),
            "mensagem inesperada: {}",
            erro.message
        );
        assert_eq!(erro.linha, Some(1));
        assert_eq!(
            erro.to_string(),
            format!(
                "erro no manifesto '{}' (linha 1, coluna 8): TOML malformado: unclosed table, expected `]`",
                dir.join(NOME_ARQUIVO).display()
            )
        );
    }

    #[test]
    fn secao_pacote_ausente_da_erro_claro() {
        let dir = cenario("sem-pacote", &["src/main.titan"]);
        let erro = analisar_em(&dir, "[modulos]\n").expect_err("falta [pacote]");

        assert_eq!(erro.message, "falta a seção obrigatória [pacote].");
    }

    #[test]
    fn campo_nome_ausente_da_erro_claro() {
        let dir = cenario("sem-nome", &["src/main.titan"]);
        let erro = analisar_em(&dir, "[pacote]\nprincipal = \"src/main.titan\"\n")
            .expect_err("falta 'nome'");

        assert_eq!(
            erro.message,
            "falta o campo obrigatório 'nome' em [pacote]."
        );
    }

    #[test]
    fn campo_principal_ausente_da_erro_claro() {
        let dir = cenario("sem-principal", &["src/main.titan"]);
        let erro = analisar_em(&dir, "[pacote]\nnome = \"p\"\n").expect_err("falta 'principal'");

        assert_eq!(
            erro.message,
            "falta o campo obrigatório 'principal' em [pacote]."
        );
    }

    #[test]
    fn campo_com_tipo_errado_da_erro_nomeando_o_tipo_em_portugues() {
        let dir = cenario("tipo-errado", &["src/main.titan"]);
        let erro = analisar_em(
            &dir,
            "[pacote]\nnome = 42\nprincipal = \"src/main.titan\"\n",
        )
        .expect_err("'nome' não é texto");

        assert_eq!(
            erro.message,
            "o campo 'nome' em [pacote] precisa ser um texto, não um inteiro."
        );
    }

    #[test]
    fn campo_vazio_da_erro_claro() {
        let dir = cenario("vazio", &["src/main.titan"]);
        let erro = analisar_em(
            &dir,
            "[pacote]\nnome = \"\"\nprincipal = \"src/main.titan\"\n",
        )
        .expect_err("'nome' vazio");

        assert_eq!(
            erro.message,
            "o campo 'nome' em [pacote] não pode ser vazio."
        );
    }

    #[test]
    fn caminho_inexistente_do_principal_da_erro_claro() {
        let dir = cenario("principal-sumido", &[]);
        let erro = analisar_em(
            &dir,
            "[pacote]\nnome = \"p\"\nprincipal = \"src/main.titan\"\n",
        )
        .expect_err("principal não existe");

        assert_eq!(
            erro.message,
            format!(
                "o arquivo principal aponta para '{}', que não existe.",
                dir.join("src/main.titan").display()
            )
        );
    }

    #[test]
    fn caminho_inexistente_de_modulo_da_erro_claro() {
        let dir = cenario("modulo-sumido", &["src/main.titan"]);
        let erro = analisar_em(&dir, FELIZ).expect_err("módulos não existem");

        assert_eq!(
            erro.message,
            format!(
                "o módulo 'lexer' aponta para '{}', que não existe.",
                dir.join("src/lexer.titan").display()
            )
        );
    }

    #[test]
    fn nome_de_modulo_duplicado_da_erro_em_portugues() {
        let dir = cenario("duplicado", &["src/main.titan", "src/lexer.titan"]);
        let erro = analisar_em(
            &dir,
            "[pacote]\nnome = \"p\"\nprincipal = \"src/main.titan\"\n\n[modulos]\nlexer = \"src/lexer.titan\"\nlexer = \"src/outro.titan\"\n",
        )
        .expect_err("módulo duplicado");

        assert_eq!(
            erro.message,
            "o módulo 'lexer' foi declarado duas vezes em [modulos]; \
             cada nome de módulo pode aparecer uma vez só."
        );
        assert_eq!(erro.linha, Some(7));
        assert_eq!(erro.coluna, Some(1));
    }

    #[test]
    fn nome_de_modulo_colidindo_com_capability_da_erro_claro() {
        let dir = cenario("colisao", &["src/main.titan", "src/texto.titan"]);
        let erro = analisar_em(
            &dir,
            "[pacote]\nnome = \"p\"\nprincipal = \"src/main.titan\"\n\n[modulos]\ntexto = \"src/texto.titan\"\n",
        )
        .expect_err("colide com capability");

        assert_eq!(
            erro.message,
            "'texto' é o nome de uma capability do compilador e não pode nomear \
             um módulo de usuário; escolha outro nome (capabilities: data, texto, io)."
        );
    }

    #[test]
    fn toda_capability_esta_protegida_contra_colisao() {
        // O teste acima fixa uma capability; este prova que a proteção vem da
        // tabela, e não de uma lista escrita à mão aqui — uma capability nova
        // (`rede`, um dia) fica protegida sem mudança neste arquivo.
        let dir = cenario("colisao-todas", &["src/main.titan", "src/x.titan"]);
        for nome in capabilities::available_module_names() {
            let fonte = format!(
                "[pacote]\nnome = \"p\"\nprincipal = \"src/main.titan\"\n\n[modulos]\n{nome} = \"src/x.titan\"\n"
            );
            let erro = analisar_em(&dir, &fonte)
                .expect_err("capability não pode nomear módulo de usuário");
            assert!(
                erro.message
                    .starts_with(&format!("'{nome}' é o nome de uma capability")),
                "mensagem inesperada para '{nome}': {}",
                erro.message
            );
        }
    }

    #[test]
    fn carregar_aceita_diretorio_ou_arquivo() {
        let dir = cenario(
            "carregar",
            &["src/main.titan", "src/lexer.titan", "src/parser.titan"],
        );
        std::fs::write(dir.join(NOME_ARQUIVO), FELIZ).expect("escreve titan.toml");

        let por_diretorio = carregar(&dir).expect("carrega pelo diretório");
        let por_arquivo = carregar(&dir.join(NOME_ARQUIVO)).expect("carrega pelo arquivo");

        assert_eq!(por_diretorio, por_arquivo);
        assert_eq!(por_diretorio.nome, "meucompilador");
    }

    #[test]
    fn manifesto_inexistente_da_erro_claro() {
        let dir = cenario("sem-arquivo", &[]);
        let erro = carregar(&dir).expect_err("não há titan.toml");

        assert!(
            erro.message
                .starts_with("não foi possível ler o manifesto:"),
            "mensagem inesperada: {}",
            erro.message
        );
        assert_eq!(erro.caminho, dir.join(NOME_ARQUIVO));
    }
}
