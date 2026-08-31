//! `lex → parse → check` sobre um buffer em memória, convertido para
//! `Vec<Diagnostic>` do LSP (PRD.md, T48).

use titanc::ast::Loc;
use titanc::checker::{self, CheckError};
use titanc::lexer::{self, LexError};
use titanc::parser::{self, ParseError};
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range};

/// Converte um `Loc` (1-indexado, coluna em bytes) para `Position` do LSP
/// (0-indexado). Válido em UTF-8 puro porque o servidor anuncia
/// `positionEncoding: "utf-8"` em `initialize` — ver comentário em `main.rs`.
fn loc_to_position(loc: &Loc) -> Position {
    Position {
        line: loc.line.saturating_sub(1) as u32,
        character: loc.col.saturating_sub(1) as u32,
    }
}

/// Um único ponto vira um range de um caractere: sem informação de extensão
/// no erro original, é a melhor aproximação para sublinhar algo visível.
fn point_range(loc: &Loc) -> Range {
    let start = loc_to_position(loc);
    let end = Position {
        line: start.line,
        character: start.character + 1,
    };
    Range { start, end }
}

fn diagnostic_at(loc: &Loc, message: String) -> Diagnostic {
    Diagnostic {
        range: point_range(loc),
        severity: Some(DiagnosticSeverity::ERROR),
        source: Some("titanc".to_string()),
        message,
        ..Diagnostic::default()
    }
}

fn lex_diagnostic(err: LexError) -> Diagnostic {
    diagnostic_at(&err.loc, err.message)
}

fn parse_diagnostic(err: ParseError) -> Diagnostic {
    diagnostic_at(&err.loc, err.message)
}

/// Todos os `CheckError` do checker saem numa publicação só — ao contrário
/// de lex/parse, que param no primeiro erro (limitação aceita nesta fase,
/// PRD.md T48).
fn check_diagnostics(errors: Vec<CheckError>) -> Vec<Diagnostic> {
    errors
        .into_iter()
        .map(|e| diagnostic_at(&e.loc, e.message))
        .collect()
}

pub fn compute_diagnostics(source: &str) -> Vec<Diagnostic> {
    let tokens = match lexer::lex(source) {
        Ok(tokens) => tokens,
        Err(err) => return vec![lex_diagnostic(err)],
    };

    let program = match parser::parse(&tokens) {
        Ok(program) => program,
        Err(err) => return vec![parse_diagnostic(err)],
    };

    match checker::check(&program) {
        Ok(_typed_program) => Vec::new(),
        Err(errors) => check_diagnostics(errors),
    }
}
