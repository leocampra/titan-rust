//! `lex → parse → check` sobre um buffer em memória, convertido para
//! `Vec<Diagnostic>` do LSP (PRD.md, T48).

use titanc::ast::Loc;
use titanc::checker::{self, CheckError};
use titanc::lexer::{self, LexError};
use titanc::parser::{self, ParseError};
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Range};

use crate::position::loc_to_position;

/// Um único ponto vira um range de um caractere: sem informação de extensão
/// no erro original, é a melhor aproximação para sublinhar algo visível.
fn point_range(source: &str, loc: &Loc) -> Range {
    let start = loc_to_position(source, loc);
    let end_loc = Loc {
        line: loc.line,
        col: loc.col + 1,
    };
    let end = loc_to_position(source, &end_loc);
    Range { start, end }
}

fn diagnostic_at(source: &str, loc: &Loc, message: String) -> Diagnostic {
    diagnostic_com_severidade(source, loc, message, DiagnosticSeverity::ERROR)
}

/// O mesmo diagnóstico, com a severidade escolhida — o aviso da T76 (o `_`
/// inalcançável) é a primeira coisa que o checker reporta sem impedir a
/// compilação, e sublinhá-lo em vermelho diria o contrário.
fn diagnostic_com_severidade(
    source: &str,
    loc: &Loc,
    message: String,
    severity: DiagnosticSeverity,
) -> Diagnostic {
    Diagnostic {
        range: point_range(source, loc),
        severity: Some(severity),
        source: Some("titanc".to_string()),
        message,
        ..Diagnostic::default()
    }
}

fn lex_diagnostic(source: &str, err: LexError) -> Diagnostic {
    diagnostic_at(source, &err.loc, err.message)
}

fn parse_diagnostic(source: &str, err: ParseError) -> Diagnostic {
    diagnostic_at(source, &err.loc, err.message)
}

/// Todos os `CheckError` do checker saem numa publicação só — ao contrário
/// de lex/parse, que param no primeiro erro (limitação aceita nesta fase,
/// PRD.md T48).
fn check_diagnostics(source: &str, errors: Vec<CheckError>) -> Vec<Diagnostic> {
    errors
        .into_iter()
        .map(|e| diagnostic_at(source, &e.loc, e.message))
        .collect()
}

pub fn compute_diagnostics(source: &str) -> Vec<Diagnostic> {
    let tokens = match lexer::lex(source) {
        Ok(tokens) => tokens,
        Err(err) => return vec![lex_diagnostic(source, err)],
    };

    let program = match parser::parse(&tokens) {
        Ok(program) => program,
        Err(err) => return vec![parse_diagnostic(source, err)],
    };

    match checker::check(&program) {
        // Um programa que tipa pode ainda ter avisos (T76) — eles saem como
        // `WARNING`, porque o código compila.
        Ok(checked) => checked
            .warnings
            .into_iter()
            .map(|w| {
                diagnostic_com_severidade(source, &w.loc, w.message, DiagnosticSeverity::WARNING)
            })
            .collect(),
        Err(errors) => check_diagnostics(source, errors),
    }
}
