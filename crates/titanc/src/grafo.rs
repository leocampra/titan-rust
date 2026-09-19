//! Resolução do conjunto de fontes e ordenação topológica dos módulos
//! (PRD.md, T80).
//!
//! Até a T79 o compilador lia **um** arquivo: `compile` (`driver.rs`) fazia
//! um `read_to_string` de `opts.input` e o programa inteiro cabia ali. A
//! T79 deu ao programa um conjunto de fontes **declarado**
//! ([`crate::manifesto`]); esta etapa é quem o transforma num **grafo**:
//! cada módulo declarado é um vértice, cada `import` de um módulo de usuário
//! é uma aresta, e a ordem topológica define a ordem de checagem (T81) e de
//! emissão (T82).
//!
//! **O caminho de arquivo único continua existindo.** Sem manifesto,
//! [`resolver`] devolve um grafo de um vértice só, cujo fonte é o arquivo
//! apontado e cujo nome de programa sai do stem — exatamente o que
//! `hello.titan`, `nucleo.titan`, `compostos.titan`, `dados.titan` e
//! `lexer.titan` sempre tiveram. É o que preserva o byte-a-byte exigido pelo
//! critério de aceite da T80.
//!
//! **Sobre ler as arestas do `TopLevelImport`, e não do manifesto:** o
//! manifesto declara quais módulos *existem*, não quem depende de quem. As
//! arestas vêm do fonte, porque é o `import` que cria a dependência de
//! verdade — um módulo declarado e nunca importado entra no grafo como
//! vértice isolado, e não como dependência de ninguém. A alternativa
//! (declarar dependências no `titan.toml`) duplicaria no manifesto uma
//! informação que o fonte já tem, e as duas divergiriam no primeiro
//! `import` apagado.
//!
//! **Sobre lexar e parsear aqui:** as arestas só se conhecem depois do
//! parse, e reparsear em `compile` seria trabalho repetido sobre um fonte
//! grande como `lexer.titan`. Por isso [`Modulo`] carrega o
//! [`ast::Program`] já parseado junto do fonte — o checker (T81) recebe o
//! grafo pronto.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::ast::{self, TopLevel};
use crate::capabilities;
use crate::lexer::{self, LexError};
use crate::manifesto::{self, Manifesto, ManifestoError};
use crate::parser::{self, ParseError};

/// Um módulo já lido e parseado, pronto para o checker.
#[derive(Debug)]
pub struct Modulo {
    /// Nome pelo qual os outros módulos o importam (`lexer` em
    /// `import lexer`). Para o programa principal é o nome declarado em
    /// `pacote.nome` — ou, sem manifesto, o stem do arquivo.
    pub nome: String,
    /// Caminho do `.titan`, para as mensagens de erro.
    pub caminho: PathBuf,
    /// O fonte lido, que o LSP e as mensagens de erro ainda precisam.
    pub fonte: String,
    /// A AST, parseada uma vez só.
    pub programa: ast::Program,
    /// Os módulos **de usuário** que este importa, na ordem do fonte e sem
    /// repetição. Capabilities (`io`, `texto`, `data`) não entram: elas não
    /// são vértices do grafo, são dependências do `Cargo.toml` (T43).
    pub dependencias: Vec<String>,
}

/// O conjunto de fontes de um programa, já resolvido e ordenado.
#[derive(Debug)]
pub struct Grafo {
    /// Nome do programa — o do executável e o do diretório de build.
    pub nome: String,
    /// Todos os módulos em **ordem topológica**: uma dependência sempre
    /// aparece antes de quem a importa, e o principal é o último.
    pub modulos: Vec<Modulo>,
    /// Índice do módulo principal (o que tem a `main`) dentro de
    /// [`Grafo::modulos`].
    pub principal: usize,
}

impl Grafo {
    /// O módulo que tem a `main`.
    pub fn principal(&self) -> &Modulo {
        &self.modulos[self.principal]
    }

    /// Busca um módulo pelo nome com que os outros o importam.
    pub fn modulo(&self, nome: &str) -> Option<&Modulo> {
        self.modulos.iter().find(|m| m.nome == nome)
    }

    /// Verdadeiro quando o programa é um arquivo só, sem manifesto — o
    /// caminho que a T80 precisa deixar intacto.
    pub fn arquivo_unico(&self) -> bool {
        self.modulos.len() == 1
    }
}

/// Tudo que pode dar errado ao montar o grafo vira mensagem em português,
/// nunca panic (PRD.md, convenções de trabalho).
#[derive(Debug)]
pub enum GrafoError {
    /// O `titan.toml` não pôde ser lido ou é inválido (T79).
    Manifesto(ManifestoError),
    /// Um módulo do grafo não passou no lexer.
    Lex {
        caminho: PathBuf,
        source: LexError,
        /// Verdadeiro quando o programa tem manifesto, e portanto pode ter
        /// mais de um arquivo — é o que decide se o caminho entra na
        /// mensagem ou seria só ruído (ver `CompileError::from`).
        multi_modulo: bool,
    },
    /// Um módulo do grafo não passou no parser.
    Parse {
        caminho: PathBuf,
        source: ParseError,
        /// Ver [`GrafoError::Lex::multi_modulo`].
        multi_modulo: bool,
    },
    /// Um `.titan` declarado não pôde ser lido do disco.
    Io {
        caminho: PathBuf,
        source: std::io::Error,
    },
    /// Um `import` aponta para um nome que não é capability nem módulo
    /// declarado no manifesto.
    ModuloNaoDeclarado {
        /// O módulo cujo `import` não resolveu.
        de: String,
        /// O nome importado que não existe.
        nome: String,
        loc: ast::Loc,
        /// Os nomes declarados em `[modulos]`, para a mensagem.
        declarados: Vec<String>,
    },
    /// Os `import` fecham um ciclo. O caminho vem completo, começando e
    /// terminando no mesmo módulo (`a → b → a`).
    Ciclo { caminho: Vec<String> },
}

impl std::fmt::Display for GrafoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GrafoError::Manifesto(e) => write!(f, "{e}"),
            GrafoError::Lex {
                caminho, source, ..
            } => {
                write!(f, "em '{}': {source}", caminho.display())
            }
            GrafoError::Parse {
                caminho, source, ..
            } => {
                write!(f, "em '{}': {source}", caminho.display())
            }
            GrafoError::Io { caminho, source } => {
                write!(f, "não foi possível ler '{}': {source}", caminho.display())
            }
            GrafoError::ModuloNaoDeclarado {
                de,
                nome,
                loc,
                declarados,
            } => {
                // A mensagem lista as **duas** fontes de módulo, capability e
                // manifesto, porque quem escreveu `import lexer` não sabe (nem
                // precisa saber) em qual das duas o compilador procurou.
                let capabilities = capabilities::available_module_names().join(", ");
                let declarados = if declarados.is_empty() {
                    "nenhum módulo declarado em [modulos]".to_string()
                } else {
                    format!("declarados no manifesto: {}", declarados.join(", "))
                };
                write!(
                    f,
                    "em '{de}' (linha {}, coluna {}): o módulo '{nome}' não existe; \
                     capabilities: {capabilities}; {declarados}.",
                    loc.line, loc.col
                )
            }
            GrafoError::Ciclo { caminho } => {
                write!(
                    f,
                    "os imports formam um ciclo: {}; um módulo não pode depender \
                     (direta ou indiretamente) de si mesmo.",
                    caminho.join(" → ")
                )
            }
        }
    }
}

impl std::error::Error for GrafoError {}

impl From<ManifestoError> for GrafoError {
    fn from(e: ManifestoError) -> Self {
        GrafoError::Manifesto(e)
    }
}

/// Procura um `titan.toml` para `entrada`.
///
/// Com `--manifesto DIR|ARQUIVO` explícito, é erro o manifesto não existir —
/// quem pediu um manifesto quer saber que ele não está lá. Sem a opção, a
/// busca é **silenciosa**: olha ao lado do arquivo de entrada e, não achando,
/// devolve `None`, que é o caminho de arquivo único.
///
/// A busca não sobe diretórios de propósito. Um `titan.toml` esquecido dois
/// níveis acima mudaria em silêncio o que `titanc nucleo.titan` compila; o
/// custo de não subir é um `--manifesto` a digitar, e ele é explícito.
pub fn localizar_manifesto(
    entrada: &Path,
    explicito: Option<&Path>,
) -> Result<Option<Manifesto>, GrafoError> {
    if let Some(caminho) = explicito {
        return manifesto::carregar(caminho)
            .map(Some)
            .map_err(GrafoError::from);
    }

    let ao_lado = entrada
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .join(manifesto::NOME_ARQUIVO);

    if ao_lado.is_file() {
        manifesto::carregar(&ao_lado)
            .map(Some)
            .map_err(GrafoError::from)
    } else {
        Ok(None)
    }
}

/// Monta o grafo de um programa.
///
/// Com manifesto, o principal e os módulos declarados são lidos, parseados e
/// ordenados topologicamente. Sem manifesto, o resultado é o grafo de um
/// vértice do caminho de arquivo único.
pub fn resolver(entrada: &Path, manifesto: Option<&Manifesto>) -> Result<Grafo, GrafoError> {
    match manifesto {
        Some(manifesto) => resolver_com_manifesto(manifesto),
        None => resolver_arquivo_unico(entrada),
    }
}

/// O caminho de sempre: um arquivo, um vértice, nome vindo do stem.
fn resolver_arquivo_unico(entrada: &Path) -> Result<Grafo, GrafoError> {
    let nome = entrada
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("programa")
        .to_string();
    let modulo = ler_modulo(&nome, entrada, &[], false)?;
    Ok(Grafo {
        nome,
        modulos: vec![modulo],
        principal: 0,
    })
}

/// O caminho novo: lê o principal e cada módulo declarado, valida os
/// `import` e ordena.
fn resolver_com_manifesto(manifesto: &Manifesto) -> Result<Grafo, GrafoError> {
    let declarados: Vec<String> = manifesto
        .nomes_de_modulo()
        .into_iter()
        .map(str::to_string)
        .collect();

    // O principal entra com o nome do pacote, e não com o stem do arquivo:
    // é o nome do programa que o usuário declarou, e usá-lo aqui mantém
    // `pacote.nome` como a única fonte do nome do executável.
    let mut modulos = vec![ler_modulo(
        &manifesto.nome,
        &manifesto.principal,
        &declarados,
        true,
    )?];
    for declarado in &manifesto.modulos {
        modulos.push(ler_modulo(
            &declarado.nome,
            &declarado.caminho,
            &declarados,
            true,
        )?);
    }

    // O principal é o vértice 0 antes da ordenação; depois dela vai para o
    // fim, porque nada o importa (ele não é declarado em `[modulos]`, logo
    // nenhum `import` pode nomeá-lo).
    let ordem = ordenar(&modulos)?;
    let modulos = reordenar(modulos, &ordem);
    let principal = modulos
        .iter()
        .position(|m| m.nome == manifesto.nome)
        .expect("o principal está no grafo: acabou de ser inserido");

    Ok(Grafo {
        nome: manifesto.nome.clone(),
        modulos,
        principal,
    })
}

/// Lê, lexa e parseia um módulo, e extrai dele as arestas do grafo.
///
/// `multi_modulo` diz se há manifesto. Ele decide duas coisas: se o caminho
/// do arquivo entra nas mensagens de erro léxico e sintático (ver
/// `CompileError::from`), e se um `import` que não resolve é erro do grafo
/// ou fica para o checker (ver [`dependencias_de`]).
fn ler_modulo(
    nome: &str,
    caminho: &Path,
    declarados: &[String],
    multi_modulo: bool,
) -> Result<Modulo, GrafoError> {
    let fonte = std::fs::read_to_string(caminho).map_err(|source| GrafoError::Io {
        caminho: caminho.to_path_buf(),
        source,
    })?;
    let tokens = lexer::lex(&fonte).map_err(|source| GrafoError::Lex {
        caminho: caminho.to_path_buf(),
        source,
        multi_modulo,
    })?;
    let programa = parser::parse(&tokens).map_err(|source| GrafoError::Parse {
        caminho: caminho.to_path_buf(),
        source,
        multi_modulo,
    })?;
    let dependencias = dependencias_de(nome, &programa, declarados, multi_modulo)?;

    Ok(Modulo {
        nome: nome.to_string(),
        caminho: caminho.to_path_buf(),
        fonte,
        programa,
        dependencias,
    })
}

/// Extrai de um programa os nomes dos **módulos de usuário** que ele
/// importa, na ordem do fonte e sem repetição.
///
/// A capability vem primeiro na busca, espelhando a ordem que o checker vai
/// usar na T81 (`lookup_module` e só então o manifesto) — o nome de um
/// módulo de usuário nunca chega aqui colidindo com capability, porque a T79
/// já rejeita a colisão no `titan.toml`.
///
/// O `import` é resolvido por `modname`, e a aresta guarda `modname`: o
/// `localname` (o alias da T72) é assunto do escopo de quem importa, não do
/// grafo — dois módulos podem importar `lexer` com aliases diferentes e
/// ainda assim é um vértice só.
///
/// **Sem manifesto (`multi_modulo: false`) o import que não resolve não é
/// erro aqui.** Um arquivo solto com `import inexistente` tem de continuar
/// recebendo a mensagem do checker ("capability 'inexistente' não existe;
/// disponíveis: ..."), que é a certa: falar de `[modulos]` a quem não tem
/// `titan.toml` seria apontar para uma seção de um arquivo que não existe.
/// O grafo só assume a resolução quando há um manifesto para resolver
/// contra.
fn dependencias_de(
    de: &str,
    programa: &ast::Program,
    declarados: &[String],
    multi_modulo: bool,
) -> Result<Vec<String>, GrafoError> {
    let mut vistos = HashSet::new();
    let mut dependencias = Vec::new();

    for node in programa {
        let TopLevel::TopLevelImport { loc, modname, .. } = node else {
            continue;
        };
        if capabilities::lookup_module(modname).is_some() {
            continue;
        }
        if !declarados.iter().any(|d| d == modname) {
            if !multi_modulo {
                continue;
            }
            return Err(GrafoError::ModuloNaoDeclarado {
                de: de.to_string(),
                nome: modname.clone(),
                loc: *loc,
                declarados: declarados.to_vec(),
            });
        }
        if vistos.insert(modname.clone()) {
            dependencias.push(modname.clone());
        }
    }

    Ok(dependencias)
}

/// Ordena os módulos topologicamente, devolvendo os índices na ordem em que
/// devem ser checados e emitidos.
///
/// DFS pós-ordem com três cores, como a detecção de record recursivo
/// (`checker.rs`, `find_recursive_record`). A diferença é o que se faz ao
/// achar o ciclo: lá basta o nome de um record envolvido, aqui o PRD pede o
/// **caminho** (`a → b → a`). Por isso a DFS carrega uma pilha dos módulos
/// em cinza: o ciclo é o sufixo dela a partir do módulo reencontrado, mais
/// ele próprio de novo no fim, fechando a volta.
fn ordenar(modulos: &[Modulo]) -> Result<Vec<usize>, GrafoError> {
    #[derive(Clone, Copy, PartialEq)]
    enum Cor {
        Branco,
        Cinza,
        Preto,
    }

    let indice: HashMap<&str, usize> = modulos
        .iter()
        .enumerate()
        .map(|(i, m)| (m.nome.as_str(), i))
        .collect();

    let mut cores = vec![Cor::Branco; modulos.len()];
    let mut ordem = Vec::with_capacity(modulos.len());
    let mut pilha: Vec<usize> = Vec::new();

    fn visitar(
        atual: usize,
        modulos: &[Modulo],
        indice: &HashMap<&str, usize>,
        cores: &mut [Cor],
        ordem: &mut Vec<usize>,
        pilha: &mut Vec<usize>,
    ) -> Result<(), GrafoError> {
        match cores[atual] {
            Cor::Preto => return Ok(()),
            Cor::Cinza => {
                let inicio = pilha
                    .iter()
                    .position(|&i| i == atual)
                    .expect("um módulo cinza está na pilha da DFS");
                let mut caminho: Vec<String> = pilha[inicio..]
                    .iter()
                    .map(|&i| modulos[i].nome.clone())
                    .collect();
                caminho.push(modulos[atual].nome.clone());
                return Err(GrafoError::Ciclo { caminho });
            }
            Cor::Branco => {}
        }

        cores[atual] = Cor::Cinza;
        pilha.push(atual);
        for dependencia in &modulos[atual].dependencias {
            // `dependencias_de` já rejeitou todo nome não declarado, então
            // todo nome que chega aqui está no índice.
            let vizinho = indice[dependencia.as_str()];
            visitar(vizinho, modulos, indice, cores, ordem, pilha)?;
        }
        pilha.pop();
        cores[atual] = Cor::Preto;
        ordem.push(atual);
        Ok(())
    }

    // A varredura começa pelo vértice 0 (o principal) para que a ordem
    // reflita as dependências de verdade antes dos módulos órfãos; os
    // declarados e nunca importados entram depois, em ordem de declaração.
    for inicio in 0..modulos.len() {
        visitar(inicio, modulos, &indice, &mut cores, &mut ordem, &mut pilha)?;
    }

    Ok(ordem)
}

/// Aplica a permutação de [`ordenar`] ao vetor de módulos, consumindo-o —
/// um `Option::take` por posição evita clonar fonte e AST de cada módulo.
fn reordenar(modulos: Vec<Modulo>, ordem: &[usize]) -> Vec<Modulo> {
    let mut slots: Vec<Option<Modulo>> = modulos.into_iter().map(Some).collect();
    ordem
        .iter()
        .map(|&i| {
            slots[i]
                .take()
                .expect("cada índice aparece uma vez na ordem")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Monta um diretório temporário com um `titan.toml` e os fontes que ele
    /// declara — a resolução toca o disco de verdade.
    fn cenario(rotulo: &str, arquivos: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "titanc-grafo-test-{rotulo}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).expect("cria diretório temporário de teste");
        for (nome, conteudo) in arquivos {
            std::fs::write(dir.join(nome), conteudo).expect("escreve arquivo do cenário");
        }
        dir
    }

    /// Um fonte mínimo que passa no lexer e no parser, com os `import`
    /// pedidos no topo.
    fn fonte(imports: &[&str]) -> String {
        let mut out = String::new();
        for import in imports {
            out.push_str(&format!("import {import}\n"));
        }
        out.push_str("function nada(): integer\n    return 0\nend\n");
        out
    }

    fn manifesto_com(modulos: &[(&str, &str)]) -> String {
        let mut out = String::from(
            "[pacote]\nnome = \"prog\"\nprincipal = \"src/main.titan\"\n\n[modulos]\n",
        );
        for (nome, caminho) in modulos {
            out.push_str(&format!("{nome} = \"{caminho}\"\n"));
        }
        out
    }

    fn grafo_de(dir: &Path) -> Result<Grafo, GrafoError> {
        let manifesto = manifesto::carregar(dir).expect("manifesto válido");
        resolver(&manifesto.principal.clone(), Some(&manifesto))
    }

    #[test]
    fn sem_manifesto_o_grafo_tem_um_vertice_so() {
        let dir = cenario("unico", &[("src/main.titan", &fonte(&[]))]);
        let entrada = dir.join("src/main.titan");

        assert!(
            localizar_manifesto(&entrada, None)
                .expect("busca não falha")
                .is_none()
        );

        let grafo = resolver(&entrada, None).expect("arquivo único resolve");
        assert!(grafo.arquivo_unico());
        assert_eq!(grafo.nome, "main");
        assert_eq!(grafo.principal().caminho, entrada);
        assert!(grafo.principal().dependencias.is_empty());
    }

    #[test]
    fn manifesto_ao_lado_do_arquivo_e_encontrado_sozinho() {
        // O manifesto mora em `src/`, ao lado do fonte, e por isso declara o
        // principal com caminho relativo a `src/` — não a raiz do cenário.
        let dir = cenario(
            "ao-lado",
            &[
                ("src/main.titan", &fonte(&[])),
                (
                    "src/titan.toml",
                    "[pacote]\nnome = \"prog\"\nprincipal = \"main.titan\"\n",
                ),
            ],
        );

        let achado = localizar_manifesto(&dir.join("src/main.titan"), None)
            .expect("busca não falha")
            .expect("há um titan.toml ao lado");

        assert_eq!(achado.nome, "prog");
        assert_eq!(achado.principal, dir.join("src/main.titan"));
    }

    #[test]
    fn busca_nao_sobe_diretorios() {
        // O `titan.toml` fica na raiz do cenário, e a entrada em `src/`.
        let dir = cenario(
            "nao-sobe",
            &[
                ("src/main.titan", &fonte(&[])),
                ("titan.toml", &manifesto_com(&[])),
            ],
        );

        assert!(
            localizar_manifesto(&dir.join("src/main.titan"), None)
                .expect("busca não falha")
                .is_none(),
            "a busca não deve subir até a raiz do projeto"
        );
    }

    #[test]
    fn manifesto_explicito_inexistente_da_erro() {
        let dir = cenario("explicito-sumido", &[("src/main.titan", &fonte(&[]))]);

        let erro = localizar_manifesto(&dir.join("src/main.titan"), Some(&dir))
            .expect_err("não há titan.toml");

        assert!(
            erro.to_string()
                .contains("não foi possível ler o manifesto"),
            "mensagem inesperada: {erro}"
        );
    }

    #[test]
    fn tres_modulos_saem_em_ordem_topologica() {
        // main → parser → lexer; lexer não depende de ninguém.
        let dir = cenario(
            "tres",
            &[
                ("src/main.titan", &fonte(&["parser"])),
                ("src/parser.titan", &fonte(&["lexer"])),
                ("src/lexer.titan", &fonte(&[])),
                (
                    "titan.toml",
                    &manifesto_com(&[("lexer", "src/lexer.titan"), ("parser", "src/parser.titan")]),
                ),
            ],
        );

        let grafo = grafo_de(&dir).expect("grafo de três módulos");

        let nomes: Vec<&str> = grafo.modulos.iter().map(|m| m.nome.as_str()).collect();
        assert_eq!(nomes, vec!["lexer", "parser", "prog"]);
        assert_eq!(grafo.principal().nome, "prog");
        assert_eq!(grafo.modulo("parser").unwrap().dependencias, vec!["lexer"]);
        assert!(!grafo.arquivo_unico());
    }

    #[test]
    fn capability_nao_vira_aresta_do_grafo() {
        let dir = cenario(
            "capability",
            &[
                ("src/main.titan", &fonte(&["io", "texto", "lexer"])),
                ("src/lexer.titan", &fonte(&["io"])),
                (
                    "titan.toml",
                    &manifesto_com(&[("lexer", "src/lexer.titan")]),
                ),
            ],
        );

        let grafo = grafo_de(&dir).expect("grafo com capabilities");

        assert_eq!(grafo.principal().dependencias, vec!["lexer"]);
        assert!(grafo.modulo("lexer").unwrap().dependencias.is_empty());
    }

    #[test]
    fn import_repetido_vira_uma_aresta_so() {
        let dir = cenario(
            "repetido",
            &[
                ("src/main.titan", &fonte(&["lexer", "lexer"])),
                ("src/lexer.titan", &fonte(&[])),
                (
                    "titan.toml",
                    &manifesto_com(&[("lexer", "src/lexer.titan")]),
                ),
            ],
        );

        let grafo = grafo_de(&dir).expect("grafo resolve");

        assert_eq!(grafo.principal().dependencias, vec!["lexer"]);
    }

    #[test]
    fn modulo_declarado_e_nunca_importado_entra_como_vertice_isolado() {
        let dir = cenario(
            "orfao",
            &[
                ("src/main.titan", &fonte(&[])),
                ("src/orfao.titan", &fonte(&[])),
                (
                    "titan.toml",
                    &manifesto_com(&[("orfao", "src/orfao.titan")]),
                ),
            ],
        );

        let grafo = grafo_de(&dir).expect("grafo resolve");

        assert_eq!(grafo.modulos.len(), 2);
        assert!(grafo.modulo("orfao").is_some());
    }

    #[test]
    fn ciclo_direto_nomeia_o_caminho() {
        let dir = cenario(
            "ciclo-direto",
            &[
                ("src/main.titan", &fonte(&["a"])),
                ("src/a.titan", &fonte(&["b"])),
                ("src/b.titan", &fonte(&["a"])),
                (
                    "titan.toml",
                    &manifesto_com(&[("a", "src/a.titan"), ("b", "src/b.titan")]),
                ),
            ],
        );

        let erro = grafo_de(&dir).expect_err("o ciclo a → b → a deve ser rejeitado");

        let GrafoError::Ciclo { caminho } = &erro else {
            panic!("esperava GrafoError::Ciclo, veio {erro:?}");
        };
        assert_eq!(
            caminho,
            &vec!["a".to_string(), "b".to_string(), "a".to_string()]
        );
        assert_eq!(
            erro.to_string(),
            "os imports formam um ciclo: a → b → a; um módulo não pode depender \
             (direta ou indiretamente) de si mesmo."
        );
    }

    #[test]
    fn ciclo_de_um_modulo_consigo_mesmo_nomeia_o_caminho() {
        let dir = cenario(
            "auto-ciclo",
            &[
                ("src/main.titan", &fonte(&["a"])),
                ("src/a.titan", &fonte(&["a"])),
                ("titan.toml", &manifesto_com(&[("a", "src/a.titan")])),
            ],
        );

        let erro = grafo_de(&dir).expect_err("um módulo não importa a si mesmo");

        assert!(
            erro.to_string().contains("ciclo: a → a"),
            "mensagem inesperada: {erro}"
        );
    }

    #[test]
    fn ciclo_longo_sai_inteiro_na_mensagem() {
        let dir = cenario(
            "ciclo-longo",
            &[
                ("src/main.titan", &fonte(&["a"])),
                ("src/a.titan", &fonte(&["b"])),
                ("src/b.titan", &fonte(&["c"])),
                ("src/c.titan", &fonte(&["a"])),
                (
                    "titan.toml",
                    &manifesto_com(&[
                        ("a", "src/a.titan"),
                        ("b", "src/b.titan"),
                        ("c", "src/c.titan"),
                    ]),
                ),
            ],
        );

        let erro = grafo_de(&dir).expect_err("ciclo de três");

        assert!(
            erro.to_string().contains("ciclo: a → b → c → a"),
            "mensagem inesperada: {erro}"
        );
    }

    #[test]
    fn import_de_modulo_nao_declarado_da_erro_listando_as_duas_fontes() {
        let dir = cenario(
            "nao-declarado",
            &[
                ("src/main.titan", &fonte(&["parser"])),
                ("src/lexer.titan", &fonte(&[])),
                (
                    "titan.toml",
                    &manifesto_com(&[("lexer", "src/lexer.titan")]),
                ),
            ],
        );

        let erro = grafo_de(&dir).expect_err("'parser' não foi declarado");

        let mensagem = erro.to_string();
        assert!(
            mensagem.contains("o módulo 'parser' não existe"),
            "mensagem inesperada: {mensagem}"
        );
        assert!(
            mensagem.contains("capabilities: data, texto, io"),
            "a mensagem precisa listar as capabilities: {mensagem}"
        );
        assert!(
            mensagem.contains("declarados no manifesto: lexer"),
            "a mensagem precisa listar os módulos declarados: {mensagem}"
        );
    }

    #[test]
    fn arquivo_unico_deixa_o_import_nao_resolvido_para_o_checker() {
        // Sem manifesto, `import lexer` não é erro **do grafo**: é o checker
        // quem diz "capability 'lexer' não existe", com a mensagem que
        // sempre saiu. Falar de `[modulos]` a quem não tem `titan.toml`
        // apontaria para uma seção de um arquivo inexistente.
        let dir = cenario("solto", &[("src/main.titan", &fonte(&["lexer"]))]);

        let grafo = resolver(&dir.join("src/main.titan"), None)
            .expect("o grafo de arquivo único não resolve imports");

        assert!(grafo.arquivo_unico());
        assert!(
            grafo.principal().dependencias.is_empty(),
            "sem manifesto não há aresta possível"
        );
    }

    #[test]
    fn erro_de_sintaxe_num_modulo_nomeia_o_arquivo() {
        let dir = cenario(
            "sintaxe",
            &[
                ("src/main.titan", &fonte(&["lexer"])),
                ("src/lexer.titan", "function quebrado("),
                (
                    "titan.toml",
                    &manifesto_com(&[("lexer", "src/lexer.titan")]),
                ),
            ],
        );

        let erro = grafo_de(&dir).expect_err("o módulo não parseia");

        assert!(
            matches!(erro, GrafoError::Parse { .. }),
            "esperava erro de parse, veio {erro:?}"
        );
        assert!(
            erro.to_string().contains("lexer.titan"),
            "a mensagem precisa nomear o arquivo: {erro}"
        );
    }

    #[test]
    fn erro_lexico_num_modulo_nomeia_o_arquivo() {
        let dir = cenario(
            "lexico",
            &[
                ("src/main.titan", &fonte(&["lexer"])),
                ("src/lexer.titan", "\"sem fechar"),
                (
                    "titan.toml",
                    &manifesto_com(&[("lexer", "src/lexer.titan")]),
                ),
            ],
        );

        let erro = grafo_de(&dir).expect_err("o módulo não lexa");

        assert!(
            matches!(erro, GrafoError::Lex { .. }),
            "esperava erro léxico, veio {erro:?}"
        );
        assert!(
            erro.to_string().contains("lexer.titan"),
            "a mensagem precisa nomear o arquivo: {erro}"
        );
    }

    #[test]
    fn diamante_ordena_a_base_antes_dos_dois_lados() {
        // main → {a, b}, a → base, b → base.
        let dir = cenario(
            "diamante",
            &[
                ("src/main.titan", &fonte(&["a", "b"])),
                ("src/a.titan", &fonte(&["base"])),
                ("src/b.titan", &fonte(&["base"])),
                ("src/base.titan", &fonte(&[])),
                (
                    "titan.toml",
                    &manifesto_com(&[
                        ("a", "src/a.titan"),
                        ("b", "src/b.titan"),
                        ("base", "src/base.titan"),
                    ]),
                ),
            ],
        );

        let grafo = grafo_de(&dir).expect("diamante não é ciclo");

        let posicao = |nome: &str| {
            grafo
                .modulos
                .iter()
                .position(|m| m.nome == nome)
                .expect("módulo no grafo")
        };
        assert!(posicao("base") < posicao("a"));
        assert!(posicao("base") < posicao("b"));
        assert!(posicao("a") < posicao("prog"));
        assert!(posicao("b") < posicao("prog"));
        assert_eq!(grafo.principal().nome, "prog");
    }

    #[test]
    fn nome_do_programa_vem_do_pacote_e_nao_do_stem() {
        let dir = cenario(
            "nome-do-pacote",
            &[
                ("src/main.titan", &fonte(&[])),
                ("titan.toml", &manifesto_com(&[])),
            ],
        );

        let grafo = grafo_de(&dir).expect("grafo resolve");

        assert_eq!(grafo.nome, "prog");
        assert_eq!(grafo.principal().nome, "prog");
    }
}
