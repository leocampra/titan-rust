//! Backend Rust: traduz a AST tipada (`checker::TypedProgram`) em código Rust
//! legível.
//!
//! Espelha a estrutura do `titan/titan-compiler/coder.lua` (uma função por
//! variante de `Stat`/`Exp`, `codestat`/`codeexp`), mas emitindo Rust em vez
//! de C acoplado à API interna do Lua (PRD.md, resumo executivo).
//!
//! Mapeamento de tipos (PRD.md, T6/T30; `string` unificado na T24) — mantido
//! isolado em [`rust_type_name`] e [`rust_param_type_name`] para que o modelo
//! de memória continue trocável num lugar só:
//!
//! | Titan | Rust |
//! |---|---|
//! | `integer` | `i64` |
//! | `float` | `f64` |
//! | `boolean` | `bool` |
//! | `string` (qualquer posição) | `String` |
//! | `nil` (retorno) | `()` |
//! | N>1 retornos (T66) | `(T1, T2, ...)` ([`rust_rettype_name`]) |
//! | `{T}` | `Vec<T>` (`&mut Vec<T>` em posição de parâmetro) |
//! | `{K: V}` | `HashMap<K, V>` (`&mut HashMap<K, V>` em posição de parâmetro) |
//! | `record Nome` | `struct Nome` (`&mut Nome` em posição de parâmetro) |
//!
//! Seis operadores **não** mapeiam para o símbolo de grafia igual no Rust
//! (PRD.md, T61) — o cruzamento entre `~` e `^` é a armadilha principal:
//!
//! | Titan | Significado | Rust |
//! |---|---|---|
//! | `^` | potência | `.powf` ([`emit_pow`]) |
//! | `~` binário | XOR | `^` |
//! | `~` unário | bitwise NOT | `!` (o mesmo de `not`, sobre `i64`) |
//! | `//` | divisão com piso | `titan_runtime::idiv` ([`emit_idiv`]) |
//! | `<<` `>>` | deslocamento de qualquer tamanho | `titan_runtime::shl`/`shr` ([`emit_shift`]) |
//!
//! `//` e os deslocamentos merecem destaque, e pelo mesmo motivo: existe um
//! operador Rust de grafia igual, mas com semântica diferente — usar o
//! símbolo direto compilaria e daria a resposta errada. O `/` trunca em direção a zero
//! onde o Titan/Lua arredonda para baixo (`-7 // 2` é `-4`, não `-3`); o
//! `<<` exige deslocamento em `0..64` e transborda fora disso, onde o
//! Titan/Lua aceita qualquer inteiro (negativo inverte a direção, 64 ou mais
//! zera).
//!
//! Nada aqui assume que valores são `Copy` (decisão 1 da Fase 2, PRD.md): a
//! semântica de valor de arrays/maps/records vem de clonar explicitamente na
//! atribuição ([`precisa_clone`]), nunca de derivar `Copy`.

use crate::checker::{
    BinOp, Callee, TypedExp, TypedExpKind, TypedLValue, TypedMultiValues, TypedProgram, TypedStat,
    TypedThen, TypedTopLevel, UnOp,
};
use crate::types::Type;
use std::collections::HashSet;

const INDENT: &str = "    ";

/// Nomes de parâmetro composto (`array`/`map`/`record`) da função **atual**
/// — dentro do corpo, esses nomes já são uma referência Rust (`&mut T`,
/// [`rust_param_type_name`]), então emprestá-los de novo (`&x`/`&mut x`)
/// duplicaria a referência (`&mut &mut Vec<_>`) e o rustc recusaria o
/// reborrow sem `mut` na ligação. Toda função de emissão que decide entre
/// "nome cru" e "nome emprestado" (T30) recebe este conjunto para saber
/// distinguir os dois casos; variável local composta não entra aqui — ela é
/// dona do valor e precisa do empréstimo normal.
type Ctx<'a> = &'a HashSet<String>;

/// Uma construção que o checker já tipa, mas que este backend ainda não sabe
/// emitir. Nunca indica erro do programa Titan em si (o checker já validou
/// isso); é limitação estrutural do codegen para tipos fora do escopo da
/// Fase 2 (`value`, `Option`, tipo de função como valor) — nenhum deles chega
/// aqui de fato, porque o checker já os rejeita em `resolve_type` antes.
#[derive(Debug)]
pub struct CodegenError(pub String);

impl std::fmt::Display for CodegenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for CodegenError {}

/// Gera o `main.rs` completo (structs de record + funções do programa + shim
/// de entrada) a partir da AST tipada.
///
/// Records saem primeiro, num laço à parte — nenhuma função os referencia
/// antes de todos estarem declarados, mas manter a ordem "tipos antes de
/// funções" é convenção usual do Rust gerado.
pub fn generate(program: &TypedProgram) -> Result<String, CodegenError> {
    let mut out = String::new();

    for top in program {
        if let TypedTopLevel::Record { name, fields, .. } = top {
            emit_record_struct(&mut out, name, fields);
            out.push('\n');
        }
    }

    for top in program {
        if matches!(top, TypedTopLevel::Func { .. }) {
            emit_toplevel(&mut out, top);
            out.push('\n');
        }
    }

    out.push_str(ENTRY_SHIM);
    Ok(out)
}

/// `struct Nome { pub campo: Tipo, .. }` — `Clone` é obrigatório (decisão 1
/// da Fase 2: `local b = a` clona um record); `Copy` nunca sai, porque um
/// record pode conter `String`/`Vec`/outro record não-`Copy`. Sem mangling no
/// nome: o namespace de tipos do Rust não colide com o `fn main` do shim
/// (ADR 0009).
fn emit_record_struct(out: &mut String, name: &str, fields: &[(String, Type)]) {
    out.push_str("#[derive(Clone, Debug, PartialEq)]\n");
    out.push_str("pub struct ");
    out.push_str(name);
    out.push_str(" {\n");
    for (fname, fty) in fields {
        out.push_str(INDENT);
        out.push_str("pub ");
        out.push_str(fname);
        out.push_str(": ");
        out.push_str(&rust_type_name(fty));
        out.push_str(",\n");
    }
    out.push_str("}\n");
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
    } = top
    else {
        // `generate` só chama `emit_toplevel` para `TypedTopLevel::Func` —
        // `Record` é emitido à parte por `emit_record_struct`.
        unreachable!("`generate` só encaminha `TypedTopLevel::Func` a `emit_toplevel`")
    };

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

    if let Some(ret) = rust_rettype_name(rettypes) {
        out.push_str(" -> ");
        out.push_str(&ret);
    }

    // Parâmetros compostos já chegam como `&mut T` (`rust_param_type_name`)
    // — dentro do corpo, o nome é uma referência, não um valor dono. `ctx`
    // carrega essa lista para toda a emissão do corpo saber a diferença.
    let ctx: HashSet<String> = params
        .iter()
        .filter(|(_, ty)| is_composite(ty))
        .map(|(name, _)| name.clone())
        .collect();

    out.push_str(" {\n");
    emit_block_stats(out, body, 1, &ctx);
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
        // T67: os alvos da declaração múltipla são destinos, não leituras —
        // só o lado direito conta, como no `Decl` simples.
        TypedStat::DeclMulti { values, .. } => collect_referenced_names_multi(values, names),
        TypedStat::AssignMulti {
            targets, values, ..
        } => {
            for target in targets {
                collect_referenced_names_lvalue(target, names);
            }
            collect_referenced_names_multi(values, names);
        }
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
        // `repeat` (T64): mesma leitura do `while`, invertida só na ordem em
        // que corpo e condição aparecem no fonte — para efeito de "que nomes
        // este statement lê", a ordem não importa.
        TypedStat::Repeat {
            block, condition, ..
        } => {
            collect_referenced_names_stat(block, names);
            collect_referenced_names_exp(condition, names);
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
        TypedStat::Assign { target, value, .. } => {
            collect_referenced_names_lvalue(target, names);
            collect_referenced_names_exp(value, names);
        }
        TypedStat::Break { .. } | TypedStat::Continue { .. } => {}
    }
}

/// Alvo de atribuição (T25): `Name` não lê nada (é o próprio destino), mas
/// `Index`/`Field` embutem uma expressão-base que pode referenciar um nome —
/// sem produtor ainda (T29/T30), mas o `match` já precisa ser exaustivo.
fn collect_referenced_names_lvalue(
    target: &TypedLValue,
    names: &mut std::collections::HashSet<String>,
) {
    match target {
        TypedLValue::Name(_) => {}
        TypedLValue::Index { base, index } => {
            collect_referenced_names_exp(base, names);
            collect_referenced_names_exp(index, names);
        }
        TypedLValue::Field { base, .. } => collect_referenced_names_exp(base, names),
    }
}

/// Lado direito de uma declaração/atribuição múltipla (T67): a chamada
/// única ou cada expressão da lista.
fn collect_referenced_names_multi(
    values: &TypedMultiValues,
    names: &mut std::collections::HashSet<String>,
) {
    match values {
        TypedMultiValues::Call(call) => collect_referenced_names_exp(call, names),
        TypedMultiValues::List(exps) => {
            for e in exps {
                collect_referenced_names_exp(e, names);
            }
        }
    }
}

fn collect_referenced_names_exp(exp: &TypedExp, names: &mut std::collections::HashSet<String>) {
    match &exp.kind {
        TypedExpKind::Var(name) => {
            names.insert(name.clone());
        }
        TypedExpKind::Call { callee, args } => {
            // `Callee::Method` (T40/T42) embute o receptor (`df` em
            // `df.soma(...)`) como uma expressão própria — se for um
            // parâmetro só lido através do método, ele conta como "usado"
            // tanto quanto qualquer outro `Var`, senão a assinatura sairia
            // `_df` (`unused_variables`) mesmo com o corpo lendo `df` de
            // verdade.
            if let Callee::Method { recv, .. } = callee {
                collect_referenced_names_exp(recv, names);
            }
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
        TypedExpKind::Index { base, index } => {
            collect_referenced_names_exp(base, names);
            collect_referenced_names_exp(index, names);
        }
        TypedExpKind::Field { base, .. } => collect_referenced_names_exp(base, names),
        TypedExpKind::ArrayLit(exps) => {
            for e in exps {
                collect_referenced_names_exp(e, names);
            }
        }
        TypedExpKind::RecordLit { fields, .. } => {
            for (_, e) in fields {
                collect_referenced_names_exp(e, names);
            }
        }
        TypedExpKind::MapLit(entries) => {
            for (k, v) in entries {
                collect_referenced_names_exp(k, names);
                collect_referenced_names_exp(v, names);
            }
        }
        // Ajuste de retorno múltiplo (T65): os nomes lidos são os da
        // chamada envolvida.
        TypedExpKind::Adjust(inner)
        | TypedExpKind::Extra { exp: inner, .. }
        // `SomeOf` (T68) é um invólucro: quem é lido é o valor dentro dele.
        | TypedExpKind::SomeOf(inner) => collect_referenced_names_exp(inner, names),
        TypedExpKind::Nil
        | TypedExpKind::Bool(_)
        | TypedExpKind::Integer(_)
        | TypedExpKind::Float(_)
        | TypedExpKind::String(_) => {}
    }
}

/// Emite os comandos de um `TypedStat::Block` (o único formato de corpo de
/// função na Fase 0) já indentados.
fn emit_block_stats(out: &mut String, stat: &TypedStat, depth: usize, ctx: Ctx) {
    let TypedStat::Block { stats, .. } = stat else {
        emit_stat(out, stat, depth, ctx);
        return;
    };
    for s in stats {
        emit_stat(out, s, depth, ctx);
    }
}

fn indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str(INDENT);
    }
}

/// Corpo de um ramo `then` que pode ter estreitado opcionais (T68/T69).
///
/// Para cada nome em `then.narrowed` o corpo foi **tipado** contra o tipo
/// base (`integer`, não `integer?`), então dentro dele toda leitura de `x`
/// emite o nome cru e espera um `i64`. Fora do ramo o mesmo nome é um
/// `Option<i64>`. A ponte entre os dois é uma ligação que sombreia o nome
/// com o valor desembrulhado, aberta no topo do ramo:
///
/// ```ignore
/// if x.is_some() {
///     let x: i64 = x.clone().unwrap();
///     /* corpo, lendo `x` como i64 */
/// }
/// ```
///
/// O `.clone()` antes do `.unwrap()` não é supérfluo: `unwrap` **consome**
/// o opcional, e o de fora precisa continuar vivo depois do `if` (é a única
/// razão de o estreitamento não vazar ser uma questão de escopo, e não de
/// valor movido). Para um `Option<i64>` o clone é uma cópia trivial que o
/// rustc apaga; para `Option<String>` ele é o que evita o E0382.
///
/// **Atribuir dentro do ramo** é a armadilha que a T68 registrou: um
/// `let x` novo é uma variável nova, e `x = 2` lá dentro escreveria nela,
/// não na de fora. Quando o ramo atribui ao nome estreitado, a emissão muda
/// de forma — um alias `&mut` para a variável externa é criado **antes** do
/// sombreamento, e o valor volta por ele no fim do ramo:
///
/// ```ignore
/// if x.is_some() {
///     let titan_opt_x: &mut Option<i64> = &mut x;
///     let mut x: i64 = titan_opt_x.clone().unwrap();
///     /* corpo, que atribui a `x` */
///     *titan_opt_x = Some(x);
/// }
/// ```
///
/// O write-back no fim (e não a cada atribuição) basta porque ninguém de
/// fora do ramo enxerga a variável enquanto o ramo roda. Um `return` no
/// meio do corpo pula o write-back — o que é exatamente o correto: a função
/// termina ali e a variável externa morre sem ser observada.
///
/// Sem nomes estreitados — o caso de toda condição que não testa opcional —
/// nada disso aparece e o corpo sai exatamente como antes da T69.
fn emit_narrowed_block(out: &mut String, then: &TypedThen, depth: usize, ctx: Ctx) {
    if then.narrowed.is_empty() {
        emit_block_stats(out, &then.block, depth, ctx);
        return;
    }

    // Tipo base de cada nome estreitado: o `Option<T>` está no tipo da
    // condição, não aqui, então quem sabe o `T` é o próprio corpo — a
    // primeira leitura do nome tem o tipo base que o checker lhe deu.
    // Nome estreitado que o corpo nunca lê nem escreve não ganha ligação
    // nenhuma: abrir um `let` que ninguém usa renderia `unused_variables`,
    // e o critério de aceite da T69 é Rust gerado **sem warnings**.
    let lidos = referenced_names(&then.block);

    for nome in &then.narrowed {
        let reatribuido = assigns_to_name(&then.block, nome);
        if !lidos.contains(nome) && !reatribuido {
            continue;
        }
        let Some(base) = narrowed_base_type(&then.condition, nome) else {
            continue;
        };
        let t = rust_type_name(&base);
        if reatribuido {
            indent(out, depth);
            out.push_str(&format!(
                "let {}: &mut Option<{t}> = &mut {nome};\n",
                alias_opcional(nome)
            ));
            indent(out, depth);
            out.push_str(&format!(
                "let mut {nome}: {t} = {}.clone().unwrap();\n",
                alias_opcional(nome)
            ));
        } else {
            indent(out, depth);
            out.push_str(&format!("let {nome}: {t} = {nome}.clone().unwrap();\n"));
        }
    }

    emit_block_stats(out, &then.block, depth, ctx);

    // Write-back, na ordem inversa da abertura: o alias mais interno é o
    // último aberto, e escrever de dentro para fora mantém cada `*alias`
    // ainda em escopo.
    for nome in then.narrowed.iter().rev() {
        if assigns_to_name(&then.block, nome) && narrowed_base_type(&then.condition, nome).is_some()
        {
            indent(out, depth);
            out.push_str(&format!("*{} = Some({nome});\n", alias_opcional(nome)));
        }
    }
}

/// Nome do alias `&mut Option<T>` que segura a variável externa enquanto o
/// ramo estreitado a sombreia. Prefixo `titan_` como todo o resto do
/// mangling do backend.
fn alias_opcional(nome: &str) -> String {
    format!("titan_opt_{nome}")
}

/// Tipo base do nome estreitado — o `T` de um `x: T?` que vale dentro do
/// ramo.
///
/// A lista `narrowed` do `TypedThen` traz só os nomes; o tipo vem da
/// **condição**, onde `x` ainda aparece com o tipo opcional que tinha antes
/// do teste (`x ~= nil` tipa o `x` como `integer?`). Ler dali, e não do
/// corpo, é o que faz um nome estreitado mas nunca usado no corpo continuar
/// respondendo com o tipo certo.
///
/// `None` quer dizer que a condição não menciona o nome com tipo opcional —
/// não deveria acontecer para um nome que o checker pôs em `narrowed`, mas
/// devolver `None` deixa a emissão seguir sem desembrulhar nada em vez de
/// entrar em pânico.
fn narrowed_base_type(condition: &TypedExp, nome: &str) -> Option<Type> {
    fn busca(exp: &TypedExp, nome: &str) -> Option<Type> {
        if let TypedExpKind::Var(n) = &exp.kind
            && n == nome
            && let Type::Option { base } = &exp.ty
        {
            return Some(base.as_ref().clone());
        }
        // A condição só precisa ser varrida através do `and`, que é a única
        // forma composta que o checker (`check_if_condition`) atravessa ao
        // montar `narrowed`; o teste em si é sempre um `~=` no topo.
        match &exp.kind {
            TypedExpKind::Binop { lhs, rhs, .. } => busca(lhs, nome).or_else(|| busca(rhs, nome)),
            _ => None,
        }
    }
    busca(condition, nome)
}

/// `true` quando o bloco atribui **diretamente** ao nome (`x = ...`,
/// `a, x = ...`) — o caso que obriga o write-back de
/// [`emit_narrowed_block`].
///
/// Escrever *através* do nome (`x[1] = 9`, `x.campo = 9`) não conta: ali o
/// alvo é o composto que o nome já aponta, e a variável externa em si não
/// muda. Um `local x` novo dentro do ramo também não conta — é outra
/// variável, que o próprio Rust sombreia.
fn assigns_to_name(stat: &TypedStat, nome: &str) -> bool {
    fn lvalue_e_o_nome(target: &TypedLValue, nome: &str) -> bool {
        matches!(target, TypedLValue::Name(n) if n == nome)
    }
    fn stat_atribui(stat: &TypedStat, nome: &str) -> bool {
        match stat {
            TypedStat::Block { stats, .. } => stats.iter().any(|s| stat_atribui(s, nome)),
            TypedStat::Assign { target, .. } => lvalue_e_o_nome(target, nome),
            TypedStat::AssignMulti { targets, .. } => {
                targets.iter().any(|t| lvalue_e_o_nome(t, nome))
            }
            TypedStat::If {
                thens, elsestat, ..
            } => {
                thens.iter().any(|t| stat_atribui(&t.block, nome))
                    || elsestat.as_ref().is_some_and(|e| stat_atribui(e, nome))
            }
            TypedStat::While { block, .. }
            | TypedStat::Repeat { block, .. }
            | TypedStat::For { block, .. } => stat_atribui(block, nome),
            TypedStat::Decl { .. }
            | TypedStat::DeclMulti { .. }
            | TypedStat::Call { .. }
            | TypedStat::Return { .. }
            | TypedStat::Break { .. }
            | TypedStat::Continue { .. } => false,
        }
    }
    stat_atribui(stat, nome)
}

fn emit_stat(out: &mut String, stat: &TypedStat, depth: usize, ctx: Ctx) {
    match stat {
        TypedStat::Block { .. } => {
            indent(out, depth);
            out.push_str("{\n");
            emit_block_stats(out, stat, depth + 1, ctx);
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
            out.push_str(&emit_slot_value(ty, value, ctx));
            out.push_str(";\n");
        }
        // `local a, b = ...` (T67). Os valores saem **todos** para
        // temporários antes de qualquer `let` do usuário: na forma de
        // chamada, porque é uma tupla a desestruturar; na forma de lista,
        // pela mesma ordem de avaliação que o `AssignMulti` abaixo exige —
        // e aqui o efeito extra é que um nome sendo declarado não sombreia
        // o homônimo externo que o lado direito lê (`local x, y = y, x`).
        TypedStat::DeclMulti {
            targets, values, ..
        } => {
            let slots: Vec<&Type> = targets.iter().map(|t| &t.ty).collect();
            let temporarios = emit_multi_values(out, values, &slots, targets.len(), depth, ctx);
            for (target, temporario) in targets.iter().zip(&temporarios) {
                indent(out, depth);
                out.push_str(if target.mutable { "let mut " } else { "let " });
                out.push_str(&target.name);
                out.push_str(": ");
                out.push_str(&rust_type_name(&target.ty));
                out.push_str(" = ");
                out.push_str(temporario);
                out.push_str(";\n");
            }
        }
        // `a, b = ...` (T67) — a armadilha central da tarefa. Em Titan,
        // como em Lua, **todo** o lado direito é avaliado antes de qualquer
        // escrita: `a, b = b, a` troca de verdade, e emitir as atribuições
        // em sequência (`a = b; b = a;`) daria `a == b`. Por isso os valores
        // vão primeiro para `let` temporários e só depois são escritos nos
        // alvos, cada um pelo mesmo caminho do `Assign` single-target.
        TypedStat::AssignMulti {
            targets, values, ..
        } => {
            let slots: Vec<&Type> = match values {
                // O tipo do slot é o do próprio valor, como no `Assign`
                // single-target: o checker já garantiu `compatible`.
                TypedMultiValues::List(exps) => exps.iter().map(|e| &e.ty).collect(),
                // Na desestruturação os componentes já vêm da tupla tipada
                // pela assinatura; `emit_multi_values` ignora os slots aqui.
                TypedMultiValues::Call(_) => Vec::new(),
            };
            let temporarios = emit_multi_values(out, values, &slots, targets.len(), depth, ctx);
            for (target, temporario) in targets.iter().zip(&temporarios) {
                emit_assign_to_lvalue(out, target, temporario, depth, ctx);
            }
        }
        TypedStat::Call { call, .. } => {
            indent(out, depth);
            out.push_str(&emit_exp(call, ctx));
            out.push_str(";\n");
        }
        TypedStat::Return { exps, .. } => {
            indent(out, depth);
            out.push_str("return");
            // Retorno múltiplo (T66): N>1 valores viram **uma** tupla Rust,
            // o par da tupla que `rust_rettype_name` pôs na assinatura. Cada
            // componente passa por `emit_slot_value` com o **seu** tipo, do
            // mesmo jeito que o retorno único sempre passou — é o que faz
            // `string` sair dona e composto ganhar `.clone()` quando a fonte
            // é um lugar que sobrevive à chamada (ADR 0006/0007). Nenhum
            // valor e um valor só continuam exatamente como antes da T66:
            // `return;` e `return x;`, nunca `(x,)`.
            match exps.as_slice() {
                [] => {}
                [value] => {
                    out.push(' ');
                    out.push_str(&emit_slot_value(&value.ty, value, ctx));
                }
                vários => {
                    let componentes: Vec<String> = vários
                        .iter()
                        .map(|value| emit_slot_value(&value.ty, value, ctx))
                        .collect();
                    out.push_str(" (");
                    out.push_str(&componentes.join(", "));
                    out.push(')');
                }
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
                out.push_str(&emit_delimited_exp(&then.condition, ctx));
                out.push_str(" {\n");
                emit_narrowed_block(out, then, depth + 1, ctx);
                indent(out, depth);
                out.push('}');
                keyword = " else if ";
            }
            if let Some(els) = elsestat {
                out.push_str(" else {\n");
                emit_block_stats(out, els, depth + 1, ctx);
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
            out.push_str(&emit_delimited_exp(condition, ctx));
            out.push_str(" {\n");
            emit_block_stats(out, block, depth + 1, ctx);
            indent(out, depth);
            out.push_str("}\n");
        }
        // `repeat corpo until cond` (T64) → `loop { corpo; if cond {
        // break; } }`. O `loop` do Rust é o único laço que não testa nada no
        // topo, que é exatamente a semântica do `repeat`: o corpo roda ao
        // menos uma vez. Corpo e condição saem dentro das **mesmas** chaves,
        // e não em blocos separados, porque em Titan — como em Lua — o
        // `until` enxerga os `local` do corpo (o checker já tipou os dois no
        // mesmo escopo). `break` e `continue` do usuário caem no `loop` sem
        // caso especial (ADR 0023): `break` sai, e `continue` volta ao topo
        // — o que, aqui, **pula o teste do `until`** daquela iteração, uma
        // divergência deliberada do C e do idioma `goto continue` do Lua,
        // registrada no ADR.
        TypedStat::Repeat {
            block, condition, ..
        } => {
            indent(out, depth);
            out.push_str("loop {\n");
            emit_block_stats(out, block, depth + 1, ctx);
            indent(out, depth + 1);
            out.push_str("if ");
            out.push_str(&emit_delimited_exp(condition, ctx));
            out.push_str(" {\n");
            indent(out, depth + 2);
            out.push_str("break;\n");
            indent(out, depth + 1);
            out.push_str("}\n");
            indent(out, depth);
            out.push_str("}\n");
        }
        TypedStat::Assign { target, value, .. } => {
            // O tipo do valor serve de tipo do slot: o checker garantiu que
            // ele é `compatible` com o da variável, e `compatible` não coage
            // entre primitivas distintas nesta fase.
            let valor = emit_slot_value(&value.ty, value, ctx);
            emit_assign_to_lvalue(out, target, &valor, depth, ctx);
        }
        // `for` numérico emitido como `loop` do Rust, nunca `Range`:
        // `.step_by` não aceita passo negativo nem float, e `Range<f64>` não
        // implementa `Iterator`. Um único template cobre integer/float, `inc`
        // omitido (o checker já materializou `1`/`1.0`), `inc` negativo e
        // `inc` só conhecido em runtime (PRD T15; ADR 0004 na T18).
        //
        // T62 revisa o ADR 0004: o desaçucaramento para `while` punha o
        // incremento no **fim** do corpo, e um `continue` do usuário pularia
        // por cima dele — laço infinito (o impeditivo que o ADR 0017
        // registrou). Aqui o incremento vai para o **topo** do `loop`,
        // guardado por um flag de primeira iteração, e o teste de parada vem
        // logo depois. Assim todo caminho que volta ao topo — queda natural
        // do fim do corpo ou `continue` explícito — passa pelo incremento.
        // Sem caminho otimizado para `inc = 1` literal nesta fase.
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
                emit_delimited_exp(start, ctx)
            ));
            indent(out, inner);
            out.push_str(&format!(
                "let titan_for_finish: {t} = {};\n",
                emit_delimited_exp(finish, ctx)
            ));
            indent(out, inner);
            out.push_str(&format!(
                "let titan_for_inc: {t} = {};\n",
                emit_delimited_exp(inc, ctx)
            ));
            // A direção do laço é computada uma única vez, antes de entrar.
            indent(out, inner);
            out.push_str(&format!(
                "let titan_for_asc: bool = titan_for_inc > 0 as {t};\n"
            ));
            // Incremento no topo, pulado só na primeira volta. O flag é a
            // única forma de manter o incremento antes do corpo sem alterar o
            // valor visto na primeira iteração.
            indent(out, inner);
            out.push_str("let mut titan_for_primeira: bool = true;\n");
            indent(out, inner);
            out.push_str("loop {\n");
            indent(out, inner + 1);
            out.push_str("if titan_for_primeira {\n");
            indent(out, inner + 2);
            out.push_str("titan_for_primeira = false;\n");
            indent(out, inner + 1);
            out.push_str("} else {\n");
            indent(out, inner + 2);
            out.push_str(&format!("{name} += titan_for_inc;\n"));
            indent(out, inner + 1);
            out.push_str("}\n");
            // Teste de parada depois do incremento: mesma condição de
            // continuação de antes, negada.
            indent(out, inner + 1);
            out.push_str(&format!(
                "if !((titan_for_asc && {name} <= titan_for_finish)\n"
            ));
            indent(out, inner + 2);
            out.push_str(&format!(
                "|| (!titan_for_asc && {name} >= titan_for_finish)) {{\n"
            ));
            indent(out, inner + 2);
            out.push_str("break;\n");
            indent(out, inner + 1);
            out.push_str("}\n");
            emit_block_stats(out, block, inner + 1, ctx);
            indent(out, inner);
            out.push_str("}\n");
            indent(out, depth);
            out.push_str("}\n");
        }
        TypedStat::Break { .. } => {
            indent(out, depth);
            out.push_str("break;\n");
        }
        // `continue` (T63) emite literalmente `continue;`, sem label: o
        // template do `for` (ADR 0022) põe o incremento no topo do `loop`, de
        // modo que voltar ao topo já avança a variável de controle, e o
        // `while` do Rust reavalia a condição — nenhum laço auxiliar.
        TypedStat::Continue { .. } => {
            indent(out, depth);
            out.push_str("continue;\n");
        }
    }
}

/// Escreve `valor` — texto Rust **já emitido** — no lugar designado por
/// `target`. É o corpo que o braço `TypedStat::Assign` sempre teve; virou
/// função na T67 para a atribuição múltipla escrever em cada um dos seus
/// alvos exatamente pelo mesmo caminho, inclusive `v[i]` e `p.campo`.
fn emit_assign_to_lvalue(
    out: &mut String,
    target: &TypedLValue,
    valor: &str,
    depth: usize,
    ctx: Ctx,
) {
    indent(out, depth);
    match target {
        TypedLValue::Name(name) => {
            out.push_str(name);
            out.push_str(" = ");
            out.push_str(valor);
            out.push_str(";\n");
        }
        // `v[i] = x`: `array_set`/`map_set` do runtime (decisão 5 da
        // Fase 2 — `array_set` escreve em `1..#v`, faz append em
        // `#v + 1`, aborta com mensagem em português no resto).
        // `base` é o array/map inteiro — [`emit_place_mut`] resolve
        // um `&mut` de verdade a ele, mesmo quando `base` é ele
        // mesmo aninhado (`m["a"][1] = x`, `xs[i][j] = x`).
        // Índice e valor são pré-computados em variáveis `let` antes
        // do `&mut` do `base` ser tomado: o índice pode ler o
        // próprio `base` (`res[#res + 1] = x`, o idioma de "append"
        // da decisão 5 da Fase 2), e `emit_place_mut(base)` produz um
        // empréstimo mutável que o rustc não consegue provar
        // disjunto de um segundo empréstimo do mesmo `base` dentro
        // dos argumentos da mesma chamada — mesmo sendo
        // semanticamente sequencial (E0502). Nomes prefixados com
        // `titan_` seguem a convenção de mangling existente.
        TypedLValue::Index { base, index } => match &base.ty {
            Type::Array { .. } => {
                out.push_str(&format!(
                    "let titan_idx = {};\n",
                    emit_delimited_exp(index, ctx)
                ));
                indent(out, depth);
                out.push_str(&format!("let titan_val = {valor};\n"));
                indent(out, depth);
                out.push_str(&format!(
                    "titan_runtime::array_set({}, titan_idx, titan_val);\n",
                    emit_place_mut(base, ctx),
                ));
            }
            Type::Map { .. } => {
                out.push_str(&format!(
                    "let titan_key = {};\n",
                    emit_slot_value(&index.ty, index, ctx)
                ));
                indent(out, depth);
                out.push_str(&format!("let titan_val = {valor};\n"));
                indent(out, depth);
                out.push_str(&format!(
                    "titan_runtime::map_set({}, titan_key, titan_val);\n",
                    emit_place_mut(base, ctx),
                ));
            }
            other => {
                unreachable!("checker só produz `Index` sobre array/map, encontrado {other:?}")
            }
        },
        // `p.campo = x`: campo é `pub`, atribuição direta. `base`
        // pode ser aninhado (`pontos[1].x = 9`, onde `base` é um
        // `Index`) — `emit_place_mut(base)` resolve um `&mut Ponto`
        // de verdade (via `array_get_mut` na recursão) em vez do
        // `Ponto` clonado que `emit_exp`/`array_get` devolveriam.
        // Os parênteses são obrigatórios: sem eles, `&mut p.x = ..`
        // parsearia como `&mut (p.x) = ..` (atribuição a uma
        // referência recém-criada, não ao campo) — `(&mut p).x = ..`
        // é que aciona o auto-deref do Rust e escreve no lugar certo.
        TypedLValue::Field { base, name } => {
            out.push_str(&format!(
                "({}).{name} = {valor};\n",
                emit_place_mut(base, ctx)
            ));
        }
    }
}

/// Emite o lado direito de uma declaração/atribuição múltipla (T67) em
/// `let` temporários e devolve os nomes deles, na ordem dos alvos.
///
/// É aqui que a semântica do Lua fica garantida: quando a emissão chega aos
/// alvos, **todo** o lado direito já foi avaliado. Para `a, b = b, a` isso é
/// a diferença entre trocar de verdade e acabar com `a == b`.
///
/// `slots` traz o tipo de slot de cada valor da forma-lista (ver
/// [`emit_slot_value`]: é o que decide `String` dona e `.clone()` de
/// composto); na forma-chamada ele é vazio, porque a tupla devolvida pela
/// função já é dona dos seus componentes — lá quem dá a contagem é
/// `alvos`.
fn emit_multi_values(
    out: &mut String,
    values: &TypedMultiValues,
    slots: &[&Type],
    alvos: usize,
    depth: usize,
    ctx: Ctx,
) -> Vec<String> {
    match values {
        // Desestruturação da tupla da T66 num único `let` com padrão: o
        // checker já conferiu que a aridade da assinatura bate com a dos
        // alvos, então cada componente tem seu temporário.
        TypedMultiValues::Call(call) => {
            let nomes: Vec<String> = (0..alvos).map(|i| format!("titan_multi_{i}")).collect();
            indent(out, depth);
            out.push_str(&format!(
                "let ({}) = {};\n",
                nomes.join(", "),
                emit_delimited_exp(call, ctx)
            ));
            nomes
        }
        TypedMultiValues::List(exps) => {
            let mut nomes = Vec::with_capacity(exps.len());
            for (i, exp) in exps.iter().enumerate() {
                let nome = format!("titan_multi_{i}");
                indent(out, depth);
                out.push_str(&format!(
                    "let {nome} = {};\n",
                    emit_slot_value(slots[i], exp, ctx)
                ));
                nomes.push(nome);
            }
            nomes
        }
    }
}

/// Gera a expressão Rust equivalente a `exp` em **posição de operando**:
/// binop/unop saem entre parênteses (`(lhs op rhs)`, `(-e)`, `(!e)`) para a
/// precedência do Titan ficar explícita em qualquer aninhamento, sem depender
/// de coincidir com a do Rust (PRD T14). Em posição que a sintaxe já delimita
/// (condição, valor de `let`/atribuição/`return`, argumento), use
/// [`emit_delimited_exp`].
fn emit_exp(exp: &TypedExp, ctx: Ctx) -> String {
    match &exp.kind {
        // `nil` num destino opcional (T69) é `None`, não `()`: o checker
        // (`widen_to_option`) deixa o literal intacto e só troca o `ty` para
        // o `Option`, justamente para a decisão cair aqui. O `nil` do tipo
        // `nil` — o retorno vazio, o valor de um `Decl` sem tipo opcional —
        // continua `()` como sempre foi.
        TypedExpKind::Nil if matches!(exp.ty, Type::Option { .. }) => "None".to_string(),
        TypedExpKind::Nil => "()".to_string(),
        TypedExpKind::Bool(v) => v.to_string(),
        TypedExpKind::Integer(v) => v.to_string(),
        TypedExpKind::Float(v) => format_float_literal(*v),
        TypedExpKind::String(v) => format_string_literal(v),
        TypedExpKind::Var(name) => name.clone(),
        TypedExpKind::Concat(exps) => emit_concat(exps, ctx),
        TypedExpKind::Call { callee, args } => emit_call(callee, args, ctx),
        // `^` vira chamada de método (`.powf`), que já se delimita sozinha —
        // não precisa dos parênteses externos em nenhuma posição.
        TypedExpKind::Binop {
            op: BinOp::Pow,
            lhs,
            rhs,
        } => emit_pow(lhs, rhs, ctx),
        TypedExpKind::Binop { op, lhs, rhs } => {
            format!("({})", emit_binop(*op, lhs, rhs, &exp.ty, ctx))
        }
        TypedExpKind::Unop { op, exp: operand } => format!("({})", emit_unop(*op, operand, ctx)),
        TypedExpKind::Index { base, index } => emit_index(base, index, ctx),
        TypedExpKind::Field { base, name } => emit_field(base, name, ctx),
        TypedExpKind::ArrayLit(elems) => emit_array_lit(elems, ctx),
        TypedExpKind::RecordLit { type_name, fields } => emit_record_lit(type_name, fields, ctx),
        TypedExpKind::MapLit(entries) => emit_map_lit(entries, ctx),
        // Retorno múltiplo (T65): a chamada devolve uma tupla Rust e o
        // ajuste é um acesso posicional. A T66 fechou o par — a tupla que
        // esse `.0`/`.n` indexa é a que `rust_rettype_name` declara na
        // assinatura e o braço `Return` monta —, então o Rust emitido aqui
        // compila.
        TypedExpKind::Adjust(inner) => format!("{}.0", emit_exp(inner, ctx)),
        TypedExpKind::Extra { exp: inner, index } => {
            format!("{}.{index}", emit_exp(inner, ctx))
        }
        // Injeção `T → T?` (T69): o `SomeOf` que o checker planta no ponto
        // exato em que o destino declara `T?` vira o `Some(...)` do Rust.
        // O valor de dentro passa por [`emit_slot_value`] com o **tipo
        // base**, e não por `emit_exp` cru, porque `Some(...)` é um slot
        // como qualquer outro: `local s: string? = t` precisa de
        // `Some(t.clone())`, senão o `t` de fora sairia movido.
        TypedExpKind::SomeOf(inner) => {
            format!("Some({})", emit_slot_value(&inner.ty, inner, ctx))
        }
    }
}

/// Expressão em posição que a sintaxe do Rust já delimita — condição de
/// `if`/`while`, valor de `let`/atribuição/`return`, argumento de chamada.
/// Binop/unop saem **sem** os parênteses externos: o lint `unused_parens` do
/// rustc reclama deles exatamente nessas posições, e o Rust gerado deve
/// compilar sem warnings. Os operandos aninhados seguem parentesizados via
/// [`emit_exp`], então a precedência continua explícita.
fn emit_delimited_exp(exp: &TypedExp, ctx: Ctx) -> String {
    match &exp.kind {
        TypedExpKind::Binop { op: BinOp::Pow, .. } => emit_exp(exp, ctx),
        TypedExpKind::Binop { op, lhs, rhs } => emit_binop(*op, lhs, rhs, &exp.ty, ctx),
        TypedExpKind::Unop { op, exp: operand } => emit_unop(*op, operand, ctx),
        _ => emit_exp(exp, ctx),
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
fn emit_owned_string(exp: &TypedExp, ctx: Ctx) -> String {
    match &exp.kind {
        TypedExpKind::String(_) => format!("{}.to_string()", emit_exp(exp, ctx)),
        TypedExpKind::Var(_) => format!("{}.clone()", emit_exp(exp, ctx)),
        _ => emit_delimited_exp(exp, ctx),
    }
}

/// Regra de clone centralizada (decisão 1 da Fase 2, PRD.md T30): um slot de
/// tipo composto (`array`/`map`/`record`) ou `string` só precisa de
/// `.clone()` quando a expressão-fonte é algo que **outra variável ainda
/// enxerga** depois — `Var` (`local b = a`), `Index` (`local x = xs[i]`) ou
/// `Field` (`local x = p.campo`). Literais, chamadas e construtores
/// (`ArrayLit`/`RecordLit`/`MapLit`, chamada de função) já são donos do valor
/// que produzem — cloná-los seria trabalho supérfluo (e nem compila para os
/// braços que retornam algo diferente de `TypedExp`, como `Concat`).
fn precisa_clone(exp: &TypedExp) -> bool {
    matches!(
        &exp.kind,
        TypedExpKind::Var(_) | TypedExpKind::Index { .. } | TypedExpKind::Field { .. }
    )
}

/// Valor emitido para um "slot" — como [`emit_owned_string`], mas para
/// qualquer tipo: aplica a regra de `string` quando `slot_ty` é `String`,
/// [`precisa_clone`] quando é composto, e delega para [`emit_delimited_exp`]
/// no resto (primitivas não-`string` nunca precisam de clone).
fn emit_slot_value(slot_ty: &Type, value: &TypedExp, ctx: Ctx) -> String {
    if *slot_ty == Type::String {
        emit_owned_string(value, ctx)
    } else if is_composite(slot_ty) && precisa_clone(value) {
        format!("{}.clone()", emit_exp(value, ctx))
    } else {
        emit_delimited_exp(value, ctx)
    }
}

/// `true` para os tipos que este backend passa por `&mut` em posição de
/// parâmetro (T30) — mesmo critério usado pelo checker (`is_composite` em
/// `checker.rs`) para decidir se um uso é mutável. `Opaque` entra na T42
/// (decisão 8 do PRD.md): o receptor de `df.soma(...)` herda de graça a
/// mesma máquina de lugares (`&mut` em parâmetro, `clone()` na atribuição,
/// `emit_place_mut`/`emit_place_expr`).
fn is_composite(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Array { .. } | Type::Map { .. } | Type::Record { .. } | Type::Opaque { .. }
    )
}

/// Empresta uma expressão composta (`array`/`map`/`record`) para uma posição
/// de **leitura** que pede `&T` — `array_get`/`map_get`/`array_len`,
/// argumento de chamada quando o parâmetro só é lido (nesta fase, todo
/// argumento composto é lido, nunca só emprestado a `&`, mas a distinção de
/// [`emit_place_mut`] só importa para escrita). `array_get`/`map_get`
/// devolvem um valor **clonado** — para leitura isso basta, então `base`
/// pode ser qualquer expressão (inclusive outro `Index`/`Field`) sem
/// precisar resolver um lugar de verdade. Só o `Var` que nomeia diretamente
/// um **parâmetro** desta função (presente em `ctx`) já é a própria
/// referência (`rust_param_type_name`: `&mut T`); reemprestá-lo (`&x`)
/// duplicaria a referência.
fn borrow_composite(exp: &TypedExp, ctx: Ctx) -> String {
    match &exp.kind {
        TypedExpKind::Var(name) if ctx.contains(name) => name.clone(),
        _ => format!("&{}", emit_exp(exp, ctx)),
    }
}

/// Produz uma expressão Rust cujo **tipo já é `&mut T`** para uma posição de
/// escrita através de um composto — `array_set`/`map_set`/escrita de campo
/// (`v[i] = x`, `p.campo = x`) e argumento de chamada Titan (T30, decisão 4:
/// todo parâmetro composto é `&mut`). Ao contrário de [`borrow_composite`],
/// aqui **importa** que `exp` resolva a um lugar de verdade, não a um valor
/// clonado — `array_get`/`map_get` devolvem por valor, então
/// `&mut array_get(...)` seria uma referência a um temporário que morre no
/// fim da expressão, descartando a escrita silenciosamente (bug corrigido no
/// T30: `xs[1][1] = 9`, `m["a"][1] = 9`, `pontos[1].x = 9`,
/// `f(mat[1])` com `mat: {{integer}}`). Por isso a recursão troca para os
/// `_mut` do runtime (`array_get_mut`/`map_get_mut`, que devolvem `&mut T`
/// de verdade) sempre que `base` é ele mesmo um composto indexado/aninhado.
fn emit_place_mut(exp: &TypedExp, ctx: Ctx) -> String {
    if let TypedExpKind::Var(name) = &exp.kind
        && ctx.contains(name)
    {
        // Parâmetro composto: o nome cru já é `&mut T` — devolvê-lo direto
        // (em vez de `&mut *nome`) evita um reborrow textual que o rustc
        // não consegue provar disjunto de outro empréstimo do mesmo
        // parâmetro dentro da mesma chamada (`array_set(&mut *xs, 1,
        // array_get(xs, 2))` falha com "borrowed as mutable" mesmo sendo
        // semanticamente sequencial; `array_set(xs, 1, array_get(xs, 2))`
        // não tem esse problema).
        return name.clone();
    }
    format!("&mut {}", emit_place_expr(exp, ctx))
}

/// Expressão-lugar (sem `&mut` externo) usada tanto por [`emit_place_mut`]
/// quanto, recursivamente, por si mesma — `&mut base.campo`/
/// `&mut array_get_mut(...)` só ficam corretos se `base`/o índice interno
/// forem construídos por esta função, nunca por [`emit_place_mut`] direto:
/// `&mut {emit_place_mut(base)}.campo` grudaria o `&mut` já existente de
/// `base` com o acesso de campo (`&mut (&mut p).x`, que nem compila) em vez
/// de produzir `&mut p.x` (uma única referência, ao campo).
fn emit_place_expr(exp: &TypedExp, ctx: Ctx) -> String {
    match &exp.kind {
        // Parâmetro composto: o nome cru já é `&mut T` — para virar um
        // *lugar*, precisa do deref explícito (`*xs`), senão `&mut *xs`
        // (via `emit_place_mut`) duplicaria a referência. Parentetizado:
        // usado como `base` de `Field` (`{}.{name}` abaixo), `*xs.campo`
        // sem parênteses parsearia como `*(xs.campo)` (`.` tem precedência
        // maior que `*` prefixo em Rust) — o mesmo bug corrigido no braço
        // `Index` logo abaixo, só que aqui é alcançável mesmo sem `Index`
        // na cadeia (`f(xs: {Ponto})` com `xs.x = 9` no corpo).
        TypedExpKind::Var(name) if ctx.contains(name) => format!("(*{name})"),
        // Local dona (array/map/record): o próprio nome já é o lugar.
        TypedExpKind::Var(_) => emit_exp(exp, ctx),
        // `p.campo` onde `campo` é composto: o lugar de `base` seguido do
        // acesso — nunca via `emit_place_mut(base)` (que já embutiria um
        // `&mut` no meio da cadeia).
        TypedExpKind::Field { base, name } => {
            format!("{}.{name}", emit_place_expr(base, ctx))
        }
        // `v[i]` onde o elemento é composto: troca para a variante `_mut`
        // do runtime, que devolve `&mut T` de verdade em vez do valor
        // clonado de `array_get`/`map_get` — a chamada em si já é uma
        // referência, então o lugar correspondente é o seu deref (`*..`),
        // simétrico ao caso do parâmetro acima. Parentetizado pelo mesmo
        // motivo: usado como `base` de outro `Field` (`caixas[1].itens`),
        // `*array_get_mut(..).itens` sem parênteses desreferenciaria o
        // campo (`Vec<T>` → `[T]`) em vez do resultado da chamada
        // (`Caixa` → `.itens`), quebrando o tipo esperado por
        // `array_set`/parâmetro de função.
        TypedExpKind::Index { base, index } => {
            let call = match &base.ty {
                Type::Array { .. } => format!(
                    "titan_runtime::array_get_mut({}, {})",
                    emit_place_mut(base, ctx),
                    emit_delimited_exp(index, ctx)
                ),
                Type::Map { .. } => format!(
                    "titan_runtime::map_get_mut({}, &{})",
                    emit_place_mut(base, ctx),
                    emit_slot_value(&index.ty, index, ctx)
                ),
                other => unreachable!(
                    "checker só produz `Index` sobre array/map, encontrado {other:?}"
                ),
            };
            format!("(*{call})")
        }
        other => unreachable!(
            "checker só produz composto endereçável a partir de Var/Field/Index, encontrado {other:?}"
        ),
    }
}

/// Operador Rust equivalente a um [`BinOp`] do Titan. Quatro variantes não
/// têm símbolo direto e viram chamada: `Pow` ([`emit_pow`] — o `^` do Rust é XOR, não
/// potência), `IDiv` ([`emit_idiv`] — o `/` do Rust trunca, o `//` do Titan
/// arredonda para baixo) e `Shl`/`Shr` ([`emit_shift`] — o Rust transborda
/// fora de `0..64`, o Titan aceita qualquer deslocamento).
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
        BinOp::BAnd => "&",
        BinOp::BOr => "|",
        // Atenção ao cruzamento: `~` binário no Titan é XOR, que no Rust é
        // `^` — e o `^` do Titan é potência, que vira `.powf` em `emit_pow`.
        BinOp::BXor => "^",
        BinOp::Shl => unreachable!("`<<` é emitido como chamada em emit_shift"),
        BinOp::Shr => unreachable!("`>>` é emitido como chamada em emit_shift"),
        BinOp::Pow => unreachable!("`^` é emitido como chamada a powf em emit_pow"),
        BinOp::IDiv => unreachable!("`//` é emitido como chamada em emit_idiv"),
    }
}

/// Corpo de um operador binário, sem os parênteses externos — quem chama
/// decide se eles são necessários ([`emit_exp`]) ou proibidos pelo lint
/// ([`emit_delimited_exp`]).
fn emit_binop(op: BinOp, lhs: &TypedExp, rhs: &TypedExp, result_ty: &Type, ctx: Ctx) -> String {
    // Os três que viram chamada saem antes de [`binop_symbol`]: não têm
    // símbolo equivalente no Rust, e consultar a tabela cedo bateria no seu
    // `unreachable!`.
    match op {
        BinOp::IDiv => return emit_idiv(lhs, rhs, result_ty, ctx),
        BinOp::Shl | BinOp::Shr => return emit_shift(op, lhs, rhs, ctx),
        _ => {}
    }
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
            emit_numeric_operand(lhs, result_ty, ctx),
            emit_numeric_operand(rhs, result_ty, ctx)
        ),
        BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => {
            emit_comparison(symbol, lhs, rhs, ctx)
        }
        // Boolean estrito dos dois lados (decisão 7 da Fase 1) — mapeamento
        // direto para os operadores de curto-circuito do Rust.
        BinOp::And | BinOp::Or => {
            format!("{} {symbol} {}", emit_exp(lhs, ctx), emit_exp(rhs, ctx))
        }
        // Bitwise sem deslocamento: o checker (T61) já garantiu `Integer`
        // dos dois lados e resultado `Integer`, então nenhum cast entra
        // aqui — mapeamento direto para os operadores do Rust sobre `i64`.
        BinOp::BAnd | BinOp::BOr | BinOp::BXor => {
            format!("{} {symbol} {}", emit_exp(lhs, ctx), emit_exp(rhs, ctx))
        }
        BinOp::IDiv => unreachable!("`//` já saiu por emit_idiv acima"),
        BinOp::Shl | BinOp::Shr => unreachable!("shifts já saíram por emit_shift acima"),
        BinOp::Pow => unreachable!("`^` é emitido como chamada a powf em emit_pow"),
    }
}

/// `//` — divisão com piso, **não** o `/` do Rust (PRD.md, T61).
///
/// Para inteiros, o `/` do Rust trunca em direção a zero (`-7 / 2 == -3`) e o
/// Titan/Lua arredonda para baixo (`-7 // 2 == -4`): a emissão delega a
/// `titan_runtime::idiv`, que faz o piso e ainda trata divisão por zero com
/// mensagem em português. `div_euclid` **não** serve: para divisor negativo
/// ele mantém o resto não-negativo em vez de arredondar para baixo
/// (`(-7).div_euclid(-2)` é 4, e `-7 // -2` no Titan é 3).
///
/// Para float o piso é `.floor()` sobre a divisão comum, como em
/// `coder.lua:1750-1767`.
fn emit_idiv(lhs: &TypedExp, rhs: &TypedExp, result_ty: &Type, ctx: Ctx) -> String {
    if *result_ty == Type::Float {
        return format!(
            "({} / {}).floor()",
            emit_numeric_operand(lhs, result_ty, ctx),
            emit_numeric_operand(rhs, result_ty, ctx)
        );
    }
    format!(
        "titan_runtime::idiv({}, {})",
        emit_delimited_exp(lhs, ctx),
        emit_delimited_exp(rhs, ctx)
    )
}

/// `<<` e `>>` — deslocamento com a semântica do Titan, **não** a do Rust
/// (PRD.md, T61).
///
/// O `<<` do Rust exige `0 <= b < 64` e transborda fora disso; quando o
/// rustc consegue provar o transbordo, ele **recusa a compilação** — e a
/// mensagem chegaria em inglês, sobre código que o usuário não escreveu
/// (`1 << 64` é o caso mínimo). No Titan/Lua o deslocamento é um inteiro
/// qualquer: negativo inverte a direção, 64 ou mais zera
/// (`coder.lua:1670-1710`). A conta inteira mora no runtime, em
/// `titan_runtime::shl`/`shr`.
fn emit_shift(op: BinOp, lhs: &TypedExp, rhs: &TypedExp, ctx: Ctx) -> String {
    let func = match op {
        BinOp::Shl => "shl",
        BinOp::Shr => "shr",
        other => unreachable!("emit_shift só trata `<<`/`>>`, recebeu {other:?}"),
    };
    format!(
        "titan_runtime::{func}({}, {})",
        emit_delimited_exp(lhs, ctx),
        emit_delimited_exp(rhs, ctx)
    )
}

/// Comparações — o checker (T13) já validou as combinações: número com
/// número (com coerção int→float quando os lados divergem), string com
/// string, e boolean com boolean (só `==`/`~=`).
fn emit_comparison(symbol: &str, lhs: &TypedExp, rhs: &TypedExp, ctx: Ctx) -> String {
    // Teste de presença (T69): `x ~= nil` / `nil ~= x` sobre um opcional é
    // `x.is_some()`, e `==` é `x.is_none()`. O mapeamento direto sairia
    // `x != ()` — nem compila, porque `Option<T>` não se compara com `()`.
    // O checker (T68) só deixa um opcional chegar a uma comparação quando o
    // outro lado é `nil`, então basta olhar de que lado está o opcional.
    if let Some(rendered) = emit_presence_test(symbol, lhs, rhs, ctx) {
        return rendered;
    }
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
            emit_numeric_operand(lhs, &target, ctx),
            emit_numeric_operand(rhs, &target, ctx)
        );
    }
    if lhs.ty == Type::String {
        // `String` não implementa `PartialOrd`/`PartialEq` cruzado com `&str`
        // no std — os dois lados precisam nascer como `String` mesmo em
        // `==`/`~=`, daí reusar [`emit_owned_string`] em vez de `emit_exp`.
        return format!(
            "{} {symbol} {}",
            emit_owned_string(lhs, ctx),
            emit_owned_string(rhs, ctx)
        );
    }
    // Igualdade de boolean: `bool == bool` direto.
    format!("{} {symbol} {}", emit_exp(lhs, ctx), emit_exp(rhs, ctx))
}

/// `x ~= nil` → `x.is_some()`, `x == nil` → `x.is_none()` (T69), nas duas
/// ordens dos operandos.
///
/// Devolve `None` quando a comparação não envolve opcional — aí
/// [`emit_comparison`] segue pelo caminho de sempre. O lado `nil` não é
/// emitido: em Rust o teste é um método sobre o próprio opcional, e emitir
/// o `None` do outro lado (`x != None`) exigiria `PartialEq` e anotação de
/// tipo que `is_some()`/`is_none()` dispensam.
fn emit_presence_test(symbol: &str, lhs: &TypedExp, rhs: &TypedExp, ctx: Ctx) -> Option<String> {
    let metodo = match symbol {
        "!=" => "is_some",
        "==" => "is_none",
        _ => return None,
    };
    let opcional = match (&lhs.ty, &rhs.ty) {
        (Type::Option { .. }, Type::Nil) => lhs,
        (Type::Nil, Type::Option { .. }) => rhs,
        _ => return None,
    };
    Some(format!("{}.{metodo}()", emit_exp(opcional, ctx)))
}

/// Operando numérico já validado pelo checker: `Integer` em posição cujo
/// resultado é `Float` ganha `(x as f64)` (PRD T14).
fn emit_numeric_operand(exp: &TypedExp, result_ty: &Type, ctx: Ctx) -> String {
    let rendered = emit_exp(exp, ctx);
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
fn emit_pow(lhs: &TypedExp, rhs: &TypedExp, ctx: Ctx) -> String {
    format!(
        "({} as f64).powf({} as f64)",
        emit_exp(lhs, ctx),
        emit_exp(rhs, ctx)
    )
}

/// Corpo de um operador unário, sem os parênteses externos — mesma divisão
/// de responsabilidade de [`emit_binop`]. `#` (T30) despacha para
/// `array_len`/`string_len` do runtime conforme o tipo do operando —
/// `check_unop` já rejeitou `#` sobre `map`/`record` com erro claro.
fn emit_unop(op: UnOp, operand: &TypedExp, ctx: Ctx) -> String {
    match op {
        UnOp::Neg => format!("-{}", emit_exp(operand, ctx)),
        UnOp::Not => format!("!{}", emit_exp(operand, ctx)),
        // `~` unário do Titan é bitwise NOT, e o `!` do Rust é o mesmo
        // operador de `not` — só que sobre `i64` em vez de `bool`. O checker
        // (T61) já garantiu que o operando é `Integer`.
        UnOp::BNot => format!("!{}", emit_exp(operand, ctx)),
        UnOp::Len => match &operand.ty {
            Type::Array { .. } => {
                format!(
                    "titan_runtime::array_len({})",
                    borrow_composite(operand, ctx)
                )
            }
            Type::String => {
                format!(
                    "titan_runtime::string_len(&{})",
                    emit_owned_string(operand, ctx)
                )
            }
            other => unreachable!("checker só produz `#` sobre array/string, encontrado {other:?}"),
        },
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
fn emit_concat(exps: &[TypedExp], ctx: Ctx) -> String {
    let mut parts = exps.iter();
    let first = parts
        .next()
        .expect("checker garante ExpConcat com ao menos um operando");
    // `acc` guarda sempre um `String` **cru** (sem `&`) — o que `concat`
    // devolve. Cada chamada empresta `acc` na hora de alimentar a próxima
    // (`&{acc}`); a última iteração deixa o resultado sem `&`, pronto para
    // ser usado como slot (`let`/atribuição/`return`) ou por
    // [`borrow_runtime_str`], que sabe emprestar qualquer expressão.
    let mut acc = borrow_runtime_str(first, ctx);
    let mut acc_is_raw = false;
    for e in parts {
        let lhs = if acc_is_raw {
            format!("&{acc}")
        } else {
            acc
        };
        acc = format!(
            "titan_runtime::concat({lhs}, {})",
            borrow_runtime_str(e, ctx)
        );
        acc_is_raw = true;
    }
    acc
}

/// Argumentos de uma chamada a função **Titan** (T24: parâmetros de tipo
/// `string` são sempre `String` dona — sem `&str`, sem alocação implícita
/// escondida do chamador). Reusa [`emit_owned_string`] para strings;
/// argumento composto (T30, decisão 4: todo parâmetro composto é `&mut`)
/// sai por [`emit_place_mut`] — cobre tanto o caso simples (`f(v)`, `v`
/// local ou parâmetro) quanto o aninhado (`f(mat[1])`, onde só um lugar de
/// verdade — nunca o valor clonado que `array_get` devolveria — faz a
/// mutação de `f` alcançar `mat`). O resto segue a posição delimitada
/// normal.
fn emit_call(callee: &Callee, args: &[TypedExp], ctx: Ctx) -> String {
    match callee {
        Callee::Direct(name) => {
            if let Some(builtin) = crate::builtins::lookup(name) {
                let rendered_args = emit_args_by_param(args, builtin.params, ctx);
                return format!("{}({})", builtin.rust_path, rendered_args.join(", "));
            }
            let rendered_args: Vec<String> = args
                .iter()
                .map(|a| {
                    if a.ty == Type::String {
                        emit_owned_string(a, ctx)
                    } else if is_composite(&a.ty) {
                        emit_place_mut(a, ctx)
                    } else {
                        emit_delimited_exp(a, ctx)
                    }
                })
                .collect();
            format!("{}({})", mangle_fn_name(name), rendered_args.join(", "))
        }
        // `data.read_csv(...)` (T39): chamada de função de módulo — sem
        // receptor, argumentos por posição contra a assinatura da
        // capability (mesma ABI por-parâmetro do builtin/função Titan).
        Callee::Module { module, name } => {
            let capability = crate::capabilities::lookup_module(module)
                .expect("checker só produz Callee::Module para módulo importado existente");
            let function = capability
                .find_function(name)
                .expect("checker só produz Callee::Module para função existente na capability");
            let rendered_args = emit_args_by_param(args, function.params, ctx);
            format!("{}({})", function.rust_path, rendered_args.join(", "))
        }
        // `df.soma(...)` (T40): método sobre tipo opaco — o receptor entra
        // como primeiro argumento posicional da função Rust do runtime, por
        // `emit_place_mut` (T42: `Opaque` já é `is_composite`, reusa a
        // mesma máquina de lugares da Fase 2 em vez de um caminho à parte).
        Callee::Method { recv, module, name } => {
            let capability = crate::capabilities::lookup_module(module)
                .expect("checker só produz Callee::Method para módulo importado existente");
            let Type::Opaque {
                name: receiver_type,
                ..
            } = &recv.ty
            else {
                unreachable!("checker só produz Callee::Method com receptor de tipo Opaque")
            };
            let method = capability
                .find_method(receiver_type, name)
                .expect("checker só produz Callee::Method para método existente na capability");
            let rendered_recv = emit_place_mut(recv, ctx);
            let rendered_args = emit_args_by_param(args, method.params, ctx);
            let mut all_args = vec![rendered_recv];
            all_args.extend(rendered_args);
            format!("{}({})", method.rust_path, all_args.join(", "))
        }
    }
}

/// Renderiza os argumentos de uma chamada **contra a assinatura declarada**
/// (`params`, por posição) — generaliza a ABI que antes só o caminho de
/// função Titan seguia (risco 3 do PRD.md, T42): builtin/módulo/método
/// passavam *todos* os argumentos por [`borrow_runtime_str`], correto só
/// enquanto `print` (que recebe `&str`) era o único caso. `String` empresta
/// (`&str`, molde do runtime); composto sai por [`emit_place_mut`] (T42:
/// `Opaque` incluso via [`is_composite`]); o resto é a posição delimitada
/// normal.
fn emit_args_by_param(args: &[TypedExp], params: &[Type], ctx: Ctx) -> Vec<String> {
    args.iter()
        .zip(params)
        .map(|(a, p)| match p {
            Type::String => borrow_runtime_str(a, ctx),
            p if is_composite(p) => emit_place_mut(a, ctx),
            _ => emit_delimited_exp(a, ctx),
        })
        .collect()
}

/// `v[i]` em posição de leitura (T30): `array_get`/`map_get` do runtime —
/// checagem de faixa/chave em português, nunca o panic cru do Rust (decisão 3
/// da Fase 2). Ambos já devolvem um valor **dono** (clonado dentro do
/// runtime, `array_get_checked`/`map_get_checked`), então o resultado não
/// precisa de `.clone()` extra aqui — é o próprio `base`/`index` que talvez
/// precisem (ex.: `index` sendo outra variável composta, caso raro mas
/// coberto pela mesma regra dos slots). Por ser leitura, `base` pode ser
/// qualquer expressão — inclusive outro `Index`/`Field` aninhado — sem
/// precisar de um lugar de verdade: [`borrow_composite`] só empresta o
/// resultado, nunca escreve nele.
fn emit_index(base: &TypedExp, index: &TypedExp, ctx: Ctx) -> String {
    match &base.ty {
        Type::Array { .. } => format!(
            "titan_runtime::array_get({}, {})",
            borrow_composite(base, ctx),
            emit_delimited_exp(index, ctx)
        ),
        Type::Map { .. } => format!(
            "titan_runtime::map_get({}, &{})",
            borrow_composite(base, ctx),
            emit_slot_value(&index.ty, index, ctx)
        ),
        other => unreachable!("checker só produz `Index` sobre array/map, encontrado {other:?}"),
    }
}

/// `p.campo` em posição de leitura (T30): acesso direto de campo — o
/// `struct` gerado tem todos os campos `pub`. Sem `.clone()` aqui: quem
/// decide se este valor precisa de cópia é a posição que o consome
/// ([`emit_slot_value`]/[`precisa_clone`]), não a leitura do campo em si.
fn emit_field(base: &TypedExp, name: &str, ctx: Ctx) -> String {
    format!("{}.{name}", emit_exp(base, ctx))
}

/// `{1, 2, 3}` como array (T30): `vec![..]`. Cada elemento passa pela mesma
/// regra de slot do tipo do array — `emit_array_lit` não tem acesso direto ao
/// `elem_ty` resolvido, mas cada `TypedExp` já carrega seu próprio `ty`
/// (o checker garantiu compatibilidade elemento a elemento), então usar
/// `elem.ty` no lugar do tipo do array é equivalente e evita replicar o
/// `Box<Type>` aqui.
fn emit_array_lit(elems: &[TypedExp], ctx: Ctx) -> String {
    let rendered: Vec<String> = elems
        .iter()
        .map(|e| emit_slot_value(&e.ty, e, ctx))
        .collect();
    format!("vec![{}]", rendered.join(", "))
}

/// `Nome{x = 1, y = 2}` como record (T30): `Nome { x: .., y: .. }` — o
/// checker já entrega `fields` na ordem canônica da declaração do record
/// (`check_record_lit`), então a emissão não precisa reordenar nada.
fn emit_record_lit(type_name: &str, fields: &[(String, TypedExp)], ctx: Ctx) -> String {
    let rendered: Vec<String> = fields
        .iter()
        .map(|(name, value)| format!("{name}: {}", emit_slot_value(&value.ty, value, ctx)))
        .collect();
    format!("{type_name} {{ {} }}", rendered.join(", "))
}

/// `{["a"] = 1}` como map (T30): `HashMap::from([(k, v), ..])`.
fn emit_map_lit(entries: &[(TypedExp, TypedExp)], ctx: Ctx) -> String {
    let rendered: Vec<String> = entries
        .iter()
        .map(|(k, v)| {
            format!(
                "({}, {})",
                emit_slot_value(&k.ty, k, ctx),
                emit_slot_value(&v.ty, v, ctx)
            )
        })
        .collect();
    format!("std::collections::HashMap::from([{}])", rendered.join(", "))
}

/// Coage uma expressão numérica ou `string` para `&str`/referência esperada
/// pelo `titan-runtime` (`print(&str)`, `concat(&str, &str)`) — a única
/// fronteira que ainda pede empréstimo em vez de posse (T24: dentro do
/// programa gerado, `string` é sempre `String`). Número vira
/// `&x.to_string()` (decisão 4 da Fase 1); string usa [`emit_owned_string`]
/// e empresta o resultado.
fn borrow_runtime_str(exp: &TypedExp, ctx: Ctx) -> String {
    if matches!(exp.ty, Type::Integer | Type::Float) {
        format!("&{}.to_string()", emit_exp(exp, ctx))
    } else {
        format!("&{}", emit_owned_string(exp, ctx))
    }
}

/// Tipo Rust de uma variável/expressão, em qualquer posição: `string` é
/// sempre `String` (T24 — zero casos especiais por posição). `Array`/`Map`
/// são genéricos (T30: `Vec<T>`/`HashMap<K, V>`, recursivo no elemento/
/// chave/valor); `Record` vira o nome do `struct` gerado por
/// [`emit_record_struct`], sem mangling (ADR 0009). `Value`/`Function`/
/// `Option`/`Invalid` nunca chegam aqui: `resolve_type` (`checker.rs`) já
/// rejeita essas anotações com erro claro antes da passada 2.
fn rust_type_name(ty: &Type) -> String {
    match ty {
        Type::Nil => "()".to_string(),
        Type::Boolean => "bool".to_string(),
        Type::Integer => "i64".to_string(),
        Type::Float => "f64".to_string(),
        Type::String => "String".to_string(),
        Type::Array { elem } => format!("Vec<{}>", rust_type_name(elem)),
        Type::Map { keys, values } => {
            format!(
                "std::collections::HashMap<{}, {}>",
                rust_type_name(keys),
                rust_type_name(values)
            )
        }
        Type::Record { name, .. } => name.clone(),
        // `T?` (T69) → `Option<T>`. O braço que a T68 deixou faltando: até
        // aqui um tipo opcional caía no `unreachable!` abaixo, e por isso
        // `generate` tinha de recusar o programa inteiro antes de emitir
        // qualquer coisa. O `base` passa por esta mesma função, então
        // composto dentro de opcional sai `Option<Vec<i64>>` e segue o ADR
        // 0006/0007 como qualquer outro composto.
        Type::Option { base } => format!("Option<{}>", rust_type_name(base)),
        // Tipo opaco de capability (T42): o caminho Rust totalmente
        // qualificado que o checker já resolveu via `requalify_rettype`
        // (`titan_data::DataFrame`), nunca o `name` Titan cru.
        Type::Opaque { rust_path, .. } => rust_path.clone(),
        other => unreachable!(
            "tipo '{other:?}' fora do subconjunto de codegen suportado — checker deveria ter rejeitado antes"
        ),
    }
}

/// Tipo Rust da **lista de retornos** de uma função (T66). `None` quer dizer
/// "sem `->` na assinatura": ou a lista é vazia, ou é o único retorno `nil`
/// — os dois são `()` em Rust, e escrever `-> ()` seria ruído que o lint
/// `unused_unit` do rustc ainda por cima reclamaria.
///
/// Com N>1 retornos a assinatura vira uma **tupla** — `-> (i64, i64)` — e
/// esse é o único ponto do backend que monta a tupla: [`rust_type_name`] não
/// muda, cada componente passa por ela sozinho. Um retorno só continua
/// exatamente como antes da T66, sem tupla de um elemento: `-> i64`, e não
/// `-> (i64,)`.
///
/// Composto dentro da tupla segue por **valor** (`Vec<i64>`, não
/// `&mut Vec<i64>`): o `&mut` do ADR 0007 é regra de **parâmetro**, e
/// devolver uma referência daria um valor emprestado de algo que morre com a
/// função. Quem recebe fica dono, como no retorno composto único que já
/// existia — por isso aqui é [`rust_type_name`] e nunca
/// [`rust_param_type_name`].
fn rust_rettype_name(rettypes: &[Type]) -> Option<String> {
    match rettypes {
        [] => None,
        [Type::Nil] => None,
        [único] => Some(rust_type_name(único)),
        vários => Some(format!(
            "({})",
            vários
                .iter()
                .map(rust_type_name)
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// Tipo Rust de um **parâmetro** de função: idêntico a [`rust_type_name`],
/// exceto os tipos compostos (T30, decisão 4 da Fase 2) — `array`, `map` e
/// `record` — que saem por `&mut T` em vez de por valor, preservando o
/// idioma in-place da referência (`selection_sort`, PRD.md).
fn rust_param_type_name(ty: &Type) -> String {
    if is_composite(ty) {
        format!("&mut {}", rust_type_name(ty))
    } else {
        rust_type_name(ty)
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
        generate(&typed.program)
            .unwrap_or_else(|e| panic!("erro de geração de código inesperado: {e}"))
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

    // ---- T15/T62: StatFor emitido como `loop` com incremento no topo -----

    #[test]
    fn for_emite_loop_com_incremento_no_topo() {
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
        assert!(rust.contains("let mut titan_for_primeira: bool = true;"));
        assert!(rust.contains("loop {"));
        assert!(rust.contains("if titan_for_primeira {"));
        assert!(rust.contains("titan_for_primeira = false;"));
        assert!(rust.contains("i += titan_for_inc;"));
        assert!(rust.contains("if !((titan_for_asc && i <= titan_for_finish)"));
        assert!(rust.contains("|| (!titan_for_asc && i >= titan_for_finish)) {"));
        // O `while` do template antigo (ADR 0004) não sobra em lugar nenhum.
        assert!(!rust.contains("while (titan_for_asc"));
        // Nunca o Range do Rust (`.step_by` não cobre passo negativo/float).
        assert!(!rust.contains(".."));
        assert!(!rust.contains("step_by"));
    }

    /// O incremento precisa vir **antes** do corpo no texto emitido: é o que
    /// faz um `continue` do usuário (T63) passar por ele em vez de pulá-lo.
    #[test]
    fn for_poe_o_incremento_antes_do_corpo_no_texto_emitido() {
        let source = r#"function main(args: {string}): integer
    for i = 1, 5 do
        print("corpo")
    end
    return 0
end"#;
        let rust = generate_source(source);
        let incremento = rust
            .find("i += titan_for_inc;")
            .expect("incremento emitido");
        let corpo = rust.find("corpo").expect("corpo emitido");
        assert!(
            incremento < corpo,
            "incremento deve preceder o corpo:\n{rust}"
        );
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
        assert!(rust.contains("let mut titan_for_primeira: bool = true;"));
        assert!(rust.contains("x += titan_for_inc;"));
        assert!(rust.contains("if !((titan_for_asc && x <= titan_for_finish)"));
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

    // ---- T30: arrays, records, maps --------------------------------------

    #[test]
    fn record_gera_struct_com_derive_clone_e_campos_pub() {
        let source = r#"record Ponto
    x: integer
    y: integer
end

function main(args: {string}): integer
    local p: Ponto = {x = 1, y = 2}
    return p.x
end"#;
        let rust = generate_source(source);
        assert!(rust.contains("#[derive(Clone, Debug, PartialEq)]\npub struct Ponto {"));
        assert!(rust.contains("pub x: i64,"));
        assert!(rust.contains("pub y: i64,"));
        // Sem mangling no nome do tipo (ADR 0009).
        assert!(rust.contains("let p: Ponto = Ponto { x: 1, y: 2 };"));
    }

    #[test]
    fn array_literal_indexacao_e_escrita_usam_runtime_checado() {
        let source = r#"function main(args: {string}): integer
    local xs: {integer} = {10, 20, 30}
    xs[1] = 99
    print("x: " .. xs[1])
    print("len: " .. #xs)
    return 0
end"#;
        let rust = generate_source(source);
        assert!(rust.contains("let mut xs: Vec<i64> = vec![10, 20, 30];"));
        assert!(rust.contains("let titan_idx = 1;"));
        assert!(rust.contains("let titan_val = 99;"));
        assert!(rust.contains("titan_runtime::array_set(&mut xs, titan_idx, titan_val);"));
        assert!(rust.contains("titan_runtime::array_get(&xs, 1)"));
        assert!(rust.contains("titan_runtime::array_len(&xs)"));
    }

    #[test]
    fn map_literal_consulta_e_escrita_usam_runtime() {
        let source = r#"function main(args: {string}): integer
    local m: {string: integer} = {["a"] = 1}
    m["b"] = 2
    print("a: " .. m["a"])
    return 0
end"#;
        let rust = generate_source(source);
        assert!(rust.contains(
            "let mut m: std::collections::HashMap<String, i64> = std::collections::HashMap::from([(\"a\".to_string(), 1)]);"
        ));
        assert!(rust.contains("let titan_key = \"b\".to_string();"));
        assert!(rust.contains("let titan_val = 2;"));
        assert!(rust.contains("titan_runtime::map_set(&mut m, titan_key, titan_val);"));
        assert!(rust.contains("titan_runtime::map_get(&m, &\"a\".to_string())"));
    }

    #[test]
    fn parametro_composto_sai_por_mut_e_reusa_referencia_no_corpo() {
        let source = r#"function dobra_primeiro(xs: {integer}): nil
    xs[1] = xs[1] * 2
end

function main(args: {string}): integer
    local v: {integer} = {1, 2}
    dobra_primeiro(v)
    return v[1]
end"#;
        let rust = generate_source(source);
        assert!(rust.contains("pub fn titan_dobra_primeiro(xs: &mut Vec<i64>)"));
        // Dentro do corpo, `xs` já é a referência — sem `&mut xs` duplicado.
        assert!(rust.contains("titan_runtime::array_set(xs, titan_idx, titan_val);"));
        assert!(rust.contains("titan_runtime::array_get(xs, 1)"));
        // No chamador, `v` é uma local dona — precisa do empréstimo.
        assert!(rust.contains("titan_dobra_primeiro(&mut v);"));
    }

    /// Prova a decisão 1 (semântica de valor): `local b = a; b[1] = 9` não
    /// deve alterar `a` — `b` nasce de um `.clone()` explícito.
    #[test]
    fn atribuicao_de_array_clona_e_preserva_original() {
        let source = r#"function main(args: {string}): integer
    local a: {integer} = {1, 2, 3}
    local b: {integer} = a
    b[1] = 999
    return a[1]
end"#;
        let rust = generate_source(source);
        assert!(rust.contains("let mut b: Vec<i64> = a.clone();"));
        let (avisos, output) = compila_e_executa(&rust, "t30-clone-array");
        assert!(avisos.is_empty(), "warnings:\n{avisos}\n{rust}");
        assert_eq!(output.status.code(), Some(1));
    }

    /// Critério de aceite do T30 (PRD.md): array criado/indexado/escrito/`#`,
    /// record construído/campo lido/escrito, map criado/consultado, função
    /// que ordena um array **in-place** (decisão 4) e `local b = a; b[1] = 9`
    /// não altera `a` (decisão 1) — tudo com `rustc` de verdade, sem
    /// warnings.
    #[test]
    fn t30_compila_e_roda_arrays_records_maps_sem_warnings() {
        let source = r#"record Ponto
    x: integer
    y: integer
end

function ordena_dois(xs: {integer}): nil
    if xs[1] > xs[2] then
        local tmp: integer = xs[1]
        xs[1] = xs[2]
        xs[2] = tmp
    end
end

function main(args: {string}): integer
    local original: {integer} = {5, 1, 3}
    local copia: {integer} = original
    copia[1] = 999
    print("original: " .. original[1])
    print("copia: " .. copia[1])

    local par: {integer} = {5, 1}
    ordena_dois(par)
    print("par1: " .. par[1])
    print("par2: " .. par[2])

    local p: Ponto = {x = 1, y = 2}
    p.x = 10
    print("p.x: " .. p.x)
    print("p.y: " .. p.y)

    local m: {string: integer} = {["a"] = 1}
    m["b"] = 2
    print("m.a: " .. m["a"])
    print("m.b: " .. m["b"])

    print("len: " .. #par)
    print("slen: " .. #"abcde")

    return 0
end"#;
        let rust = generate_source(source);
        let (avisos, output) = compila_e_executa(&rust, "t30-completo");

        assert!(
            avisos.is_empty(),
            "warnings no Rust gerado:\n{avisos}\n{rust}"
        );

        let esperado = "original: 5\n\
                        copia: 999\n\
                        par1: 1\n\
                        par2: 5\n\
                        p.x: 10\n\
                        p.y: 2\n\
                        m.a: 1\n\
                        m.b: 2\n\
                        len: 2\n\
                        slen: 5\n";
        assert_eq!(String::from_utf8_lossy(&output.stdout), esperado);
        assert_eq!(output.status.code(), Some(0));
    }

    /// Regressão: escrita através de um composto **aninhado** (a base de
    /// `Index`/`Field`/argumento é ela mesma um `Index`) não pode passar
    /// pelo `array_get`/`map_get` que clonam — `&mut array_get(...)`
    /// emprestaria um temporário e a escrita se perderia silenciosamente,
    /// sem erro de compilação nem panic. Cobre os quatro casos que
    /// `emit_place_mut`/`emit_place_expr` existem para resolver: array de
    /// array, map de array, array de record (escrita de campo via índice) e
    /// elemento indexado composto passado como argumento `&mut`.
    #[test]
    fn escrita_atraves_de_composto_aninhado_alcanca_a_raiz() {
        let source = r#"record Ponto
    x: integer
    y: integer
end

function dobra_primeiro(xs: {integer}): nil
    xs[1] = xs[1] * 2
end

function main(args: {string}): integer
    local mat: {{integer}} = {{1, 2}, {3, 4}}
    mat[1][1] = 99
    print("mat11: " .. mat[1][1])

    local mm: {string: {integer}} = {["a"] = {1, 2}}
    mm["a"][1] = 77
    print("mma1: " .. mm["a"][1])

    local pontos: {Ponto} = {{x = 1, y = 2}, {x = 3, y = 4}}
    pontos[1].x = 55
    print("p1x: " .. pontos[1].x)

    dobra_primeiro(mat[2])
    print("mat21: " .. mat[2][1])

    return 0
end"#;
        let rust = generate_source(source);
        let (avisos, output) = compila_e_executa(&rust, "t30-aninhado");

        assert!(
            avisos.is_empty(),
            "warnings no Rust gerado:\n{avisos}\n{rust}"
        );

        let esperado = "mat11: 99\n\
                        mma1: 77\n\
                        p1x: 55\n\
                        mat21: 6\n";
        assert_eq!(String::from_utf8_lossy(&output.stdout), esperado);
        assert_eq!(output.status.code(), Some(0));
    }

    /// Regressão: `emit_place_expr` do braço `Index` produzia `*chamada`
    /// sem parênteses — correto isolado (`*array_get_mut(..)`), mas quebrado
    /// assim que usado como `base` de um `Field` (`caixas[1].itens`), porque
    /// `.` tem precedência maior que `*` prefixo em Rust:
    /// `*array_get_mut(&mut caixas, 1).itens` desreferenciava o **campo**
    /// (`Vec<i64>` → `[i64]`, incompatível com `array_set`/parâmetro de
    /// função) em vez do resultado da chamada (`Caixa` → `.itens`). Cobre
    /// também o caso simétrico do parâmetro (`Var` em `ctx`): escrever num
    /// campo de um parâmetro composto (`p.x = ..` com `p: Ponto`) e passar
    /// o campo array de um elemento indexado como argumento `&mut` de outra
    /// função.
    #[test]
    fn campo_de_elemento_indexado_e_campo_de_parametro_resolvem_lugar_correto() {
        let source = r#"record Ponto
    x: integer
    y: integer
end

record Caixa
    itens: {integer}
end

function move(p: Ponto): nil
    p.x = p.x + 1
end

function dobra_primeiro(xs: {integer}): nil
    xs[1] = xs[1] * 2
end

function main(args: {string}): integer
    local caixas: {Caixa} = {{itens = {1, 2}}, {itens = {3, 4}}}
    caixas[1].itens[1] = 100
    print("c1i1: " .. caixas[1].itens[1])

    dobra_primeiro(caixas[2].itens)
    print("c2i1: " .. caixas[2].itens[1])

    local p: Ponto = {x = 1, y = 2}
    move(p)
    print("p.x: " .. p.x)

    return 0
end"#;
        let rust = generate_source(source);
        let (avisos, output) = compila_e_executa(&rust, "t30-campo-de-indexado");

        assert!(
            avisos.is_empty(),
            "warnings no Rust gerado:\n{avisos}\n{rust}"
        );

        let esperado = "c1i1: 100\nc2i1: 6\np.x: 2\n";
        assert_eq!(String::from_utf8_lossy(&output.stdout), esperado);
        assert_eq!(output.status.code(), Some(0));
    }

    // ---- T42: emissão de chamada qualificada e de método ------------------
    //
    // Estes testes checam só o Rust **emitido** (`generate_source`), não a
    // compilação real — `compila_e_executa` linka apenas `titan-runtime`, e
    // `titan_data::*` puxaria `polars` (custo de build alto, PRD.md T41) só
    // para provar texto que já é conferível estaticamente.

    #[test]
    fn chamada_de_modulo_emite_caminho_qualificado_do_runtime() {
        let source = r#"import data

function main(args: {string}): integer
    local df: data.DataFrame = data.read_csv("v.csv")
    return 0
end"#;
        let rust = generate_source(source);

        assert!(
            rust.contains(r#"titan_data::read_csv(&"v.csv".to_string())"#),
            "esperava chamada qualificada de módulo no Rust gerado:\n{rust}"
        );
        // `Opaque` mapeado para o `rust_path` da capability, não o nome
        // Titan cru (`rust_type_name`, T42).
        assert!(
            rust.contains("let df: titan_data::DataFrame = titan_data::read_csv"),
            "esperava tipo opaco mapeado para titan_data::DataFrame:\n{rust}"
        );
    }

    #[test]
    fn chamada_de_metodo_emite_receptor_por_referencia_mutavel() {
        let source = r#"import data

function main(args: {string}): integer
    local df: data.DataFrame = data.read_csv("v.csv")
    local total: float = df.soma("valor")
    return 0
end"#;
        let rust = generate_source(source);

        // Receptor (`df`, variável local dona de um `Opaque`) por
        // `emit_place_mut` — `Opaque` é composto (T42), então o lugar é
        // `&mut df` (não `df` cru: só parâmetro composto já é `&mut T`).
        assert!(
            rust.contains(r#"titan_data::soma(&mut df, &"valor".to_string())"#),
            "esperava método emitido com receptor por &mut e argumento string:\n{rust}"
        );
    }

    #[test]
    fn variavel_de_tipo_opaco_como_parametro_e_mut_por_referencia() {
        // `Opaque` entra em `is_composite` (T42, decisão 8 do PRD.md):
        // receber um `data.DataFrame` como parâmetro de função Titan segue
        // a mesma ABI `&mut T` de array/map/record (`rust_param_type_name`).
        let source = r#"import data

function processa(df: data.DataFrame): nil
    local total: float = df.soma("valor")
end

function main(args: {string}): integer
    local df: data.DataFrame = data.read_csv("v.csv")
    processa(df)
    return 0
end"#;
        let rust = generate_source(source);

        assert!(
            rust.contains("fn titan_processa(df: &mut titan_data::DataFrame)"),
            "esperava parâmetro opaco por &mut titan_data::DataFrame:\n{rust}"
        );
        // Argumento de chamada Titan (não builtin/módulo/método) para
        // parâmetro composto sai por `emit_place_mut` também — `df` é
        // variável local **dona** (não um parâmetro já `&mut T`), então o
        // lugar é `&mut df` (mesma regra de qualquer array/map/record local
        // passado a outra função, T30).
        assert!(
            rust.contains("titan_processa(&mut df)"),
            "esperava argumento de chamada Titan emprestado por &mut df:\n{rust}"
        );
    }

    /// A ABI de argumentos por-parâmetro (risco 3 do PRD.md, T42): antes da
    /// generalização, `emit_call` passava *todos* os argumentos de builtin
    /// por `borrow_runtime_str`, o que só estava correto porque `print`
    /// (único builtin) recebe exclusivamente `string`. Prova diretamente em
    /// [`emit_args_by_param`] — sem depender de a stdlib ganhar um builtin
    /// de assinatura mista de verdade — que um `integer` na assinatura sai
    /// pela posição delimitada normal (`42`), não por `&42.to_string()`
    /// (que só é correto para os builtins de hoje, todos `&str`).
    #[test]
    fn emit_args_by_param_usa_a_posicao_certa_por_tipo_do_parametro() {
        let loc = crate::ast::Loc { line: 0, col: 0 };
        let ctx: HashSet<String> = HashSet::new();
        let args = vec![
            TypedExp {
                loc,
                ty: Type::Integer,
                kind: TypedExpKind::Integer(42),
            },
            TypedExp {
                loc,
                ty: Type::String,
                kind: TypedExpKind::String("oi".to_string()),
            },
        ];
        let params = [Type::Integer, Type::String];

        let rendered = emit_args_by_param(&args, &params, &ctx);

        assert_eq!(rendered, vec!["42", r#"&"oi".to_string()"#]);
    }

    // ---- T61: bitwise e `//` -------------------------------------------

    /// Os critérios de aceite da T61 em execução real: `7 // 2` → 3,
    /// **`-7 // 2` → -4** (piso, não truncagem), `5 & 3` → 1, `5 | 3` → 7,
    /// `5 ~ 3` → 6 (XOR), `1 << 10` → 1024, `~0` → -1. Um programa só,
    /// compilado com o rustc de verdade e conferido por stdout. Os
    /// parênteses em volta dos bitwise não são decoração: na cascata do
    /// Titan (T60, fiel a `parser.lua:369-395`) `..` liga **mais forte**
    /// que `&`/`|`/`~`/`<<`/`>>`, então `"and=" .. 5 & 3` seria
    /// `("and=" .. 5) & 3` — string num operando bitwise, erro de tipo.
    #[test]
    fn t61_bitwise_e_divisao_inteira_compilam_e_rodam_sem_warnings() {
        let source = r#"function main(args: {string}): integer
    print("7//2=" .. 7 // 2)
    print("-7//2=" .. -7 // 2)
    print("7//-2=" .. 7 // -2)
    print("-7//-2=" .. -7 // -2)
    print("7.0//2=" .. 7.0 // 2)
    print("-7.0//2=" .. -7.0 // 2)
    print("and=" .. (5 & 3))
    print("or=" .. (5 | 3))
    print("xor=" .. (5 ~ 3))
    print("shl=" .. (1 << 10))
    print("shr=" .. (1024 >> 10))
    print("not=" .. ~0)
    return 0
end"#;
        let rust = generate_source(source);

        let (avisos, output) = compila_e_executa(&rust, "t61_bitwise");
        assert!(
            avisos.is_empty(),
            "warnings no Rust gerado:\n{avisos}\n{rust}"
        );
        let esperado = concat!(
            "7//2=3\n",
            // O critério que separa `//` do `/` do Rust, que daria -3.
            "-7//2=-4\n",
            "7//-2=-4\n",
            "-7//-2=3\n",
            "7.0//2=3\n",
            "-7.0//2=-4\n",
            "and=1\n",
            "or=7\n",
            "xor=6\n",
            "shl=1024\n",
            "shr=1\n",
            "not=-1\n",
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout), esperado);
        assert_eq!(output.status.code(), Some(0));
    }

    /// Deslocamento com quantidade fora de `0..64` é legal no Titan (zera) e
    /// **overflow** no Rust — quando o rustc consegue provar, recusa a
    /// compilação com mensagem em inglês. Este teste é a prova de que o
    /// pipeline não deixa isso chegar ao rustc: a conta sai pelo runtime, e
    /// o programa roda.
    #[test]
    fn deslocamento_fora_da_faixa_roda_em_vez_de_quebrar_o_rustc() {
        let source = r#"function main(args: {string}): integer
    local n: integer = 64
    local m: integer = -10
    print("shl64=" .. (1 << n))
    print("shr64=" .. (1 >> n))
    print("negativo=" .. (1024 << m))
    print("logico=" .. (-1 >> 63))
    return 0
end"#;
        let rust = generate_source(source);
        assert!(
            rust.contains("titan_runtime::shl(1, n)"),
            "shift deveria sair pelo runtime:\n{rust}"
        );

        let (avisos, output) = compila_e_executa(&rust, "t61_shift_faixa");
        assert!(
            avisos.is_empty(),
            "warnings no Rust gerado:\n{avisos}\n{rust}"
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "shl64=0\nshr64=0\nnegativo=1\nlogico=1\n"
        );
        assert_eq!(output.status.code(), Some(0));
    }

    /// `//` inteiro sai por `titan_runtime::idiv`, nunca pelo `/` cru do
    /// Rust; `//` float sai por `.floor()` sobre a divisão comum.
    #[test]
    fn divisao_inteira_emite_idiv_do_runtime_e_floor_para_float() {
        let source = r#"function main(args: {string}): integer
    local a: integer = 7 // 2
    local b: float = 7.0 // 2.0
    return 0
end"#;
        let rust = generate_source(source);
        assert!(
            rust.contains("let a: i64 = titan_runtime::idiv(7, 2);"),
            "esperava chamada a idiv, obteve:\n{rust}"
        );
        assert!(
            rust.contains("let b: f64 = (7.0 / 2.0).floor();"),
            "esperava floor para float, obteve:\n{rust}"
        );
    }

    /// O cruzamento de símbolos que a T61 precisa acertar: Titan `~`
    /// binário (XOR) vira `^` do Rust, Titan `^` (potência) vira `.powf`, e
    /// Titan `~` unário (NOT) vira `!`.
    #[test]
    fn til_e_circunflexo_nao_se_confundem_na_emissao() {
        let source = r#"function main(args: {string}): integer
    local xor: integer = 5 ~ 3
    local pot: float = 5.0 ^ 3.0
    local nao: integer = ~0
    return 0
end"#;
        let rust = generate_source(source);
        assert!(rust.contains("let xor: i64 = 5 ^ 3;"), "{rust}");
        assert!(
            rust.contains("let pot: f64 = (5.0 as f64).powf(3.0 as f64);"),
            "{rust}"
        );
        assert!(rust.contains("let nao: i64 = !0;"), "{rust}");
    }

    /// Bitwise em posição de operando continua parentetizado — a
    /// precedência do Titan (`|` mais frouxo que `&`, que é mais frouxo que
    /// os shifts) fica explícita no Rust gerado, sem depender de coincidir
    /// com a do Rust.
    #[test]
    fn precedencia_de_bitwise_sai_explicita_em_parenteses() {
        let source = r#"function main(args: {string}): integer
    local a: integer = 1 | 2 & 3
    local b: integer = 1 << 2 + 3
    return 0
end"#;
        let rust = generate_source(source);
        assert!(rust.contains("let a: i64 = 1 | (2 & 3);"), "{rust}");
        // O shift vira chamada, e a precedência aparece no argumento: o
        // `+` já foi agrupado pelo parser antes de virar operando.
        assert!(
            rust.contains("let b: i64 = titan_runtime::shl(1, 2 + 3);"),
            "{rust}"
        );
    }

    /// T63: `continue` do Titan emite literalmente `continue;` do Rust, sem
    /// label e sem laço auxiliar — e, dentro do `for`, o `continue;` aparece
    /// **depois** do incremento no texto, que é o que garante que voltar ao
    /// topo avance a variável de controle (ADR 0022).
    #[test]
    fn continue_emite_continue_do_rust_depois_do_incremento() {
        let source = r#"function main(args: {string}): integer
    for i = 1, 5 do
        if i == 2 then
            continue
        end
        print("corpo")
    end
    return 0
end"#;
        let rust = generate_source(source);
        assert!(rust.contains("continue;"), "{rust}");
        let incremento = rust
            .find("i += titan_for_inc;")
            .expect("incremento emitido");
        let cont = rust.find("continue;").expect("continue emitido");
        assert!(incremento < cont, "incremento deve preceder o continue:\n{rust}");
    }

    /// No `while` o `continue;` também sai sem label: o `while` do Rust
    /// reavalia a condição ao voltar ao topo, igual ao do Titan.
    #[test]
    fn continue_em_while_emite_continue_sem_label() {
        let source = r#"function main(args: {string}): integer
    local i: integer = 0
    while i < 5 do
        i = i + 1
        continue
    end
    return 0
end"#;
        let rust = generate_source(source);
        assert!(rust.contains("continue;"), "{rust}");
        assert!(!rust.contains("'titan"), "sem label:\n{rust}");
    }

    /// T64: `repeat corpo until cond` vira `loop { corpo; if cond { break; }
    /// }` — o `loop` do Rust é o único laço sem teste no topo, que é
    /// exatamente a semântica do `repeat`.
    #[test]
    fn repeat_emite_loop_com_teste_no_fim() {
        let source = r#"function main(args: {string}): integer
    local n: integer = 0
    repeat
        n = n + 1
    until n > 3
    return 0
end"#;
        let rust = generate_source(source);
        assert!(rust.contains("loop {"), "{rust}");
        // O teste sai **depois** do corpo, e não antes: é isso que faz o
        // laço rodar ao menos uma vez.
        let corpo = rust.find("n = n + 1;").expect("corpo emitido");
        let teste = rust.find("if n > 3 {").expect("teste do until emitido");
        assert!(corpo < teste, "corpo deve preceder o teste:\n{rust}");
        assert!(rust.contains("break;"), "{rust}");
        // Sem `while` nem flag de primeira iteração: o `loop` já dá isso de
        // graça, diferente do `for` (ADR 0022).
        assert!(!rust.contains("titan_for_primeira"), "{rust}");
    }

    /// Corpo e condição saem dentro das **mesmas** chaves, e não em blocos
    /// separados: um `local` do corpo referenciado pelo `until` precisa
    /// estar em escopo no Rust gerado, ou o `rustc` rejeitaria o programa.
    #[test]
    fn local_do_corpo_fica_visivel_para_o_teste_do_until() {
        let source = r#"function main(args: {string}): integer
    local n: integer = 0
    repeat
        local x: integer = n
        n = n + 1
    until x > 3
    return 0
end"#;
        let rust = generate_source(source);
        let decl = rust.find("let x: i64 = n;").expect("local emitido");
        let teste = rust.find("if x > 3 {").expect("teste do until emitido");
        assert!(decl < teste, "declaração deve preceder o teste:\n{rust}");
        // Mesma indentação = mesmo bloco: se o corpo saísse num `{ ... }`
        // próprio, o teste do `until` estaria um nível acima e `x` teria
        // saído de escopo no Rust gerado.
        assert!(
            rust.contains("        let x: i64 = n;") && rust.contains("        if x > 3 {"),
            "corpo e teste em níveis distintos:\n{rust}"
        );
    }

    /// `break` e `continue` do usuário dentro de `repeat` saem sem label,
    /// como em qualquer outro laço (ADR 0023) — o `loop` do Rust os aceita
    /// sem caso especial.
    #[test]
    fn break_e_continue_dentro_de_repeat_saem_sem_label() {
        let source = r#"function main(args: {string}): integer
    local n: integer = 0
    repeat
        n = n + 1
        if n == 1 then
            continue
        end
        break
    until false
    return 0
end"#;
        let rust = generate_source(source);
        assert!(rust.contains("continue;"), "{rust}");
        assert!(!rust.contains("'titan"), "sem label:\n{rust}");
    }

    /// Critério de aceite da T66 no nível do texto emitido: a assinatura com
    /// dois retornos vira `-> (i64, i64)` e o `return a, b` vira uma tupla
    /// só. Os dois lados têm que casar — é justamente o par que a T65 deixou
    /// aberto, com o `.0` do ajuste apontando para uma tupla que ninguém
    /// produzia.
    #[test]
    fn dois_retornos_viram_tupla_na_assinatura_e_no_return() {
        let source = r#"function divmod(a: integer, b: integer): integer, integer
    return a // b, a % b
end
function main(args: {string}): integer
    return 0
end"#;
        let rust = generate_source(source);
        assert!(
            rust.contains("pub fn titan_divmod(a: i64, b: i64) -> (i64, i64) {"),
            "assinatura sem tupla:\n{rust}"
        );
        assert!(
            rust.contains("return (titan_runtime::idiv(a, b), a % b);"),
            "return sem tupla:\n{rust}"
        );
    }

    /// O outro lado da regra: **um** retorno continua exatamente como antes
    /// da T66. Rust tem tupla de um elemento (`(i64,)`), e emiti-la aqui
    /// seria uma mudança silenciosa em todo programa já existente — todo o
    /// resto da suíte de codegen depende de `-> i64` e `return 0;` crus.
    #[test]
    fn um_retorno_so_nao_vira_tupla_de_um() {
        let source = r#"function dobro(a: integer): integer
    return a * 2
end
function main(args: {string}): integer
    return 0
end"#;
        let rust = generate_source(source);
        assert!(
            rust.contains("pub fn titan_dobro(a: i64) -> i64 {"),
            "{rust}"
        );
        assert!(rust.contains("return a * 2;"), "{rust}");
        assert!(!rust.contains("(i64,)"), "tupla de um elemento:\n{rust}");
    }

    /// `nil` continua `()`: sem `->` na assinatura (escrevê-lo faria o lint
    /// `unused_unit` do rustc reclamar) e `return ();` no corpo, o mesmo de
    /// antes da T66.
    #[test]
    fn retorno_nil_continua_sem_seta_na_assinatura() {
        let source = r#"function nada(a: integer): nil
    print("x" .. a)
    return nil
end
function main(args: {string}): integer
    return 0
end"#;
        let rust = generate_source(source);
        assert!(rust.contains("pub fn titan_nada(a: i64) {"), "{rust}");
        assert!(!rust.contains("-> ()"), "seta para unit:\n{rust}");
        assert!(rust.contains("return ();"), "{rust}");
    }

    /// Composto dentro da tupla segue as regras do ADR 0006/0007: por
    /// **valor** na assinatura (`Vec<i64>`, nunca `&mut Vec<i64>` — o `&mut`
    /// é regra de parâmetro, e devolver referência a um local não
    /// compilaria) e com `.clone()` no componente cuja fonte é um lugar que
    /// sobrevive ao `return`. `string` na tupla sai dona, pela mesma regra
    /// de slot do retorno único (T24).
    #[test]
    fn composto_e_string_na_tupla_seguem_as_regras_de_slot() {
        let source = r#"function par(): {integer}, string
    local v: {integer} = {1, 2}
    local s: string = "a"
    return v, s
end
function main(args: {string}): integer
    return 0
end"#;
        let rust = generate_source(source);
        assert!(
            rust.contains("pub fn titan_par() -> (Vec<i64>, String) {"),
            "composto na tupla deveria sair por valor:\n{rust}"
        );
        assert!(
            !rust.contains("&mut Vec<i64>)"),
            "`&mut` é regra de parâmetro, não de retorno:\n{rust}"
        );
        assert!(
            rust.contains("return (v.clone(), s.clone());"),
            "componentes sem a regra de clone:\n{rust}"
        );
    }

    /// O canto do ADR 0007 dentro da tupla: devolver um **parâmetro**
    /// composto. No corpo, `xs` é `&mut Vec<i64>`; como componente de um
    /// retorno por valor ele precisa virar dono, e é `precisa_clone` (ADR
    /// 0006) que já cuida disso — sem o `.clone()`, o rustc recusaria
    /// devolver um `&mut` emprestado como `Vec<i64>`.
    #[test]
    fn parametro_composto_devolvido_na_tupla_vira_dono() {
        let source = r#"function eco(xs: {integer}): {integer}, integer
    return xs, #xs
end
function main(args: {string}): integer
    return 0
end"#;
        let rust = generate_source(source);
        assert!(
            rust.contains("pub fn titan_eco(xs: &mut Vec<i64>) -> (Vec<i64>, i64) {"),
            "parâmetro por `&mut`, retorno por valor:\n{rust}"
        );
        assert!(
            rust.contains("return (xs.clone(), titan_runtime::array_len(xs));"),
            "parâmetro composto na tupla sem `.clone()`:\n{rust}"
        );
    }

    /// `nil` **dentro** da tupla é o único ponto onde os dois braços de
    /// `rust_rettype_name` se encostam: `: nil` sozinho apaga a seta da
    /// assinatura, mas `: integer, nil` é uma lista de dois, e o `nil` vira
    /// um componente `()` como qualquer outro tipo. O rustc aceita `(i64,
    /// ())` sem reclamar — é `-> ()` sozinho que dispararia `unused_unit`.
    #[test]
    fn nil_dentro_da_tupla_vira_componente_unit() {
        let source = r#"function f(): integer, nil
    return 1, nil
end
function main(args: {string}): integer
    return 0
end"#;
        let rust = generate_source(source);
        assert!(rust.contains("pub fn titan_f() -> (i64, ()) {"), "{rust}");
        assert!(rust.contains("return (1, ());"), "{rust}");
    }

    /// O ajuste da T65 (`Adjust`) e o enésimo valor (`Extra`) indexam a
    /// tupla que a T66 passou a produzir. `Extra` ainda não tem sintaxe de
    /// fonte — só a multi-atribuição da T67 o produzirá —, então o nó é
    /// trocado à mão sobre a árvore já tipada, o mesmo recurso que o teste
    /// de `ExpExtra` no checker (T65) usa.
    #[test]
    fn ajuste_e_extra_indexam_a_tupla_do_retorno() {
        let source = r#"function divmod(a: integer, b: integer): integer, integer
    return a // b, a % b
end
function main(args: {string}): integer
    local q: integer = divmod(7, 2)
    return q
end"#;
        let tokens = lex(source).expect("erro léxico inesperado");
        let program = parse(&tokens).expect("erro sintático inesperado");
        let mut typed = check(&program).expect("erro de tipo inesperado");

        let rust = generate(&typed.program).expect("erro de geração inesperado");
        assert!(
            rust.contains("let q: i64 = titan_divmod(7, 2).0;"),
            "ajuste não indexou a tupla:\n{rust}"
        );

        // Troca o `Adjust` do inicializador de `q` por um `Extra` de índice
        // 1: o segundo valor de retorno, o que a T67 vai desestruturar.
        let TypedTopLevel::Func { body, .. } = &mut typed.program[1] else {
            panic!("esperava `main` como segunda declaração");
        };
        let TypedStat::Block { stats, .. } = body.as_mut() else {
            panic!("esperava bloco no corpo de `main`");
        };
        let TypedStat::Decl { value, .. } = &mut stats[0] else {
            panic!("esperava a declaração de `q` como primeiro comando");
        };
        let TypedExpKind::Adjust(chamada) = &value.kind else {
            panic!("esperava `Adjust`, obteve {:?}", value.kind);
        };
        value.kind = TypedExpKind::Extra {
            exp: chamada.clone(),
            index: 1,
        };

        let rust = generate(&typed.program).expect("erro de geração inesperado");
        assert!(
            rust.contains("let q: i64 = titan_divmod(7, 2).1;"),
            "`Extra` não indexou o segundo valor da tupla:\n{rust}"
        );
    }
    // ---- T67: multi-assign e declaração múltipla -----------------------

    /// A armadilha central da tarefa, no texto emitido: `a, b = b, a`
    /// precisa avaliar **todo** o lado direito antes de escrever em
    /// qualquer alvo. Emitir `a = b; b = a;` — a tradução ingênua — daria
    /// `a == b`, um bug silencioso que nenhuma checagem de tipo pegaria.
    #[test]
    fn swap_passa_por_temporarios_antes_de_qualquer_escrita() {
        let rust = generate_source(
            "function main(args: {string}): integer\n\
             \x20   local a: integer = 1\n\
             \x20   local b: integer = 2\n\
             \x20   a, b = b, a\n\
             \x20   return 0\n\
             end",
        );
        let esperado = "let titan_multi_0 = b;\n    \
                        let titan_multi_1 = a;\n    \
                        a = titan_multi_0;\n    \
                        b = titan_multi_1;";
        assert!(
            rust.contains(esperado),
            "o swap não passou por temporários:\n{rust}"
        );
    }

    /// A desestruturação da tupla da T66 sai num `let` de padrão só, e cada
    /// alvo recebe o seu componente.
    #[test]
    fn declaracao_multipla_desestrutura_a_tupla_num_let_so() {
        let rust = generate_source(
            "function divmod(a: integer, b: integer): integer, integer\n\
             \x20   return a // b, a % b\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   local q, r = divmod(7, 2)\n\
             \x20   return q + r\n\
             end",
        );
        assert!(
            rust.contains("let (titan_multi_0, titan_multi_1) = titan_divmod(7, 2);"),
            "tupla não desestruturada:\n{rust}"
        );
        assert!(rust.contains("let q: i64 = titan_multi_0;"), "{rust}");
        assert!(rust.contains("let r: i64 = titan_multi_1;"), "{rust}");
    }

    /// Só o alvo reatribuído sai `let mut` — o fix-up marca cada alvo por
    /// si, e `unused_mut` no Rust gerado seria warning.
    #[test]
    fn so_o_alvo_reatribuido_sai_mutavel() {
        let rust = generate_source(
            "function main(args: {string}): integer\n\
             \x20   local a: integer, b: integer = 1, 2\n\
             \x20   a = a + b\n\
             \x20   return a\n\
             end",
        );
        assert!(rust.contains("let mut a: i64 = titan_multi_0;"), "{rust}");
        assert!(rust.contains("let b: i64 = titan_multi_1;"), "{rust}");
    }

    /// Alvos compostos entram pelo mesmo caminho do single-target:
    /// `array_set` para `v[i]` e escrita direta para `p.campo` — com os
    /// valores já nos temporários, de modo que o swap também vale ali.
    #[test]
    fn alvos_compostos_da_atribuicao_multipla_usam_o_caminho_de_sempre() {
        let rust = generate_source(
            "record Ponto\n\
             \x20   x: integer\n\
             \x20   y: integer\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   local v: {integer} = {10, 20}\n\
             \x20   v[1], v[2] = v[2], v[1]\n\
             \x20   local p: Ponto = {x = 1, y = 2}\n\
             \x20   p.x, p.y = p.y, p.x\n\
             \x20   return 0\n\
             end",
        );
        assert!(
            rust.contains("let titan_val = titan_multi_0;")
                && rust.contains("titan_runtime::array_set(&mut v, titan_idx, titan_val);"),
            "`v[i]` não passou por array_set:\n{rust}"
        );
        assert!(
            rust.contains("(&mut p).x = titan_multi_0;")
                && rust.contains("(&mut p).y = titan_multi_1;"),
            "`p.campo` não recebeu dos temporários:\n{rust}"
        );
    }

    /// `string` e composto desestruturados de uma chamada chegam **donos**
    /// aos alvos: a tupla devolvida já é dona dos componentes, então nada
    /// de `.clone()` extra na desestruturação.
    #[test]
    fn desestruturacao_de_string_e_composto_nao_reclona() {
        let rust = generate_source(
            "function rotula(n: integer): string, {integer}\n\
             \x20   return \"rot\", {n, n + 1}\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   local s, w = rotula(5)\n\
             \x20   return #w\n\
             end",
        );
        assert!(
            rust.contains("let (titan_multi_0, titan_multi_1) = titan_rotula(5);"),
            "{rust}"
        );
        assert!(rust.contains("let s: String = titan_multi_0;"), "{rust}");
        assert!(rust.contains("let w: Vec<i64> = titan_multi_1;"), "{rust}");
    }

    /// O critério de aceite da T67 em execução real, com o rustc de
    /// verdade: o swap troca, `divmod` dá 3 e 1, e nada disso gera warning.
    #[test]
    fn t67_multi_assign_compila_e_roda_sem_warnings() {
        let rust = generate_source(
            "function divmod(a: integer, b: integer): integer, integer\n\
             \x20   return a // b, a % b\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   local a: integer = 1\n\
             \x20   local b: integer = 2\n\
             \x20   a, b = b, a\n\
             \x20   print(\"swap-\" .. a .. \"-\" .. b)\n\
             \x20   local q, r = divmod(7, 2)\n\
             \x20   print(\"divmod-\" .. q .. \"-\" .. r)\n\
             \x20   local s: string = \"um\"\n\
             \x20   local t: string = \"dois\"\n\
             \x20   s, t = t, s\n\
             \x20   print(\"str-\" .. s .. \"-\" .. t)\n\
             \x20   local v: {integer} = {10, 20}\n\
             \x20   v[1], v[2] = v[2], v[1]\n\
             \x20   print(\"vetor-\" .. v[1] .. \"-\" .. v[2])\n\
             \x20   return 0\n\
             end",
        );

        let (avisos, output) = compila_e_executa(&rust, "multi_assign");
        assert!(
            avisos.is_empty(),
            "warnings no Rust gerado:\n{avisos}\n{rust}"
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "swap-2-1\ndivmod-3-1\nstr-dois-um\nvetor-20-10\n"
        );
        assert_eq!(output.status.code(), Some(0));
    }

    // ---- T69: `Option` no Rust gerado ---------------------------------

    #[test]
    fn t69_tipo_opcional_vira_option() {
        let rust = generate_source(
            "function main(args: {string}): integer\n\
             \x20   local x: integer? = nil\n\
             \x20   if x ~= nil then\n\
             \x20       print(\"\" .. x)\n\
             \x20   end\n\
             \x20   return 0\n\
             end",
        );
        assert!(rust.contains("let x: Option<i64> = None;"), "{rust}");
    }

    /// `nil` só vira `None` quando o destino é opcional — o `nil` do tipo
    /// `nil` continua `()`, que é o que a função sem retorno devolve.
    #[test]
    fn t69_valor_do_tipo_base_vira_some() {
        let rust = generate_source(
            "function main(args: {string}): integer\n\
             \x20   local x: integer? = 10\n\
             \x20   if x ~= nil then\n\
             \x20       print(\"\" .. x)\n\
             \x20   end\n\
             \x20   return 0\n\
             end",
        );
        assert!(rust.contains("let x: Option<i64> = Some(10);"), "{rust}");
    }

    /// `Some(...)` é um slot como outro qualquer: a `string` de dentro sai
    /// dona, senão o valor de fora sairia movido.
    #[test]
    fn t69_some_de_string_clona_o_valor_de_dentro() {
        let rust = generate_source(
            "function main(args: {string}): integer\n\
             \x20   local t: string = \"oi\"\n\
             \x20   local s: string? = t\n\
             \x20   if s ~= nil then\n\
             \x20       print(s)\n\
             \x20   end\n\
             \x20   print(t)\n\
             \x20   return 0\n\
             end",
        );
        assert!(
            rust.contains("let s: Option<String> = Some(t.clone());"),
            "{rust}"
        );
    }

    /// O teste de presença vira método sobre o opcional — `x != ()` nem
    /// compilaria.
    #[test]
    fn t69_teste_de_presenca_vira_is_some_e_is_none() {
        let rust = generate_source(
            "function main(args: {string}): integer\n\
             \x20   local x: integer? = 1\n\
             \x20   if x ~= nil then\n\
             \x20       print(\"\" .. x)\n\
             \x20   end\n\
             \x20   if x == nil then\n\
             \x20       print(\"vazio\")\n\
             \x20   end\n\
             \x20   return 0\n\
             end",
        );
        assert!(rust.contains("if x.is_some() {"), "{rust}");
        assert!(rust.contains("if x.is_none() {"), "{rust}");
        assert!(!rust.contains("x != ()"), "{rust}");
    }

    /// `nil ~= x` estreita igual — a ordem dos operandos não muda a emissão.
    #[test]
    fn t69_teste_de_presenca_com_nil_do_lado_esquerdo() {
        let rust = generate_source(
            "function main(args: {string}): integer\n\
             \x20   local x: integer? = 1\n\
             \x20   if nil ~= x then\n\
             \x20       print(\"\" .. x)\n\
             \x20   end\n\
             \x20   return 0\n\
             end",
        );
        assert!(rust.contains("if x.is_some() {"), "{rust}");
    }

    /// Ramo que só **lê** o nome estreitado: ligação simples, sem `mut`,
    /// sem alias e sem write-back.
    #[test]
    fn t69_ramo_que_so_le_abre_ligacao_simples() {
        let rust = generate_source(
            "function main(args: {string}): integer\n\
             \x20   local x: integer? = 1\n\
             \x20   if x ~= nil then\n\
             \x20       print(\"\" .. x)\n\
             \x20   end\n\
             \x20   return 0\n\
             end",
        );
        assert!(rust.contains("let x: i64 = x.clone().unwrap();"), "{rust}");
        assert!(!rust.contains("titan_opt_x"), "{rust}");
    }

    /// A armadilha que a T68 registrou: atribuir dentro do ramo estreitado
    /// precisa alcançar a variável de fora, não a ligação nova.
    #[test]
    fn t69_atribuicao_no_ramo_estreitado_volta_para_a_variavel_externa() {
        let rust = generate_source(
            "function main(args: {string}): integer\n\
             \x20   local x: integer? = 1\n\
             \x20   if x ~= nil then\n\
             \x20       x = x + 10\n\
             \x20   end\n\
             \x20   return 0\n\
             end",
        );
        assert!(
            rust.contains("let titan_opt_x: &mut Option<i64> = &mut x;"),
            "{rust}"
        );
        assert!(
            rust.contains("let mut x: i64 = titan_opt_x.clone().unwrap();"),
            "{rust}"
        );
        assert!(rust.contains("*titan_opt_x = Some(x);"), "{rust}");
    }

    /// Nome estreitado que o corpo nunca menciona não abre ligação nenhuma
    /// — um `let` não lido seria `unused_variables` no Rust gerado.
    #[test]
    fn t69_nome_estreitado_sem_uso_nao_abre_ligacao() {
        let rust = generate_source(
            "function main(args: {string}): integer\n\
             \x20   local x: integer? = 1\n\
             \x20   if x ~= nil then\n\
             \x20       print(\"presente\")\n\
             \x20   end\n\
             \x20   return 0\n\
             end",
        );
        assert!(!rust.contains("let x: i64"), "{rust}");
    }

    /// Composto dentro de `Option` segue o ADR 0006/0007: o `Vec`/`HashMap`
    /// entra em `Option<...>` e o retorno sai por valor.
    #[test]
    fn t69_composto_dentro_de_opcional() {
        let rust = generate_source(
            "function f(): {integer}?\n\
             \x20   return nil\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   local v: {integer}? = f()\n\
             \x20   if v ~= nil then\n\
             \x20       print(\"\" .. #v)\n\
             \x20   end\n\
             \x20   return 0\n\
             end",
        );
        assert!(
            rust.contains("pub fn titan_f() -> Option<Vec<i64>> {"),
            "{rust}"
        );
        assert!(
            rust.contains("let v: Vec<i64> = v.clone().unwrap();"),
            "{rust}"
        );
    }

    /// O critério de aceite da T69 em execução real: uma função que devolve
    /// `integer?`, um chamador que testa, e Rust gerado **sem warnings**.
    #[test]
    fn t69_opcional_compila_e_roda_sem_warnings() {
        let rust = generate_source(
            "function busca(v: {integer}, alvo: integer): integer?\n\
             \x20   local i: integer = 1\n\
             \x20   while i <= #v do\n\
             \x20       if v[i] == alvo then\n\
             \x20           return i\n\
             \x20       end\n\
             \x20       i = i + 1\n\
             \x20   end\n\
             \x20   return nil\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   local v: {integer} = {10, 20, 30}\n\
             \x20   local achou: integer? = busca(v, 20)\n\
             \x20   if achou ~= nil then\n\
             \x20       print(\"achou-\" .. achou)\n\
             \x20   end\n\
             \x20   local nao: integer? = busca(v, 99)\n\
             \x20   if nao == nil then\n\
             \x20       print(\"nao-achou\")\n\
             \x20   end\n\
             \x20   local s: string? = \"oi\"\n\
             \x20   if s ~= nil then\n\
             \x20       print(\"str-\" .. s)\n\
             \x20   end\n\
             \x20   local acc: integer? = 0\n\
             \x20   local i: integer = 1\n\
             \x20   while i <= 3 do\n\
             \x20       if acc ~= nil then\n\
             \x20           acc = acc + i\n\
             \x20       end\n\
             \x20       i = i + 1\n\
             \x20   end\n\
             \x20   if acc ~= nil then\n\
             \x20       print(\"acc-\" .. acc)\n\
             \x20   end\n\
             \x20   return 0\n\
             end",
        );

        let (avisos, output) = compila_e_executa(&rust, "t69_opcional");
        assert!(
            avisos.is_empty(),
            "warnings no Rust gerado:\n{avisos}\n{rust}"
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "achou-2\nnao-achou\nstr-oi\nacc-6\n"
        );
        assert_eq!(output.status.code(), Some(0));
    }
}
