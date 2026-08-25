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

use crate::checker::{BinOp, TypedExp, TypedExpKind, TypedProgram, TypedStat, TypedTopLevel, UnOp};
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
            name,
            ty,
            value,
            mutable,
            ..
        } => {
            indent(out, depth);
            out.push_str(if *mutable { "let mut " } else { "let " });
            out.push_str(name);
            out.push_str(": ");
            out.push_str(&rust_type_name(ty));
            out.push_str(" = ");
            out.push_str(&emit_slot_value(ty, value));
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
                out.push_str(&emit_slot_value(&value.ty, value));
            }
            out.push_str(";\n");
        }
        TypedStat::If {
            thens, elsestat, ..
        } => {
            indent(out, depth);
            // `thens` nunca é vazio: o parser exige `if exp then`.
            let mut keyword = "if ";
            for then in thens {
                out.push_str(keyword);
                out.push_str(&emit_delimited_exp(&then.condition));
                out.push_str(" {\n");
                emit_block_stats(out, &then.block, depth + 1);
                indent(out, depth);
                out.push('}');
                keyword = " else if ";
            }
            if let Some(els) = elsestat {
                out.push_str(" else {\n");
                emit_block_stats(out, els, depth + 1);
                indent(out, depth);
                out.push('}');
            }
            out.push('\n');
        }
        TypedStat::While {
            condition, block, ..
        } => {
            indent(out, depth);
            out.push_str("while ");
            out.push_str(&emit_delimited_exp(condition));
            out.push_str(" {\n");
            emit_block_stats(out, block, depth + 1);
            indent(out, depth);
            out.push_str("}\n");
        }
        TypedStat::Assign { name, value, .. } => {
            indent(out, depth);
            out.push_str(name);
            out.push_str(" = ");
            // O tipo do valor serve de tipo do slot: o checker garantiu que
            // ele é `compatible` com o da variável, e `compatible` não coage
            // entre primitivas distintas nesta fase.
            out.push_str(&emit_slot_value(&value.ty, value));
            out.push_str(";\n");
        }
        // O checker (T12) já aceita `for`, mas seu desaçucaramento para
        // `while` chega na T15. Até lá o Rust gerado carrega um
        // `compile_error!` explicativo — a build do programa falha com
        // mensagem clara em vez de emitir código silenciosamente errado
        // (e o titanc segue sem panic).
        TypedStat::For { .. } => {
            indent(out, depth);
            out.push_str(
                "compile_error!(\"`for` ainda não tem geração de código (chega na tarefa T15)\");\n",
            );
        }
    }
}

/// Gera a expressão Rust equivalente a `exp` em **posição de operando**:
/// binop/unop saem entre parênteses (`(lhs op rhs)`, `(-e)`, `(!e)`) para a
/// precedência do Titan ficar explícita em qualquer aninhamento, sem depender
/// de coincidir com a do Rust (PRD T14). Em posição que a sintaxe já delimita
/// (condição, valor de `let`/atribuição/`return`, argumento), use
/// [`emit_delimited_exp`].
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
        // `^` vira chamada de método (`.powf`), que já se delimita sozinha —
        // não precisa dos parênteses externos em nenhuma posição.
        TypedExpKind::Binop {
            op: BinOp::Pow,
            lhs,
            rhs,
        } => emit_pow(lhs, rhs),
        TypedExpKind::Binop { op, lhs, rhs } => format!("({})", emit_binop(*op, lhs, rhs, &exp.ty)),
        TypedExpKind::Unop { op, exp: operand } => format!("({})", emit_unop(*op, operand)),
    }
}

/// Expressão em posição que a sintaxe do Rust já delimita — condição de
/// `if`/`while`, valor de `let`/atribuição/`return`, argumento de chamada.
/// Binop/unop saem **sem** os parênteses externos: o lint `unused_parens` do
/// rustc reclama deles exatamente nessas posições, e o Rust gerado deve
/// compilar sem warnings. Os operandos aninhados seguem parentesizados via
/// [`emit_exp`], então a precedência continua explícita.
fn emit_delimited_exp(exp: &TypedExp) -> String {
    match &exp.kind {
        TypedExpKind::Binop { op: BinOp::Pow, .. } => emit_exp(exp),
        TypedExpKind::Binop { op, lhs, rhs } => emit_binop(*op, lhs, rhs, &exp.ty),
        TypedExpKind::Unop { op, exp: operand } => emit_unop(*op, operand),
        _ => emit_exp(exp),
    }
}

/// Valor emitido para um "slot" cujo tipo Rust vem de [`rust_type_name`] —
/// inicializador de `let`, lado direito de atribuição, valor de `return`.
/// Slot de tipo `string` é `String` dona: literal (`&'static str`) e variável
/// (`String` local ou `&str` parâmetro) ganham `.to_string()`, que cobre os
/// dois casos e, na variável, copia em vez de mover — a original continua
/// utilizável depois de `local a: string = b`. Concat e chamada já produzem
/// `String` e passam direto.
fn emit_slot_value(slot_ty: &Type, value: &TypedExp) -> String {
    if *slot_ty == Type::String
        && matches!(value.kind, TypedExpKind::String(_) | TypedExpKind::Var(_))
    {
        format!("{}.to_string()", emit_exp(value))
    } else {
        emit_delimited_exp(value)
    }
}

/// Operador Rust equivalente a um [`BinOp`] do Titan. `Pow` não tem operador
/// (o `^` do Rust é XOR) e é emitido como chamada em [`emit_pow`].
fn binop_symbol(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Mod => "%",
        BinOp::Eq => "==",
        // Atenção: `~=` no Titan, `!=` no Rust.
        BinOp::Ne => "!=",
        BinOp::Lt => "<",
        BinOp::Gt => ">",
        BinOp::Le => "<=",
        BinOp::Ge => ">=",
        BinOp::And => "&&",
        BinOp::Or => "||",
        BinOp::Pow => unreachable!("`^` é emitido como chamada a powf em emit_pow"),
    }
}

/// Corpo de um operador binário, sem os parênteses externos — quem chama
/// decide se eles são necessários ([`emit_exp`]) ou proibidos pelo lint
/// ([`emit_delimited_exp`]).
fn emit_binop(op: BinOp, lhs: &TypedExp, rhs: &TypedExp, result_ty: &Type) -> String {
    let symbol = binop_symbol(op);
    match op {
        // Aritméticos: o tipo do resultado já veio decidido do checker
        // (`numeric_result`); operando Integer em resultado Float ganha o
        // cast aqui — o checker não emite nó de cast (T13). Obs.: `%` mapeia
        // para o resto truncado do Rust, que difere do módulo com piso do
        // Lua quando há operando negativo — o PRD (T14) fixa o mapeamento
        // direto nesta fase.
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => format!(
            "{} {symbol} {}",
            emit_numeric_operand(lhs, result_ty),
            emit_numeric_operand(rhs, result_ty)
        ),
        BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => {
            emit_comparison(op, symbol, lhs, rhs)
        }
        // Boolean estrito dos dois lados (decisão 7 da Fase 1) — mapeamento
        // direto para os operadores de curto-circuito do Rust.
        BinOp::And | BinOp::Or => format!("{} {symbol} {}", emit_exp(lhs), emit_exp(rhs)),
        BinOp::Pow => unreachable!("`^` é emitido como chamada a powf em emit_pow"),
    }
}

/// Comparações — o checker (T13) já validou as combinações: número com
/// número (com coerção int→float quando os lados divergem), string com
/// string, e boolean com boolean (só `==`/`~=`).
fn emit_comparison(op: BinOp, symbol: &str, lhs: &TypedExp, rhs: &TypedExp) -> String {
    if matches!(lhs.ty, Type::Integer | Type::Float) {
        // Mesma regra de `numeric_result`: qualquer Float promove os dois
        // lados para f64.
        let target = if lhs.ty == Type::Float || rhs.ty == Type::Float {
            Type::Float
        } else {
            Type::Integer
        };
        return format!(
            "{} {symbol} {}",
            emit_numeric_operand(lhs, &target),
            emit_numeric_operand(rhs, &target)
        );
    }
    if lhs.ty == Type::String && matches!(op, BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge) {
        return format!("{} {symbol} {}", str_ord_operand(lhs), str_ord_operand(rhs));
    }
    // Igualdade de string/boolean: os impls cruzados de `PartialEq` do std
    // (`String == &str` e vice-versa) cobrem todas as combinações.
    format!("{} {symbol} {}", emit_exp(lhs), emit_exp(rhs))
}

/// Operando string de `<`/`>`/`<=`/`>=`: o std não implementa `PartialOrd`
/// cruzado entre `String` e `&str`, então os dois lados são nivelados para
/// `&str` com `&…[..]` — que funciona igualmente sobre `String` (local) e
/// `&str` (parâmetro/literal). Literal já é `&str` e passa direto.
fn str_ord_operand(exp: &TypedExp) -> String {
    match &exp.kind {
        TypedExpKind::String(_) => emit_exp(exp),
        _ => format!("&{}[..]", emit_exp(exp)),
    }
}

/// Operando numérico já validado pelo checker: `Integer` em posição cujo
/// resultado é `Float` ganha `(x as f64)` (PRD T14).
fn emit_numeric_operand(exp: &TypedExp, result_ty: &Type) -> String {
    let rendered = emit_exp(exp);
    if exp.ty == Type::Integer && *result_ty == Type::Float {
        format!("({rendered} as f64)")
    } else {
        rendered
    }
}

/// `^` → `(lhs as f64).powf(rhs as f64)` — Rust não tem operador de potência
/// (`^` é XOR). O cast sai **sempre**, mesmo com operando já float: é um
/// cast trivial (lint allow por padrão) e resolve o literal float como
/// receptor de método — `2.0.powf(…)` não compila (tipo numérico ambíguo).
fn emit_pow(lhs: &TypedExp, rhs: &TypedExp) -> String {
    format!("({} as f64).powf({} as f64)", emit_exp(lhs), emit_exp(rhs))
}

/// Corpo de um operador unário, sem os parênteses externos — mesma divisão
/// de responsabilidade de [`emit_binop`].
fn emit_unop(op: UnOp, operand: &TypedExp) -> String {
    match op {
        UnOp::Neg => format!("-{}", emit_exp(operand)),
        UnOp::Not => format!("!{}", emit_exp(operand)),
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
    let mut acc = concat_operand(first);
    for e in parts {
        acc = format!("titan_runtime::concat({acc}, {})", concat_operand(e));
    }
    acc
}

/// Operando de `..`: número (`Integer`/`Float`, decisão 4 da Fase 1) vira
/// `&x.to_string()` para casar com o `titan_runtime::concat(&str, &str)`
/// existente; string segue a coerção padrão de argumento.
fn concat_operand(exp: &TypedExp) -> String {
    if matches!(exp.ty, Type::Integer | Type::Float) {
        return format!("&{}.to_string()", emit_exp(exp));
    }
    coerce_to_borrowed_str(exp)
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
/// Argumento de chamada é posição delimitada — binop/unop numéricos/boolean
/// saem sem parênteses externos (strings nunca são binop/unop: `..` é nó
/// próprio).
fn coerce_to_borrowed_str(exp: &TypedExp) -> String {
    let rendered = emit_delimited_exp(exp);
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

    /// Compila `rust` com o rustc real (linkando o titan-runtime) e executa
    /// o binário; devolve (stderr da compilação, saída da execução).
    fn compila_e_executa(rust: &str, nome: &str) -> (String, std::process::Output) {
        let dir = std::env::temp_dir().join(format!(
            "titanc-codegen-test-{nome}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("cria diretório temporário");
        let src_path = dir.join("main.rs");
        std::fs::write(&src_path, rust).expect("escreve main.rs gerado");

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

        let bin_path = dir.join(nome);
        let compile = std::process::Command::new("rustc")
            .args(["--edition", "2024", "--extern"])
            .arg(format!("titan_runtime={}", runtime_out.display()))
            .arg("-o")
            .arg(&bin_path)
            .arg(&src_path)
            .output()
            .expect("invoca rustc no arquivo gerado");
        assert!(
            compile.status.success(),
            "rustc falhou ao compilar o Rust gerado:\n{}\n--- fonte gerado ---\n{rust}",
            String::from_utf8_lossy(&compile.stderr)
        );

        let output = std::process::Command::new(&bin_path)
            .output()
            .expect("executa o binário gerado");
        let _ = std::fs::remove_dir_all(&dir);
        (
            String::from_utf8_lossy(&compile.stderr).into_owned(),
            output,
        )
    }

    #[test]
    fn gerado_compila_e_roda_com_rustc_de_verdade() {
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/hello.titan"
        ))
        .expect("examples/hello.titan deve existir");
        let rust = generate_source(&source);

        let (_avisos, output) = compila_e_executa(&rust, "hello");
        assert_eq!(String::from_utf8_lossy(&output.stdout), "Olá, mundo!\n");
        assert_eq!(output.status.code(), Some(0));
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

    // ---- T14: If/While/Assign/Binop/Unop --------------------------------

    #[test]
    fn if_elseif_else_emite_cascata_rust() {
        let source = r#"function classifica(n: integer): integer
    if n < 0 then
        return -1
    elseif n == 0 then
        return 0
    else
        return 1
    end
end

function main(args: {string}): integer
    return classifica(0)
end"#;
        let rust = generate_source(source);
        assert!(rust.contains("    if n < 0 {\n        return -1;\n    } else if n == 0 {"));
        assert!(rust.contains("    } else {\n        return 1;\n    }\n"));
    }

    #[test]
    fn while_assign_e_let_mut_apenas_nas_reatribuidas() {
        let source = r#"function main(args: {string}): integer
    local acc: integer = 1
    local i: integer = 1
    local limite: integer = 5
    while i <= limite do
        acc = acc * i
        i = i + 1
    end
    print("acc: " .. acc)
    return 0
end"#;
        let rust = generate_source(source);
        assert!(rust.contains("let mut acc: i64 = 1;"));
        assert!(rust.contains("let mut i: i64 = 1;"));
        assert!(rust.contains("let limite: i64 = 5;"));
        assert!(rust.contains("    while i <= limite {\n"));
        assert!(rust.contains("acc = acc * i;"));
        assert!(rust.contains("i = i + 1;"));
    }

    #[test]
    fn precedencia_explicita_nos_parenteses_de_operando() {
        let source = r#"function main(args: {string}): integer
    local a: integer = 1 + 2 * 3
    local b: boolean = a == 7 and a ~= 0
    local c: integer = - -a
    local d: boolean = not not true
    if b and d then
        return c
    end
    return 0
end"#;
        let rust = generate_source(source);
        // `*` associa antes de `+`; só o operando aninhado ganha parênteses.
        assert!(rust.contains("let a: i64 = 1 + (2 * 3);"));
        // `~=` do Titan vira `!=`; operandos de `&&` saem parentesizados.
        assert!(rust.contains("let b: bool = (a == 7) && (a != 0);"));
        assert!(rust.contains("let c: i64 = -(-a);"));
        assert!(rust.contains("let d: bool = !(!true);"));
        assert!(rust.contains("if b && d {"));
    }

    #[test]
    fn div_e_pow_promovem_para_float() {
        let source = r#"function main(args: {string}): integer
    local d: float = 10 / 3
    local p: float = 2 ^ 10
    local m: float = 1 + 0.5
    print("d: " .. d)
    print("p: " .. p)
    print("m: " .. m)
    return 0
end"#;
        let rust = generate_source(source);
        assert!(rust.contains("let d: f64 = (10 as f64) / (3 as f64);"));
        assert!(rust.contains("let p: f64 = (2 as f64).powf(10 as f64);"));
        assert!(rust.contains("let m: f64 = (1 as f64) + 0.5;"));
    }

    #[test]
    fn concat_com_numero_usa_to_string() {
        let source = r#"function main(args: {string}): integer
    print("i: " .. 42)
    print("f: " .. 1.5)
    print("exp: " .. 1 + 2)
    return 0
end"#;
        let rust = generate_source(source);
        assert!(rust.contains("titan_runtime::concat(\"i: \", &42.to_string())"));
        assert!(rust.contains("titan_runtime::concat(\"f: \", &1.5.to_string())"));
        assert!(rust.contains("titan_runtime::concat(\"exp: \", &(1 + 2).to_string())"));
    }

    #[test]
    fn slot_string_ganha_to_string_para_literal_e_variavel() {
        let source = r#"function main(args: {string}): integer
    local a: string = "oi"
    local b: string = a
    b = "tchau"
    print(a)
    print(b)
    return 0
end"#;
        let rust = generate_source(source);
        assert!(rust.contains("let a: String = \"oi\".to_string();"));
        assert!(rust.contains("let mut b: String = a.to_string();"));
        assert!(rust.contains("b = \"tchau\".to_string();"));
    }

    #[test]
    fn comparacao_de_strings_nivela_ordem_para_str() {
        let source = r#"function main(args: {string}): integer
    local a: string = "abc"
    local menor: boolean = a < "abd"
    local igual: boolean = a == "abc"
    if menor and igual then
        return 0
    end
    return 1
end"#;
        let rust = generate_source(source);
        // Ordem precisa nivelar `String`/`&str` (sem PartialOrd cruzado no
        // std); igualdade usa os impls cruzados de PartialEq direto.
        assert!(rust.contains("let menor: bool = &a[..] < \"abd\";"));
        assert!(rust.contains("let igual: bool = a == \"abc\";"));
    }

    #[test]
    fn fase1_compila_roda_e_sem_warnings_de_mut_ou_parenteses() {
        let source = r#"function fatorial(n: integer): integer
    if n <= 1 then
        return 1
    end
    local acc: integer = 1
    local i: integer = 2
    while i <= n do
        acc = acc * i
        i = i + 1
    end
    return acc
end

function main(args: {string}): integer
    print("fatorial: " .. fatorial(5))
    print("div: " .. 10 / 3)
    print("pow: " .. 2 ^ 10)
    local x: integer = 7
    if x ~= 7 or false then
        print("nunca")
    elseif x > 4 and x % 2 == 1 then
        print("impar maior que 4")
    else
        print("outro")
    end
    return 0
end"#;
        let rust = generate_source(source);
        let (avisos, output) = compila_e_executa(&rust, "nucleo-t14");

        // Critério da fase: Rust gerado sem `let mut` sobrando nem
        // parênteses redundantes (o aviso pré-existente de `args` não usado
        // não é objeto da T14).
        assert!(
            !avisos.contains("unused_parens"),
            "parênteses redundantes no Rust gerado:\n{avisos}\n{rust}"
        );
        assert!(
            !avisos.contains("unused_mut"),
            "`let mut` desnecessário no Rust gerado:\n{avisos}\n{rust}"
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        let linhas: Vec<&str> = stdout.lines().collect();
        assert_eq!(linhas.len(), 4, "stdout inesperado: {stdout}");
        assert_eq!(linhas[0], "fatorial: 120");
        // 10 / 3 é divisão float: 3.333…, não 3.
        assert!(
            linhas[1].starts_with("div: 3.333"),
            "divisão deveria ser float: {}",
            linhas[1]
        );
        assert_eq!(linhas[2], "pow: 1024");
        assert_eq!(linhas[3], "impar maior que 4");
        assert_eq!(output.status.code(), Some(0));
    }
}
