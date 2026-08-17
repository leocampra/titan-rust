//! Backend Rust: traduz a AST tipada (`checker::TypedProgram`) em código Rust
//! legível.
//!
//! Espelha a estrutura do `titan/titan-compiler/coder.lua` (uma função por
//! variante de `Stat`/`Exp`, `codestat`/`codeexp`), mas emitindo Rust em vez
//! de C acoplado à API interna do Lua (PRD.md, resumo executivo).
//!
//! Mapeamento de tipos da Fase 0 (PRD.md, T6) — mantido isolado em
//! [`rust_type_name`] e [`rust_param_type_name`] para que a Fase 2 (arrays,
//! maps, records) possa trocar o modelo de memória sem espalhar a mudança:
//!
//! | Titan | Rust |
//! |---|---|
//! | `integer` | `i64` |
//! | `float` | `f64` |
//! | `boolean` | `bool` |
//! | `string` (literal) | `&'static str` |
//! | `string` (computada) | `String` |
//! | `nil` (retorno) | `()` |
//! | `{string}` (só param de `main`) | `&[String]` |
//!
//! Nada aqui assume que valores são `Copy` — ver aviso no PRD.md sobre a
//! Fase 2.

use crate::checker::{TypedExp, TypedExpKind, TypedProgram, TypedStat, TypedTopLevel};
use crate::types::Type;

const INDENT: &str = "    ";

/// Gera o `main.rs` completo (funções do programa + shim de entrada) a partir
/// da AST tipada.
pub fn generate(program: &TypedProgram) -> String {
    let mut out = String::new();

    for top in program {
        emit_toplevel(&mut out, top);
        out.push('\n');
    }

    out.push_str(ENTRY_SHIM);
    out
}

/// Shim de entrada (PRD.md, T6): o `fn main` real do binário gerado — separado
/// do `main` do Titan, que vira `titan_main` via mangling — coleta os
/// argumentos da linha de comando e usa o código de saída retornado.
const ENTRY_SHIM: &str = "\
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(titan_main(&args) as i32);
}
";

/// Prefixo de mangling para nomes de função Titan, evitando colisão com o
/// `fn main` do shim e com palavras-chave do Rust (PRD.md, T6).
fn mangle_fn_name(name: &str) -> String {
    format!("titan_{name}")
}

fn emit_toplevel(out: &mut String, top: &TypedTopLevel) {
    let TypedTopLevel::Func {
        islocal,
        name,
        params,
        rettypes,
        body,
        ..
    } = top;

    if *islocal {
        // `local function` não é visível fora do arquivo gerado.
    } else {
        out.push_str("pub ");
    }

    out.push_str("fn ");
    out.push_str(&mangle_fn_name(name));
    out.push('(');
    let param_list: Vec<String> = params
        .iter()
        .map(|(name, ty)| format!("{name}: {}", rust_param_type_name(ty)))
        .collect();
    out.push_str(&param_list.join(", "));
    out.push(')');

    let ret = rettypes.first().unwrap_or(&Type::Nil);
    if !matches!(ret, Type::Nil) {
        out.push_str(" -> ");
        out.push_str(&rust_type_name(ret));
    }

    out.push_str(" {\n");
    emit_block_stats(out, body, 1);
    out.push_str("}\n");
}

/// Emite os comandos de um `TypedStat::Block` (o único formato de corpo de
/// função na Fase 0) já indentados.
fn emit_block_stats(out: &mut String, stat: &TypedStat, depth: usize) {
    let TypedStat::Block { stats, .. } = stat else {
        emit_stat(out, stat, depth);
        return;
    };
    for s in stats {
        emit_stat(out, s, depth);
    }
}

fn indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str(INDENT);
    }
}

fn emit_stat(out: &mut String, stat: &TypedStat, depth: usize) {
    match stat {
        TypedStat::Block { .. } => {
            indent(out, depth);
            out.push_str("{\n");
            emit_block_stats(out, stat, depth + 1);
            indent(out, depth);
            out.push_str("}\n");
        }
        TypedStat::Decl {
            name, ty, value, ..
        } => {
            indent(out, depth);
            out.push_str("let ");
            out.push_str(name);
            out.push_str(": ");
            out.push_str(&rust_type_name(ty));
            out.push_str(" = ");
            out.push_str(&emit_exp(value));
            out.push_str(";\n");
        }
        TypedStat::Call { call, .. } => {
            indent(out, depth);
            out.push_str(&emit_exp(call));
            out.push_str(";\n");
        }
        TypedStat::Return { exps, .. } => {
            indent(out, depth);
            out.push_str("return");
            if let Some(value) = exps.first() {
                out.push(' ');
                out.push_str(&emit_exp(value));
            }
            out.push_str(";\n");
        }
    }
}

/// Gera a expressão Rust equivalente a `exp`, já convertida para o tipo Rust
/// esperado no ponto de uso (ver [`coerce_to_borrowed_str`] para a única
/// coerção necessária na Fase 0: `String` → `&str` em argumentos de chamada).
fn emit_exp(exp: &TypedExp) -> String {
    match &exp.kind {
        TypedExpKind::Nil => "()".to_string(),
        TypedExpKind::Bool(v) => v.to_string(),
        TypedExpKind::Integer(v) => v.to_string(),
        TypedExpKind::Float(v) => format_float_literal(*v),
        TypedExpKind::String(v) => format_string_literal(v),
        TypedExpKind::Var(name) => name.clone(),
        TypedExpKind::Concat(exps) => emit_concat(exps),
        TypedExpKind::Call { callee, args } => emit_call(callee, args),
    }
}

/// Literais float sempre carregam `.0` (ou expoente) para nascer como `f64`
/// mesmo quando o valor é matematicamente inteiro (`1.0`, não `1`).
fn format_float_literal(v: f64) -> String {
    if v.is_finite() && v.fract() == 0.0 {
        format!("{v:.1}")
    } else {
        format!("{v}")
    }
}

fn format_string_literal(v: &str) -> String {
    format!("{v:?}")
}

/// `..` do Titan é N-ário; `titan_runtime::concat` é binário — encadeia par a
/// par, associando à esquerda.
fn emit_concat(exps: &[TypedExp]) -> String {
    let mut parts = exps.iter();
    let first = parts
        .next()
        .expect("checker garante ExpConcat com ao menos um operando");
    let mut acc = coerce_to_borrowed_str(first);
    for e in parts {
        acc = format!(
            "titan_runtime::concat({acc}, {})",
            coerce_to_borrowed_str(e)
        );
    }
    acc
}

fn emit_call(callee: &str, args: &[TypedExp]) -> String {
    let rendered_args: Vec<String> = args.iter().map(coerce_to_borrowed_str).collect();
    if callee == "print" {
        format!("titan_runtime::print({})", rendered_args.join(", "))
    } else {
        format!("{}({})", mangle_fn_name(callee), rendered_args.join(", "))
    }
}

/// Coage uma expressão para `&str` quando seu tipo Rust nasce como `String`
/// (string computada) — todo parâmetro de função da Fase 0 que recebe string
/// espera `&str` (ver `titan_runtime::print` e `titan_runtime::concat`).
/// Literais (`&'static str`) e variáveis já `&str`-compatíveis passam direto.
fn coerce_to_borrowed_str(exp: &TypedExp) -> String {
    let rendered = emit_exp(exp);
    if exp.ty == Type::String && is_owned_string_expr(exp) {
        format!("&{rendered}")
    } else {
        rendered
    }
}

/// Uma expressão de tipo `string` nasce como `String` (em vez de
/// `&'static str`) somente quando é uma concatenação, uma chamada de função
/// que devolve string, ou uma variável cujo valor seria uma dessas — na
/// Fase 0 só existe `local` para introduzir variáveis, e o tipo Rust de uma
/// variável espelha o da expressão original; como o checker não anota o
/// nascimento da variável aqui, tratamos qualquer `Var` de tipo `string` como
/// potencialmente `String` e coagimos sempre, o que é seguro tanto para
/// `String` quanto para `&str`/`&'static str` (`&String` faz deref-coercion
/// para `&str` automaticamente).
fn is_owned_string_expr(exp: &TypedExp) -> bool {
    matches!(
        exp.kind,
        TypedExpKind::Concat(_) | TypedExpKind::Call { .. } | TypedExpKind::Var(_)
    )
}

/// Tipo Rust de uma variável/expressão `string`, conforme o mapeamento da
/// Fase 0 (PRD.md, T6): computada → `String`, o resto (primitivas e o `{string}`
/// de `main`) segue [`rust_type_name`].
fn rust_type_name(ty: &Type) -> String {
    match ty {
        Type::Nil => "()".to_string(),
        Type::Boolean => "bool".to_string(),
        Type::Integer => "i64".to_string(),
        Type::Float => "f64".to_string(),
        Type::String => "String".to_string(),
        Type::Array { elem } if **elem == Type::String => "&[String]".to_string(),
        other => unreachable!(
            "tipo '{other:?}' fora do subconjunto de codegen da Fase 0 — checker deveria ter rejeitado antes"
        ),
    }
}

/// Tipo Rust de um **parâmetro** de função: idêntico a [`rust_type_name`],
/// exceto que `string` em posição de parâmetro usa `&str` (parâmetros não são
/// donos do valor na Fase 0 — só `main` recebe `{string}`, que já é `&[String]`).
fn rust_param_type_name(ty: &Type) -> String {
    match ty {
        Type::String => "&str".to_string(),
        other => rust_type_name(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checker::check;
    use crate::lexer::lex;
    use crate::parser::parse;

    fn generate_source(source: &str) -> String {
        let tokens = lex(source).unwrap_or_else(|e| panic!("erro léxico inesperado: {e}"));
        let program = parse(&tokens).unwrap_or_else(|e| panic!("erro sintático inesperado: {e}"));
        let typed = check(&program).unwrap_or_else(|errs| {
            panic!(
                "erro de tipo inesperado: {}",
                errs.iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        });
        generate(&typed)
    }

    #[test]
    fn gera_hello_world() {
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/hello.titan"
        ))
        .expect("examples/hello.titan deve existir");

        let rust = generate_source(&source);

        assert!(rust.contains("pub fn titan_main(args: &[String]) -> i64 {"));
        assert!(rust.contains("titan_runtime::print(\"Olá, mundo!\");"));
        assert!(rust.contains("return 0;"));
        assert!(rust.contains("fn main() {"));
        assert!(rust.contains("std::process::exit(titan_main(&args) as i32);"));
    }

    #[test]
    fn gerado_compila_e_roda_com_rustc_de_verdade() {
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/hello.titan"
        ))
        .expect("examples/hello.titan deve existir");
        let rust = generate_source(&source);

        let dir = std::env::temp_dir().join(format!(
            "titanc-codegen-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("cria diretório temporário");
        let src_path = dir.join("main.rs");
        std::fs::write(&src_path, &rust).expect("escreve main.rs gerado");

        let runtime_src =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../titan-runtime/src/lib.rs");
        let runtime_out = dir.join("libtitan_runtime.rlib");
        let status = std::process::Command::new("rustc")
            .args(["--crate-type", "lib", "--edition", "2024", "-o"])
            .arg(&runtime_out)
            .arg(&runtime_src)
            .status()
            .expect("invoca rustc para o runtime");
        assert!(status.success(), "falha ao compilar titan-runtime");

        let bin_path = dir.join("hello");
        let status = std::process::Command::new("rustc")
            .args(["--edition", "2024", "--extern"])
            .arg(format!("titan_runtime={}", runtime_out.display()))
            .arg("-o")
            .arg(&bin_path)
            .arg(&src_path)
            .status()
            .expect("invoca rustc no arquivo gerado");
        assert!(status.success(), "rustc falhou ao compilar o Rust gerado");

        let output = std::process::Command::new(&bin_path)
            .output()
            .expect("executa o binário gerado");
        assert_eq!(String::from_utf8_lossy(&output.stdout), "Olá, mundo!\n");
        assert_eq!(output.status.code(), Some(0));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn concat_encadeia_par_a_par_e_coage_string_computada() {
        let source = r#"function main(args: {string}): integer
    local a: string = "x" .. "y" .. "z"
    print(a)
    return 0
end"#;
        let rust = generate_source(source);
        assert!(rust.contains(
            "let a: String = titan_runtime::concat(titan_runtime::concat(\"x\", \"y\"), \"z\");"
        ));
        assert!(rust.contains("titan_runtime::print(&a);"));
    }

    #[test]
    fn chamada_de_funcao_local_usa_mangling_e_sem_pub() {
        let source = r#"local function ajuda(): integer
    return 1
end

function main(args: {string}): integer
    return ajuda()
end"#;
        let rust = generate_source(source);
        assert!(rust.contains("fn titan_ajuda() -> i64 {"));
        assert!(!rust.contains("pub fn titan_ajuda"));
        assert!(rust.contains("return titan_ajuda();"));
    }
}
