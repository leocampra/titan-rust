//! Backend Rust: traduz a AST tipada (`checker::TypedProgram`) em código Rust
//! legível.
//!
//! Espelha a estrutura do `titan/titan-compiler/coder.lua` (uma função por
//! variante de `Stat`/`Exp`, `codestat`/`codeexp`), mas emitindo Rust em vez
//! de C acoplado à API interna do Lua (PRD.md, resumo executivo).
//!
//! Mapeamento de tipos (PRD.md, T6; `string` unificado na T24) — mantido
//! isolado em [`rust_type_name`] e [`rust_param_type_name`] para que a Fase 2
//! (arrays, maps, records) possa trocar o modelo de memória sem espalhar a
//! mudança:
//!
//! | Titan | Rust |
//! |---|---|
//! | `integer` | `i64` |
//! | `float` | `f64` |
//! | `boolean` | `bool` |
//! | `string` (qualquer posição) | `String` |
//! | `nil` (retorno) | `()` |
//! | `{string}` (só param de `main`) | `&mut Vec<String>` |
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
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(titan_main(&mut args) as i32);
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
    let used = referenced_names(body);
    let param_list: Vec<String> = params
        .iter()
        .map(|(name, ty)| {
            let rust_name = if used.contains(name.as_str()) {
                name.clone()
            } else {
                format!("_{name}")
            };
            format!("{rust_name}: {}", rust_param_type_name(ty))
        })
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

/// Nomes lidos em algum ponto do corpo (`TypedExpKind::Var`), usado para
/// decidir se um parâmetro sai como `nome` ou `_nome` na assinatura — Rust
/// avisa (`unused_variables`) sobre parâmetros nunca lidos, e a Fase 0/1
/// tem programas legítimos que declaram `args: {string}` sem usá-lo.
fn referenced_names(stat: &TypedStat) -> std::collections::HashSet<String> {
    let mut names = std::collections::HashSet::new();
    collect_referenced_names_stat(stat, &mut names);
    names
}

fn collect_referenced_names_stat(stat: &TypedStat, names: &mut std::collections::HashSet<String>) {
    match stat {
        TypedStat::Block { stats, .. } => {
            for s in stats {
                collect_referenced_names_stat(s, names);
            }
        }
        TypedStat::Decl { value, .. } => collect_referenced_names_exp(value, names),
        TypedStat::Call { call, .. } => collect_referenced_names_exp(call, names),
        TypedStat::Return { exps, .. } => {
            for e in exps {
                collect_referenced_names_exp(e, names);
            }
        }
        TypedStat::If {
            thens, elsestat, ..
        } => {
            for then in thens {
                collect_referenced_names_exp(&then.condition, names);
                collect_referenced_names_stat(&then.block, names);
            }
            if let Some(elsestat) = elsestat {
                collect_referenced_names_stat(elsestat, names);
            }
        }
        TypedStat::While {
            condition, block, ..
        } => {
            collect_referenced_names_exp(condition, names);
            collect_referenced_names_stat(block, names);
        }
        TypedStat::For {
            start,
            finish,
            inc,
            block,
            ..
        } => {
            collect_referenced_names_exp(start, names);
            collect_referenced_names_exp(finish, names);
            collect_referenced_names_exp(inc, names);
            collect_referenced_names_stat(block, names);
        }
        TypedStat::Assign { value, .. } => collect_referenced_names_exp(value, names),
    }
}

fn collect_referenced_names_exp(exp: &TypedExp, names: &mut std::collections::HashSet<String>) {
    match &exp.kind {
        TypedExpKind::Var(name) => {
            names.insert(name.clone());
        }
        TypedExpKind::Call { args, .. } => {
            for a in args {
                collect_referenced_names_exp(a, names);
            }
        }
        TypedExpKind::Concat(parts) => {
            for p in parts {
                collect_referenced_names_exp(p, names);
            }
        }
        TypedExpKind::Binop { lhs, rhs, .. } => {
            collect_referenced_names_exp(lhs, names);
            collect_referenced_names_exp(rhs, names);
        }
        TypedExpKind::Unop { exp, .. } => collect_referenced_names_exp(exp, names),
        TypedExpKind::Nil
        | TypedExpKind::Bool(_)
        | TypedExpKind::Integer(_)
        | TypedExpKind::Float(_)
        | TypedExpKind::String(_) => {}
    }
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
        // `for` numérico sempre desaçucarado para `while`, nunca `Range` do
        // Rust: `.step_by` não aceita passo negativo nem float, e `Range<f64>`
        // não implementa `Iterator`. Um único template cobre integer/float,
        // `inc` omitido (o checker já materializou `1`/`1.0`), `inc` negativo
        // e `inc` só conhecido em runtime (PRD T15; ADR na T18). Sem caminho
        // otimizado para `inc = 1` literal nesta fase — otimização futura.
        TypedStat::For {
            name,
            ty,
            start,
            finish,
            inc,
            block,
            ..
        } => {
            let t = rust_type_name(ty);
            let inner = depth + 1;
            // Bloco externo: a variável de controle e as auxiliares não vazam
            // para fora do laço (semântica Titan) — e laços aninhados apenas
            // sombreiam as auxiliares do laço externo. O prefixo `titan_`
            // segue a convenção de mangling existente.
            indent(out, depth);
            out.push_str("{\n");
            indent(out, inner);
            out.push_str(&format!(
                "let mut {name}: {t} = {};\n",
                emit_delimited_exp(start)
            ));
            indent(out, inner);
            out.push_str(&format!(
                "let titan_for_finish: {t} = {};\n",
                emit_delimited_exp(finish)
            ));
            indent(out, inner);
            out.push_str(&format!(
                "let titan_for_inc: {t} = {};\n",
                emit_delimited_exp(inc)
            ));
            // A direção do laço é computada uma única vez, antes de entrar.
            indent(out, inner);
            out.push_str(&format!(
                "let titan_for_asc: bool = titan_for_inc > 0 as {t};\n"
            ));
            indent(out, inner);
            out.push_str(&format!(
                "while (titan_for_asc && {name} <= titan_for_finish)\n"
            ));
            indent(out, inner + 1);
            out.push_str(&format!(
                "|| (!titan_for_asc && {name} >= titan_for_finish) {{\n"
            ));
            emit_block_stats(out, block, inner + 1);
            indent(out, inner + 1);
            out.push_str(&format!("{name} += titan_for_inc;\n"));
            indent(out, inner);
            out.push_str("}\n");
            indent(out, depth);
            out.push_str("}\n");
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
/// inicializador de `let`, lado direito de atribuição, valor de `return`,
/// argumento de chamada a função Titan (T24: `string` é sempre `String`, em
/// toda posição — parâmetros de função Titan não são mais `&str`). Slot de
/// tipo `string` sempre precisa de uma `String` dona: literal ganha
/// `.to_string()`; variável ganha `.clone()` — copia em vez de mover, a
/// original continua utilizável depois de `local a: string = b`. Concat e
/// chamada já produzem `String` e passam direto.
fn emit_owned_string(exp: &TypedExp) -> String {
    match &exp.kind {
        TypedExpKind::String(_) => format!("{}.to_string()", emit_exp(exp)),
        TypedExpKind::Var(_) => format!("{}.clone()", emit_exp(exp)),
        _ => emit_delimited_exp(exp),
    }
}

/// Valor emitido para um "slot" — como [`emit_owned_string`], mas para
/// qualquer tipo: aplica a regra de `string` quando `slot_ty` é `String` e
/// delega para [`emit_delimited_exp`] no resto.
fn emit_slot_value(slot_ty: &Type, value: &TypedExp) -> String {
    if *slot_ty == Type::String {
        emit_owned_string(value)
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
            emit_comparison(symbol, lhs, rhs)
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
fn emit_comparison(symbol: &str, lhs: &TypedExp, rhs: &TypedExp) -> String {
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
    if lhs.ty == Type::String {
        // `String` não implementa `PartialOrd`/`PartialEq` cruzado com `&str`
        // no std — os dois lados precisam nascer como `String` mesmo em
        // `==`/`~=`, daí reusar [`emit_owned_string`] em vez de `emit_exp`.
        return format!(
            "{} {symbol} {}",
            emit_owned_string(lhs),
            emit_owned_string(rhs)
        );
    }
    // Igualdade de boolean: `bool == bool` direto.
    format!("{} {symbol} {}", emit_exp(lhs), emit_exp(rhs))
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
/// par, associando à esquerda. `titan_runtime::concat` continua pedindo
/// `&str` nos dois lados (T24 não muda a fronteira do runtime); o valor
/// devolvido aqui é o `String` **cru** que o `concat` produz (sem `&` — quem
/// precisar emprestá-lo usa [`borrow_runtime_str`], que sabe envolver
/// qualquer expressão, inclusive esta).
fn emit_concat(exps: &[TypedExp]) -> String {
    let mut parts = exps.iter();
    let first = parts
        .next()
        .expect("checker garante ExpConcat com ao menos um operando");
    // `acc` guarda sempre um `String` **cru** (sem `&`) — o que `concat`
    // devolve. Cada chamada empresta `acc` na hora de alimentar a próxima
    // (`&{acc}`); a última iteração deixa o resultado sem `&`, pronto para
    // ser usado como slot (`let`/atribuição/`return`) ou por
    // [`borrow_runtime_str`], que sabe emprestar qualquer expressão.
    let mut acc = borrow_runtime_str(first);
    let mut acc_is_raw = false;
    for e in parts {
        let lhs = if acc_is_raw {
            format!("&{acc}")
        } else {
            acc
        };
        acc = format!("titan_runtime::concat({lhs}, {})", borrow_runtime_str(e));
        acc_is_raw = true;
    }
    acc
}

/// Argumentos de uma chamada a função **Titan** (T24: parâmetros de tipo
/// `string` são sempre `String` dona — sem `&str`, sem alocação implícita
/// escondida do chamador). Reusa [`emit_owned_string`] para strings; o resto
/// segue a posição delimitada normal.
fn emit_call(callee: &str, args: &[TypedExp]) -> String {
    if callee == "print" {
        let rendered_args: Vec<String> = args.iter().map(borrow_runtime_str).collect();
        return format!("titan_runtime::print({})", rendered_args.join(", "));
    }
    let rendered_args: Vec<String> = args
        .iter()
        .map(|a| {
            if a.ty == Type::String {
                emit_owned_string(a)
            } else {
                emit_delimited_exp(a)
            }
        })
        .collect();
    format!("{}({})", mangle_fn_name(callee), rendered_args.join(", "))
}

/// Coage uma expressão numérica ou `string` para `&str`/referência esperada
/// pelo `titan-runtime` (`print(&str)`, `concat(&str, &str)`) — a única
/// fronteira que ainda pede empréstimo em vez de posse (T24: dentro do
/// programa gerado, `string` é sempre `String`). Número vira
/// `&x.to_string()` (decisão 4 da Fase 1); string usa [`emit_owned_string`]
/// e empresta o resultado.
fn borrow_runtime_str(exp: &TypedExp) -> String {
    if matches!(exp.ty, Type::Integer | Type::Float) {
        format!("&{}.to_string()", emit_exp(exp))
    } else {
        format!("&{}", emit_owned_string(exp))
    }
}

/// Tipo Rust de uma variável/expressão, em qualquer posição: `string` é
/// sempre `String` (T24 — zero casos especiais por posição).
fn rust_type_name(ty: &Type) -> String {
    match ty {
        Type::Nil => "()".to_string(),
        Type::Boolean => "bool".to_string(),
        Type::Integer => "i64".to_string(),
        Type::Float => "f64".to_string(),
        Type::String => "String".to_string(),
        Type::Array { elem } if **elem == Type::String => "Vec<String>".to_string(),
        other => unreachable!(
            "tipo '{other:?}' fora do subconjunto de codegen suportado — checker deveria ter rejeitado antes"
        ),
    }
}

/// Tipo Rust de um **parâmetro** de função: idêntico a [`rust_type_name`],
/// exceto o único parâmetro composto desta fase — `{string}` (só `main`) —
/// que sai por `&mut Vec<String>` em vez de por valor.
fn rust_param_type_name(ty: &Type) -> String {
    match ty {
        Type::Array { elem } if **elem == Type::String => "&mut Vec<String>".to_string(),
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

        // `args` não é lido no corpo de `hello.titan` — sai `_args` para o
        // Rust gerado não emitir `unused_variables`.
        assert!(rust.contains("pub fn titan_main(_args: &mut Vec<String>) -> i64 {"));
        assert!(rust.contains("titan_runtime::print(&\"Olá, mundo!\".to_string());"));
        assert!(rust.contains("return 0;"));
        assert!(rust.contains("fn main() {"));
        assert!(rust.contains("std::process::exit(titan_main(&mut args) as i32);"));
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

        let (avisos, output) = compila_e_executa(&rust, "hello");
        assert!(
            avisos.is_empty(),
            "warnings no Rust gerado:\n{avisos}\n{rust}"
        );
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
            "let a: String = titan_runtime::concat(&titan_runtime::concat(&\"x\".to_string(), &\"y\".to_string()), &\"z\".to_string());"
        ));
        assert!(rust.contains("titan_runtime::print(&a.clone());"));
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

    /// Parâmetro nunca lido no corpo sai `_nome` na assinatura, para o Rust
    /// gerado não emitir `unused_variables` — caso comum de `main(args:
    /// {string})` quando o programa não usa `args` (`hello.titan`,
    /// `nucleo.titan`). Quando o parâmetro É lido (mesmo só repassado para
    /// outra chamada), mantém o nome original.
    #[test]
    fn parametro_nao_usado_sai_com_underscore_e_usado_mantem_o_nome() {
        let source = r#"function conta(a: {string}): integer
    return 0
end

function main(args: {string}): integer
    return conta(args)
end"#;
        let rust = generate_source(source);
        assert!(rust.contains("fn titan_conta(_a: &mut Vec<String>) -> i64 {"));
        assert!(rust.contains("fn titan_main(args: &mut Vec<String>) -> i64 {"));
        assert!(rust.contains("return titan_conta(args);"));
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
        assert!(rust.contains("titan_runtime::concat(&\"i: \".to_string(), &42.to_string())"));
        assert!(rust.contains("titan_runtime::concat(&\"f: \".to_string(), &1.5.to_string())"));
        assert!(rust.contains("titan_runtime::concat(&\"exp: \".to_string(), &(1 + 2).to_string())"));
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
        assert!(rust.contains("let mut b: String = a.clone();"));
        assert!(rust.contains("b = \"tchau\".to_string();"));
    }

    #[test]
    fn comparacao_de_strings_usa_string_dos_dois_lados() {
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
        // T24: `string` é sempre `String` — sem `PartialOrd`/`PartialEq`
        // cruzado com `&str` no std, os dois lados nascem como `String`
        // mesmo em `==`.
        assert!(rust.contains("let menor: bool = a.clone() < \"abd\".to_string();"));
        assert!(rust.contains("let igual: bool = a.clone() == \"abc\".to_string();"));
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

        // Critério da fase: Rust gerado sem nenhum warning do rustc — nem
        // `let mut` sobrando, nem parênteses redundantes, nem `args` não
        // usado (corrigido via `_`-prefixing de parâmetros não lidos).
        assert!(
            avisos.is_empty(),
            "warnings no Rust gerado:\n{avisos}\n{rust}"
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

    // ---- T15: StatFor desaçucarado para while ---------------------------

    #[test]
    fn for_emite_template_while_desacucarado() {
        let source = r#"function main(args: {string}): integer
    for i = 1, 5 do
        print("x" .. i)
    end
    return 0
end"#;
        let rust = generate_source(source);
        assert!(rust.contains("let mut i: i64 = 1;"));
        assert!(rust.contains("let titan_for_finish: i64 = 5;"));
        assert!(rust.contains("let titan_for_inc: i64 = 1;"));
        assert!(rust.contains("let titan_for_asc: bool = titan_for_inc > 0 as i64;"));
        assert!(rust.contains("while (titan_for_asc && i <= titan_for_finish)"));
        assert!(rust.contains("|| (!titan_for_asc && i >= titan_for_finish) {"));
        assert!(rust.contains("i += titan_for_inc;"));
        // Nunca o Range do Rust (`.step_by` não cobre passo negativo/float).
        assert!(!rust.contains(".."));
        assert!(!rust.contains("step_by"));
    }

    #[test]
    fn for_float_usa_o_mesmo_template_com_f64() {
        let source = r#"function main(args: {string}): integer
    for x = 0.0, 1.0, 0.25 do
        print("passo")
    end
    return 0
end"#;
        let rust = generate_source(source);
        assert!(rust.contains("let mut x: f64 = 0.0;"));
        assert!(rust.contains("let titan_for_finish: f64 = 1.0;"));
        assert!(rust.contains("let titan_for_inc: f64 = 0.25;"));
        assert!(rust.contains("let titan_for_asc: bool = titan_for_inc > 0 as f64;"));
        assert!(rust.contains("x += titan_for_inc;"));
    }

    #[test]
    fn for_compila_e_roda_todos_os_casos_do_criterio() {
        let source = r#"function conta(inicio: integer, fim: integer, passo: integer): integer
    local n: integer = 0
    for i = inicio, fim, passo do
        n = n + 1
    end
    return n
end

function main(args: {string}): integer
    for i = 1, 5 do
        print("a" .. i)
    end
    for i = 5, 1, -1 do
        print("b" .. i)
    end
    for i = 1, 10, 2 do
        print("c" .. i)
    end
    local cont: integer = 0
    for x = 0.0, 1.0, 0.25 do
        cont = cont + 1
    end
    print("cont: " .. cont)
    print("d: " .. conta(10, 1, -3))
    for i = 1, 2 do
        for j = 1, 2 do
            print("n" .. i .. j)
        end
    end
    for i = 1, 0 do
        print("nunca")
    end
    return 0
end"#;
        let rust = generate_source(source);
        let (avisos, output) = compila_e_executa(&rust, "for-t15");

        assert!(
            avisos.is_empty(),
            "warnings no Rust gerado:\n{avisos}\n{rust}"
        );

        // Linha a linha: `for i = 1, 5` crescente (a1..a5) ·
        // `for i = 5, 1, -1` decrescente (b5..b1) · `for i = 1, 10, 2`
        // (c1,c3,c5,c7,c9) · `for x = 0.0, 1.0, 0.25` float conferido por
        // contagem (cont: 5) · passo negativo só conhecido em runtime, via
        // parâmetro (d: 4) · laços aninhados, auxiliares internas apenas
        // sombreiam (n11..n22) · `for i = 1, 0` zero iterações (sem "nunca").
        let esperado = "a1\na2\na3\na4\na5\n\
                        b5\nb4\nb3\nb2\nb1\n\
                        c1\nc3\nc5\nc7\nc9\n\
                        cont: 5\nd: 4\n\
                        n11\nn12\nn21\nn22\n";
        assert_eq!(String::from_utf8_lossy(&output.stdout), esperado);
        assert_eq!(output.status.code(), Some(0));
    }
}
