//! Autocomplete (PRD.md, T50): completa contra as tabelas que já são fonte
//! única de verdade, sem estrutura nova além do que a T49 já preparou —
//! [`ScopeSnapshot`](titanc::checker::ScopeSnapshot) para símbolos em escopo,
//! `capabilities.rs` para membros de módulo/método de tipo opaco e
//! `builtins.rs`/`lexer.rs` para o restante.
//!
//! Três contextos, na ordem em que este módulo os resolve:
//! 1. depois de `nome.` onde `nome` é um módulo importado (`data.`, `texto.`,
//!    `io.`, à medida que essas capabilities existirem em `CAPABILITIES`) →
//!    lista `find_function`/`find_opaque` do módulo;
//! 2. depois de `nome.` onde `nome` é uma variável de tipo opaco
//!    (`df: data.DataFrame`) → lista os métodos daquele tipo;
//! 3. sem `.` antes do cursor → símbolos em escopo (T49) + `BUILTINS` +
//!    palavras-chave do léxico.
//!
//! **A armadilha desta tarefa:** o buffer no momento em que o usuário pede
//! completar geralmente **não compila** — `data.<cursor>` é um erro de parse
//! (esperava um nome depois do `.`), e mesmo `data.titan_lsp_cursor` sozinho
//! não é: uma expressão solta como statement só é aceita pelo parser se for
//! uma chamada (`parser.rs:422`, `parse_stat`) ou o lado direito de uma
//! atribuição — nunca uma referência nua. `analysis::analyze` (T49) só
//! devolve algo quando lex/parse/check tira de letra o buffer inteiro, o que
//! aqui seria quase nunca. A saída é a mesma que editores usam para qualquer
//! linguagem com essa limitação: **remendar o buffer antes de analisar**,
//! trocando a linha inteira onde o cursor está por
//! `local titan_lsp_stmt = <receptor.>titan_lsp_cursor` — sempre um
//! `StatDecl` válido, então o resto do arquivo, incluindo tudo que vem antes
//! da linha do cursor (o `import data`, a declaração de `df`, etc.),
//! permanece intacto para o checker resolver.

use titanc::ast::Loc;
use titanc::capabilities::{self, Capability};
use titanc::checker::{CheckedProgram, ScopeSnapshot};
use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind, Position};

/// Identificador usado para tapar o buraco onde o usuário está digitando —
/// precisa ser um `Name` válido do lexer e não colidir com nada real.
const PLACEHOLDER: &str = "titan_lsp_cursor";
/// Nome do `local` fictício que torna a linha remendada um `StatDecl`
/// válido — nunca lido, só existe para o parser aceitar a linha.
const STMT_NAME: &str = "titan_lsp_stmt";

/// Palavras-chave do léxico (`lexer.rs:469-493`) oferecidas em posição de
/// expressão — mesma lista literal que `lex_name_or_keyword` mapeia, porque
/// o lexer não expõe essa tabela como dado.
const KEYWORDS: &[&str] = &[
    "function", "local", "return", "end", "true", "false", "nil", "and", "or", "not", "if", "then",
    "elseif", "else", "while", "do", "for", "boolean", "integer", "float", "string", "value",
    "record", "as", "import",
];

/// Caracteres válidos num `Name` do Titan (`lexer.rs`: letras, dígitos e
/// `_`, sem começar por dígito — irrelevante aqui pois só andamos para trás
/// a partir de um ponto qualquer).
fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Contexto de autocomplete inferido a partir do texto antes do cursor —
/// resultado de andar para trás a partir da coluna do cursor, dentro da
/// própria linha (nomes não quebram linha, mesma premissa de `analysis.rs`).
enum Context {
    /// Sem `.` antes do identificador parcial: posição de expressão.
    Expression,
    /// `nome.parcial<cursor>` — `nome` é o texto antes do `.`.
    Member { receiver: String },
}

/// Remenda o buffer e classifica o contexto (PRD.md, T50 — "a armadilha da
/// tarefa" descrita no módulo): troca a linha inteira do cursor por
/// `local titan_lsp_stmt = <receptor.>titan_lsp_cursor`, preservando a
/// indentação para as colunas de `checker::ScopeSnapshot` da própria linha
/// continuarem plausíveis. Linhas antes e depois do cursor não mudam.
fn patch_and_classify(text: &str, position: Position) -> (String, Context) {
    let lines: Vec<&str> = text.split('\n').collect();
    let line_idx = (position.line as usize).min(lines.len().saturating_sub(1));
    let line = lines.get(line_idx).copied().unwrap_or("");
    let col = (position.character as usize).min(line.len());

    let before = &line[..col];
    let indent_end = before
        .find(|c: char| !c.is_whitespace())
        .unwrap_or(before.len());
    let indent = &before[..indent_end];

    let name_start = before
        .rfind(|c: char| !is_name_char(c))
        .map(|i| i + 1)
        .unwrap_or(0);
    let before_name = &before[..name_start];

    let (context, expr) = if before_name.ends_with('.') {
        let receiver_end = before_name.len() - 1;
        let receiver_start = before_name[..receiver_end]
            .rfind(|c: char| !is_name_char(c))
            .map(|i| i + 1)
            .unwrap_or(0);
        let receiver = before_name[receiver_start..receiver_end].to_string();
        // `()`: membro de módulo/método só resolve em posição de chamada
        // (`checker.rs`, `resolve_callee`) — `data.titan_lsp_cursor` sozinho,
        // sem chamar, é sempre erro ("módulo, não um valor" ou similar),
        // mesmo quando `titan_lsp_cursor` existisse de verdade.
        let expr = format!("{receiver}.{PLACEHOLDER}()");
        (Context::Member { receiver }, expr)
    } else {
        (Context::Expression, PLACEHOLDER.to_string())
    };

    let patched_line = format!("{indent}local {STMT_NAME} = {expr}");
    let mut patched_lines = lines;
    patched_lines[line_idx] = &patched_line;
    (patched_lines.join("\n"), context)
}

/// Converte `Position` do LSP (0-indexado) para `Loc` do checker
/// (1-indexado) — mesma conversão de `analysis.rs`.
fn position_to_loc(pos: Position) -> Loc {
    Loc {
        line: pos.line as usize + 1,
        col: pos.character as usize + 1,
    }
}

/// O snapshot mais interno (intervalo mais estreito) que contém `cursor` —
/// blocos aninhados produzem snapshots aninhados (`checker.rs`,
/// `ScopeSnapshot`), então o de menor extensão é o mais específico.
fn innermost_scope(scopes: &[ScopeSnapshot], cursor: Loc) -> Option<&ScopeSnapshot> {
    let contains = |s: &ScopeSnapshot| {
        (s.start.line, s.start.col) <= (cursor.line, cursor.col)
            && (cursor.line, cursor.col) <= (s.end.line, s.end.col)
    };
    scopes.iter().filter(|s| contains(s)).min_by_key(|s| {
        let lines = s.end.line.saturating_sub(s.start.line);
        (lines, s.end.col.saturating_sub(s.start.col))
    })
}

fn keyword_items() -> Vec<CompletionItem> {
    KEYWORDS
        .iter()
        .map(|kw| CompletionItem {
            label: kw.to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            ..CompletionItem::default()
        })
        .collect()
}

fn builtin_items() -> Vec<CompletionItem> {
    titanc::builtins::BUILTINS
        .iter()
        .map(|b| CompletionItem {
            label: b.titan_name.to_string(),
            kind: Some(CompletionItemKind::FUNCTION),
            detail: Some(format!("({} parâmetro(s))", b.params.len())),
            ..CompletionItem::default()
        })
        .collect()
}

/// Contexto 3: símbolos em escopo no ponto do cursor (T49's `ScopeSnapshot`)
/// + `BUILTINS` + palavras-chave.
fn expression_items(checked: &CheckedProgram, position: Position) -> Vec<CompletionItem> {
    let cursor = position_to_loc(position);
    let mut items = Vec::new();

    if let Some(scope) = innermost_scope(&checked.scopes, cursor) {
        for symbol in &scope.symbols {
            if symbol.name == PLACEHOLDER {
                continue;
            }
            items.push(CompletionItem {
                label: symbol.name.clone(),
                kind: Some(if symbol.is_module {
                    CompletionItemKind::MODULE
                } else {
                    CompletionItemKind::VARIABLE
                }),
                detail: Some(symbol.type_name.clone()),
                ..CompletionItem::default()
            });
        }
    }

    items.extend(builtin_items());
    items.extend(keyword_items());
    items
}

/// Contextos 1/2: membros de `receiver.` — módulo importado (funções +
/// tipos opacos) ou variável cujo tipo é um opaco de módulo (métodos).
/// Resolve o tipo de `receiver`, quando é variável, pelo mesmo
/// `ScopeSnapshot` do contexto 3 — o `type_name` já vem formatado como
/// `"módulo.Tipo"` por `checker::type_name`.
fn member_items(
    checked: &CheckedProgram,
    position: Position,
    receiver: &str,
) -> Vec<CompletionItem> {
    let cursor = position_to_loc(position);
    let Some(scope) = innermost_scope(&checked.scopes, cursor) else {
        return Vec::new();
    };

    let Some(symbol) = scope.symbols.iter().find(|s| s.name == receiver) else {
        return Vec::new();
    };

    if symbol.is_module {
        let Some(capability) = capabilities::lookup_module(receiver) else {
            return Vec::new();
        };
        return module_items(capability);
    }

    // Variável de tipo opaco `módulo.Tipo` (`df: data.DataFrame`): separa a
    // string já formatada por `checker::type_name` (`type_name:2626`).
    let Some((module, type_name)) = symbol.type_name.split_once('.') else {
        return Vec::new();
    };
    let Some(capability) = capabilities::lookup_module(module) else {
        return Vec::new();
    };
    method_items(capability, type_name)
}

fn module_items(capability: &'static Capability) -> Vec<CompletionItem> {
    let mut items: Vec<CompletionItem> = capability
        .functions
        .iter()
        .filter(|f| f.receiver.is_none())
        .map(|f| CompletionItem {
            label: f.titan_name.to_string(),
            kind: Some(CompletionItemKind::FUNCTION),
            detail: Some(format!("({} parâmetro(s))", f.params.len())),
            ..CompletionItem::default()
        })
        .collect();
    items.extend(capability.opaque_types.iter().map(|t| CompletionItem {
        label: t.titan_name.to_string(),
        kind: Some(CompletionItemKind::CLASS),
        ..CompletionItem::default()
    }));
    items
}

fn method_items(capability: &'static Capability, receiver_type: &str) -> Vec<CompletionItem> {
    capability
        .functions
        .iter()
        .filter(|f| f.receiver == Some(receiver_type))
        .map(|f| CompletionItem {
            label: f.titan_name.to_string(),
            kind: Some(CompletionItemKind::METHOD),
            detail: Some(format!("({} parâmetro(s))", f.params.len())),
            ..CompletionItem::default()
        })
        .collect()
}

/// Ponto de entrada: analisa o buffer remendado (ver comentário do módulo) e
/// despacha para o contexto correto.
///
/// Usa [`checker::check_partial`] em vez de `analysis::analyze` (T49) porque
/// o buffer remendado tipicamente **não** tipa sem erro — `data.titan_lsp_
/// cursor()` não existe no módulo, por exemplo — mas os `scopes`/`uses` que
/// o autocomplete consome já estão completos mesmo assim (o erro de tipo
/// acontece depois de resolver o receptor). Só lex/parse falhando (o
/// remendo produziu algo sintaticamente inválido, fora dos casos previstos
/// em `patch_and_classify`) devolve lista vazia.
pub fn complete(text: &str, position: Position) -> Vec<CompletionItem> {
    let (patched, context) = patch_and_classify(text, position);
    let Ok(tokens) = titanc::lexer::lex(&patched) else {
        return Vec::new();
    };
    let Ok(program) = titanc::parser::parse(&tokens) else {
        return Vec::new();
    };
    let checked = titanc::checker::check_partial(&program);

    match context {
        Context::Expression => expression_items(&checked, position),
        Context::Member { receiver } => member_items(&checked, position, &receiver),
    }
}
