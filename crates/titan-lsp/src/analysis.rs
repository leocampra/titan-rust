//! Hover e go-to-definition (PRD.md, T49): roda `lex → parse → check` sobre
//! o buffer em memória, igual a `diagnostics.rs`, mas em caso de sucesso
//! guarda o [`titanc::checker::CheckedProgram`] — em especial `uses`, o
//! índice colateral que a T49 acrescentou ao checker — para responder
//! consultas de posição sem reanalisar o texto a cada request.
//!
//! **Limitação aceita nesta fase** (mesmo espírito da T48 documentar
//! `LexError`/`ParseError` parando no primeiro erro): hover e
//! go-to-definition só respondem quando o buffer atual compila sem erro de
//! tipo. Um `.titan` com erro de tipo publica o diagnóstico (T48) mas não
//! tem índice de usos — o cursor sobre ele não devolve nada, em vez de usar
//! informação potencialmente obsoleta de uma análise anterior.

use titanc::ast::Loc;
use titanc::checker::{self, CheckedProgram};
use titanc::{lexer, parser};
use tower_lsp::lsp_types::{Hover, HoverContents, Location, MarkedString, Position, Range, Url};

use crate::position::{loc_to_position, position_to_loc};

/// Resultado de analisar um buffer — `None` quando lex/parse/check falhou;
/// a T49 não tem uso para o próprio erro aqui (diagnostics.rs já publica).
pub fn analyze(source: &str) -> Option<CheckedProgram> {
    let tokens = lexer::lex(source).ok()?;
    let program = parser::parse(&tokens).ok()?;
    checker::check(&program).ok()
}

/// Range de um identificador: de `use_loc` até `use_loc + len(name)`
/// caracteres. `SymbolUse` só guarda o ponto inicial (mesma limitação de
/// `ast::Loc` em todo o checker) — o comprimento do próprio nome fecha o
/// range.
fn name_range(source: &str, use_loc: &Loc, name: &str) -> Range {
    let start = loc_to_position(source, use_loc);
    let end_loc = Loc {
        line: use_loc.line,
        col: use_loc.col + name.chars().count(),
    };
    let end = loc_to_position(source, &end_loc);
    Range { start, end }
}

/// O cursor está sobre este uso? Mesma linha (nomes não quebram linha) e
/// coluna dentro de `[use_loc, use_loc + len(name))`.
fn covers(use_loc: &Loc, name: &str, cursor: &Loc) -> bool {
    cursor.line == use_loc.line
        && cursor.col >= use_loc.col
        && cursor.col < use_loc.col + name.chars().count()
}

fn find_use_at<'a>(
    checked: &'a CheckedProgram,
    source: &str,
    position: &Position,
) -> Option<&'a checker::SymbolUse> {
    let cursor = position_to_loc(source, position);
    checked
        .uses
        .iter()
        .find(|u| covers(&u.use_loc, &u.name, &cursor))
}

/// Tipo do símbolo sob o cursor, formatado por `checker::type_name` — a
/// mesma função que o checker já usa nas mensagens de erro (PRD.md, T49).
pub fn hover_at(checked: &CheckedProgram, source: &str, position: Position) -> Option<Hover> {
    let usage = find_use_at(checked, source, &position)?;
    Some(Hover {
        contents: HoverContents::Scalar(MarkedString::String(format!(
            "{}: {}",
            usage.name, usage.type_name
        ))),
        range: Some(name_range(source, &usage.use_loc, &usage.name)),
    })
}

/// Local da declaração do símbolo sob o cursor — `local`, parâmetro, função
/// top-level, record e campo de record (PRD.md, T49).
pub fn definition_at(
    checked: &CheckedProgram,
    source: &str,
    uri: Url,
    position: Position,
) -> Option<Location> {
    let usage = find_use_at(checked, source, &position)?;
    Some(Location {
        uri,
        range: name_range(source, &usage.def_loc, &usage.name),
    })
}
