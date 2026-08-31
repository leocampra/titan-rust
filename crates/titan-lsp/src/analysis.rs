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

/// Resultado de analisar um buffer — `None` quando lex/parse/check falhou;
/// a T49 não tem uso para o próprio erro aqui (diagnostics.rs já publica).
pub fn analyze(source: &str) -> Option<CheckedProgram> {
    let tokens = lexer::lex(source).ok()?;
    let program = parser::parse(&tokens).ok()?;
    checker::check(&program).ok()
}

/// Converte `Loc` (1-indexado, coluna em bytes) para `Position` do LSP
/// (0-indexado) — mesma conversão de `diagnostics.rs`, válida porque o
/// servidor anuncia `positionEncoding: "utf-8"` em `initialize`.
fn loc_to_position(loc: &Loc) -> Position {
    Position {
        line: loc.line.saturating_sub(1) as u32,
        character: loc.col.saturating_sub(1) as u32,
    }
}

fn position_to_loc(pos: &Position) -> Loc {
    Loc {
        line: pos.line as usize + 1,
        col: pos.character as usize + 1,
    }
}

/// Range de um identificador: de `use_loc` até `use_loc + len(name)` bytes.
/// `SymbolUse` só guarda o ponto inicial (mesma limitação de `ast::Loc` em
/// todo o checker) — o comprimento do próprio nome fecha o range.
fn name_range(use_loc: &Loc, name: &str) -> Range {
    let start = loc_to_position(use_loc);
    let end = Position {
        line: start.line,
        character: start.character + name.chars().count() as u32,
    };
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
    position: &Position,
) -> Option<&'a checker::SymbolUse> {
    let cursor = position_to_loc(position);
    checked
        .uses
        .iter()
        .find(|u| covers(&u.use_loc, &u.name, &cursor))
}

/// Tipo do símbolo sob o cursor, formatado por `checker::type_name` — a
/// mesma função que o checker já usa nas mensagens de erro (PRD.md, T49).
pub fn hover_at(checked: &CheckedProgram, position: Position) -> Option<Hover> {
    let usage = find_use_at(checked, &position)?;
    Some(Hover {
        contents: HoverContents::Scalar(MarkedString::String(format!(
            "{}: {}",
            usage.name, usage.type_name
        ))),
        range: Some(name_range(&usage.use_loc, &usage.name)),
    })
}

/// Local da declaração do símbolo sob o cursor — `local`, parâmetro, função
/// top-level, record e campo de record (PRD.md, T49).
pub fn definition_at(checked: &CheckedProgram, uri: Url, position: Position) -> Option<Location> {
    let usage = find_use_at(checked, &position)?;
    Some(Location {
        uri,
        range: name_range(&usage.def_loc, &usage.name),
    })
}
