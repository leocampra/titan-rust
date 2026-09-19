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
//! A **fronteira de FFI** (T73, ADR 0025) tem uma tabela própria
//! ([`c_abi_type_name`]), que difere desta em exatamente um ponto: `string`
//! vira `*const c_char`, o `char*` do C, e não `String` — a conversão nos
//! dois sentidos fica no runtime (`ffi_cstring`/`ffi_string`).
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
    BinOp, Callee, CastKind, TypedExp, TypedExpKind, TypedForInKind, TypedLValue, TypedMultiValues,
    TypedMatchArm, TypedPattern, TypedProgram, TypedStat, TypedThen, TypedTopLevel, UnOp,
};
use crate::types::Type;
use std::collections::{HashMap, HashSet};

const INDENT: &str = "    ";

/// O que toda função de emissão precisa saber sobre o **contexto** em que
/// escreve, além da própria expressão: os parâmetros compostos da função
/// atual e o mapa de campos encaixotados dos enums do programa.
///
/// Era só o conjunto de parâmetros compostos até a T77; virou struct quando
/// a emissão de tipos soma passou a precisar, no mesmo ponto, de saber quais
/// campos de variante saem `Box<T>` — a alternativa seria um segundo
/// parâmetro em trinta assinaturas.
struct EmitCtx {
    /// Nomes de parâmetro composto (`array`/`map`/`record`) da função
    /// **atual** — dentro do corpo, esses nomes já são uma referência Rust
    /// (`&mut T`, [`rust_param_type_name`]), então emprestá-los de novo
    /// (`&x`/`&mut x`) duplicaria a referência (`&mut &mut Vec<_>`) e o
    /// rustc recusaria o reborrow sem `mut` na ligação. Toda função de
    /// emissão que decide entre "nome cru" e "nome emprestado" (T30) consulta
    /// este conjunto para saber distinguir os dois casos; variável local
    /// composta não entra aqui — ela é dona do valor e precisa do empréstimo
    /// normal.
    params: HashSet<String>,
    /// Campos de variante que saem `Box<T>` (T77) — ver [`campos_boxeados`].
    /// Vale para o programa inteiro, não só para a função atual: a decisão é
    /// da **declaração** do enum, e construção e padrão precisam concordar
    /// com ela em qualquer função.
    boxed: BoxedFields,
}

impl EmitCtx {
    /// Atalho de leitura para os nomes de parâmetro composto, que é como os
    /// três pontos de decisão de empréstimo (T30) sempre consultaram o
    /// contexto.
    fn e_parametro_composto(&self, name: &str) -> bool {
        self.params.contains(name)
    }

    /// `true` se o campo de índice `field` da variante `variant` sai
    /// `Box<T>` na declaração do enum.
    fn e_boxeado(&self, variant: &str, field: usize) -> bool {
        self.boxed.contains(&(variant.to_string(), field))
    }
}

type Ctx<'a> = &'a EmitCtx;

/// Quais campos de variante saem `Box<T>` no `enum` do Rust, identificados
/// por `(nome da variante, índice do campo)`.
///
/// A variante basta como chave sem o nome do enum: o checker (T76) exige que
/// nomes de variante sejam **únicos no programa inteiro**, justamente porque
/// a construção `ExpInteger(42)` não diz de que enum ela vem.
type BoxedFields = HashSet<(String, usize)>;

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

/// Decide quais campos de variante saem `Box<T>` no `enum` do Rust — **a
/// armadilha central da fase** (PRD.md, T77).
///
/// Um `enum Exp` com um campo do próprio `Exp` seria, em Rust, um tipo de
/// tamanho infinito, e o rustc o recusaria em inglês ("recursive type has
/// infinite size") sobre código que o usuário não escreveu. O `Box` quebra o
/// ciclo, e quem o insere é este backend — de propósito, e não por acidente:
/// ao contrário de `checker.rs` (que detecta ciclo de record **para
/// rejeitar**), aqui o ciclo é detectado **para ser suportado** (decisão
/// técnica 6 do PRD.md).
///
/// A regra é "alcança o próprio enum sem passar por indireção": o campo é
/// encaixotado se, andando pelos tipos que ficam **embutidos** no valor
/// (outro `enum` pelo nome, os campos de um `record`), chega-se ao enum sendo
/// declarado.
///
/// Os três tipos que **param** a busca são justamente os que já carregam a
/// indireção de graça: `{Exp}` é `Vec<Exp>`, `{string: Exp}` é
/// `HashMap<String, Exp>` e `Exp?` é `Option<Exp>` — o primeiro põe os
/// elementos no heap, o segundo também, e o terceiro tem o tamanho do maior
/// braço, que já é finito porque o `Exp` de dentro é o mesmo que estamos
/// dimensionando. Encaixotar um deles compilaria e só acrescentaria uma
/// alocação por valor.
///
/// A travessia atravessa `Sum` **pelo nome**, consultando a tabela do
/// programa, e não pelas variantes embutidas no próprio tipo: um `Sum`
/// aninhado chega do checker como placeholder de variantes vazias
/// (`collect_enum_names`), então recursão mútua (`enum A AB(B) end` +
/// `enum B BA(A) end`) só é visível pela tabela. Um `record`, ao contrário,
/// chega com os campos resolvidos, e por isso o ciclo indireto
/// `enum Exp ExpNo(Caixa) end` + `record Caixa e: Exp end` — que a checagem
/// de ciclo de record não vê, porque `Exp` não é um record — é encontrado
/// aqui.
fn campos_boxeados(program: &TypedProgram) -> BoxedFields {
    let enums: HashMap<&str, &[(String, Vec<Type>)]> = program
        .iter()
        .filter_map(|top| match top {
            TypedTopLevel::Enum { name, variants, .. } => {
                Some((name.as_str(), variants.as_slice()))
            }
            _ => None,
        })
        .collect();

    /// `true` se um valor de `ty` contém um `alvo` **embutido** — o que, em
    /// Rust, faria o tamanho de um depender do tamanho do outro.
    ///
    /// `visitados` guarda os enums já abertos, e é o que faz a travessia
    /// terminar em **todo** ciclo: o de dois enums mútuos, e também o que
    /// passa por record (`record A e: E end` + `record B a: A end` +
    /// `enum E EB(B) EA(A) end`), porque todo ciclo que chega aqui tem de
    /// atravessar ao menos um `enum` — ciclo só de records o checker já
    /// rejeitou ("o record ... é recursivo"), e é por isso que o braço de
    /// `Record` não precisa de guarda própria.
    fn alcanca(
        ty: &Type,
        alvo: &str,
        enums: &HashMap<&str, &[(String, Vec<Type>)]>,
        visitados: &mut HashSet<String>,
    ) -> bool {
        match ty {
            Type::Sum { name, .. } => {
                if name == alvo {
                    return true;
                }
                if !visitados.insert(name.clone()) {
                    return false;
                }
                let Some(variants) = enums.get(name.as_str()) else {
                    return false;
                };
                variants
                    .iter()
                    .flat_map(|(_, fields)| fields)
                    .any(|f| alcanca(f, alvo, enums, visitados))
            }
            Type::Record { fields, .. } => fields
                .iter()
                .any(|(_, f)| alcanca(f, alvo, enums, visitados)),
            // Indireção: o tamanho do container não depende do tamanho do
            // que ele guarda.
            Type::Array { .. } | Type::Map { .. } | Type::Option { .. } => false,
            _ => false,
        }
    }

    let mut boxed = BoxedFields::new();
    for (enum_name, variants) in &enums {
        for (variant, fields) in variants.iter() {
            for (i, field) in fields.iter().enumerate() {
                let mut visitados = HashSet::new();
                if alcanca(field, enum_name, &enums, &mut visitados) {
                    boxed.insert((variant.clone(), i));
                }
            }
        }
    }
    boxed
}

/// Gera o `main.rs` completo (structs de record + funções do programa + shim
/// de entrada) a partir da AST tipada.
///
/// Records e enums saem primeiro, num laço à parte — nenhuma função os
/// referencia antes de todos estarem declarados, mas manter a ordem "tipos
/// antes de funções" é convenção usual do Rust gerado. Entre os dois a ordem
/// não importa: no Rust, um item pode referenciar outro declarado adiante.
pub fn generate(program: &TypedProgram) -> Result<String, CodegenError> {
    // O mapa de campos encaixotados (T77) é calculado **uma vez**, sobre o
    // programa inteiro, e vale para toda a emissão: a declaração do `enum`, a
    // construção de variante e o padrão do `match` têm de concordar sobre
    // quais campos são `Box<T>`, e a única forma de não divergirem é os três
    // lerem a mesma resposta.
    let ctx = EmitCtx {
        params: HashSet::new(),
        boxed: campos_boxeados(program),
    };

    let mut out = String::new();

    for top in program {
        if let TypedTopLevel::Record { name, fields, .. } = top {
            emit_record_struct(&mut out, name, fields);
            out.push('\n');
        }
    }

    for top in program {
        if let TypedTopLevel::Enum { name, variants, .. } = top {
            emit_enum(&mut out, name, variants, &ctx);
            out.push('\n');
        }
    }

    // `foreign function` (T73) → um bloco `unsafe extern "C"` por
    // declaração. Um bloco por função, em vez de um só com todas: a ordem
    // do fonte Titan é preservada, cada declaração fica ao lado do que a
    // originou, e o `extern` nunca vira uma lista distante do resto.
    for top in program {
        if let TypedTopLevel::ForeignFunc {
            name,
            params,
            rettypes,
            ..
        } = top
        {
            emit_foreign_extern(&mut out, name, params, rettypes);
            out.push('\n');
        }
    }

    for top in program {
        if matches!(top, TypedTopLevel::Func { .. }) {
            emit_toplevel(&mut out, top, &ctx.boxed);
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

/// `enum Nome Variante(Tipo, ..) .. end` → o `enum` do Rust (T77), no molde
/// de [`emit_record_struct`]: mesmos `derive`, mesmo namespace de tipos sem
/// mangling (ADR 0009).
///
/// A diferença que dá nome à tarefa é o `Box`: campo cujo tipo alcança o
/// próprio enum sai `Box<T>`, pela decisão que [`campos_boxeados`] tomou uma
/// vez para o programa inteiro. Variante sem campo sai sem parênteses
/// (`Vermelho`, não `Vermelho()`) — o segundo compilaria, mas o padrão
/// `Cor::Vermelho` do `match` não casaria com ele.
fn emit_enum(out: &mut String, name: &str, variants: &[(String, Vec<Type>)], ctx: Ctx) {
    out.push_str("#[derive(Clone, Debug, PartialEq)]\n");
    out.push_str("pub enum ");
    out.push_str(name);
    out.push_str(" {\n");
    for (vname, fields) in variants {
        out.push_str(INDENT);
        out.push_str(vname);
        if !fields.is_empty() {
            let tipos: Vec<String> = fields
                .iter()
                .enumerate()
                .map(|(i, fty)| rust_field_type_name(fty, ctx.e_boxeado(vname, i)))
                .collect();
            out.push('(');
            out.push_str(&tipos.join(", "));
            out.push(')');
        }
        out.push_str(",\n");
    }
    out.push_str("}\n");
}

/// Tipo Rust de um campo de variante: [`rust_type_name`], envolvido em
/// `Box<..>` quando o campo foi encaixotado (T77).
fn rust_field_type_name(ty: &Type, boxeado: bool) -> String {
    if boxeado {
        format!("Box<{}>", rust_type_name(ty))
    } else {
        rust_type_name(ty)
    }
}

/// `foreign function abs(n: integer): integer` (T73) → o bloco `extern "C"`
/// que declara o símbolo para o linker.
///
/// Os nomes dos parâmetros entram na declaração só por legibilidade do Rust
/// gerado (o `extern` não os usa); o que importa é a lista de tipos, que sai
/// por [`c_abi_type_name`] — e **não** por [`rust_type_name`], porque a
/// fronteira C não conhece `String`.
///
/// Sem mangling no nome (ao contrário de [`mangle_fn_name`]): este é o
/// símbolo que o linker vai procurar em libc, e renomeá-lo faria a busca
/// falhar. Colisão com o `fn main` do shim não é possível — o checker recusa
/// redeclarar um nome já declarado, e `main` em Titan é uma `function`
/// comum, que o mangling afasta.
fn emit_foreign_extern(out: &mut String, name: &str, params: &[(String, Type)], rettypes: &[Type]) {
    out.push_str("unsafe extern \"C\" {\n");
    out.push_str(INDENT);
    out.push_str("fn ");
    out.push_str(name);
    out.push('(');
    let param_list: Vec<String> = params
        .iter()
        .map(|(pname, ty)| format!("{pname}: {}", c_abi_type_name(ty)))
        .collect();
    out.push_str(&param_list.join(", "));
    out.push(')');
    if let Some(ret) = c_abi_rettype_name(rettypes) {
        out.push_str(" -> ");
        out.push_str(&ret);
    }
    out.push_str(";\n}\n");
}

/// Tipo Rust de um valor **na fronteira C** (T73) — o mapeamento que difere
/// de [`rust_type_name`] em exatamente um ponto: `string`.
///
/// Um `String` do Rust é ponteiro + tamanho + capacidade, layout que nenhuma
/// função C sabe ler; na fronteira ele vira `*const c_char`, o `char*`
/// terminado em zero que o C espera, e a conversão nos dois sentidos fica no
/// runtime (`ffi_cstring`/`ffi_string`). Os escalares atravessam como são:
/// `i64` é o `int64_t` do C, `f64` é `double`, e `bool` tem `repr(C)`
/// garantido como o `_Bool` do C99.
///
/// Todo tipo fora dessa lista já foi recusado por
/// `check_foreign_boundary_type` (`checker.rs`) com erro em português — daí
/// o `unreachable!` do braço final, no molde de [`rust_type_name`].
fn c_abi_type_name(ty: &Type) -> String {
    match ty {
        Type::Integer => "i64".to_string(),
        Type::Float => "f64".to_string(),
        Type::Boolean => "bool".to_string(),
        Type::String => "*const std::os::raw::c_char".to_string(),
        other => unreachable!(
            "tipo '{other:?}' não atravessa a fronteira de FFI — checker deveria ter rejeitado antes"
        ),
    }
}

/// Retorno de uma `foreign function` na fronteira C. `None` é o `void` do C
/// — ou a lista vazia, ou o único retorno `nil` —, mesmo critério de
/// [`rust_rettype_name`]. Mais de um retorno nem chega aqui: o checker já
/// recusa, porque a ABI C devolve um valor só.
fn c_abi_rettype_name(rettypes: &[Type]) -> Option<String> {
    match rettypes {
        [] | [Type::Nil] => None,
        [único] => Some(c_abi_type_name(único)),
        vários => unreachable!(
            "`foreign function` com {} retornos — checker deveria ter rejeitado antes",
            vários.len()
        ),
    }
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

fn emit_toplevel(out: &mut String, top: &TypedTopLevel, boxed: &BoxedFields) {
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
    // carrega essa lista para toda a emissão do corpo saber a diferença, ao
    // lado do mapa de campos encaixotados (T77), que é do programa e não
    // desta função.
    let ctx = EmitCtx {
        params: params
            .iter()
            .filter(|(_, ty)| is_composite(ty))
            .map(|(name, _)| name.clone())
            .collect(),
        boxed: boxed.clone(),
    };

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
        // `for`-in (T71): o container é lido — é ele que dá o `&mut`/`&` do
        // iterador —, as variáveis do laço são destinos como as de `For`.
        TypedStat::ForIn {
            container, block, ..
        } => {
            collect_referenced_names_exp(container, names);
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
        TypedStat::Assign { target, value, .. } => {
            collect_referenced_names_lvalue(target, names);
            collect_referenced_names_exp(value, names);
        }
        // `match` (T76): o escrutinado é lido, e o corpo de cada braço é
        // código como o de um ramo do `if`. Os nomes que o padrão **liga**
        // não entram: eles são destinos do padrão, não leituras — exatamente
        // como a variável de controle de um `for`.
        TypedStat::Match { exp, arms, .. } => {
            collect_referenced_names_exp(exp, names);
            for arm in arms {
                collect_referenced_names_stat(&arm.body, names);
            }
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
        | TypedExpKind::SomeOf(inner)
        // `Cast` (T70), idem: o `as` não lê nome nenhum por conta própria.
        | TypedExpKind::Cast { exp: inner, .. } => collect_referenced_names_exp(inner, names),
        // Construção de variante (T76): os argumentos são expressões como
        // as de uma chamada.
        TypedExpKind::VariantLit { args, .. } => {
            for arg in args {
                collect_referenced_names_exp(arg, names);
            }
        }
        // `match` como expressão (T76): mesma leitura do comando.
        TypedExpKind::Match { exp, arms } => {
            collect_referenced_names_exp(exp, names);
            for arm in arms {
                collect_referenced_names_exp(&arm.body, names);
            }
        }
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
            | TypedStat::For { block, .. }
            | TypedStat::ForIn { block, .. } => stat_atribui(block, nome),
            // `match` (T76): um braço atribui ao nome como um ramo do `if`.
            TypedStat::Match { arms, .. } => arms.iter().any(|arm| stat_atribui(&arm.body, nome)),
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
        // `for`-in (T71) → `for` **nativo** do Rust sobre `.iter()`, nunca o
        // template de `loop` do `for` numérico (ADR 0022): aquele existe
        // porque `Range` não cobre passo negativo nem float, e nada disso se
        // aplica a percorrer um container. Aqui o iterador do Rust é
        // exatamente a construção certa, e `break`/`continue` (T63) caem
        // dentro dele sem caso especial nenhum.
        //
        // `.iter()` e não `.iter_mut()`: o checker já recusou qualquer
        // mutação do container dentro do corpo, então não há o que mutar
        // através do iterador — e um `&mut` desnecessário só arriscaria
        // empréstimos que o `rustc` recusaria em inglês.
        //
        // O nome que o iterador liga é `titan_forin_*` (referência), e o
        // nome do usuário nasce logo dentro do corpo, por valor — mesmo
        // idioma do `if let` do narrowing (T68). Isso resolve de uma vez
        // três coisas: o corpo lê `x` com o tipo `T` que o checker lhe deu,
        // o clone de composto/`String` do ADR 0006 acontece num lugar só, e
        // escalares saem com um `*` que não custa nada.
        TypedStat::ForIn {
            kind,
            container,
            block,
            ..
        } => {
            let base = emit_exp(container, ctx);
            // Nome do laço que o corpo nunca **usa** não ganha ligação
            // nenhuma, e o iterador o descarta com `_` — `for k, v in m do`
            // que só olha `v` é escrita natural, e abrir um `let k` que
            // ninguém lê renderia `unused_variables`. Mesma disciplina do
            // narrowing (T68) e mesmo critério de aceite da T69: Rust **sem
            // warnings**.
            //
            // "Usar" inclui **escrever**: `for x in v do x = 0 end` é aceito
            // pelo checker (a variável do laço é `SymbolKind::ForVar`, que
            // permite atribuição), e a ligação precisa existir — e ser `mut`
            // — senão o corpo emitiria `x = 0;` para um `x` que não foi
            // declarado. É atribuição a uma **cópia**, sem efeito sobre o
            // container (ADR 0024), como no `for` numérico.
            let lidos = referenced_names(block);
            let usado = |nome: &str| lidos.contains(nome) || assigns_to_name(block, nome);
            indent(out, depth);
            match kind {
                TypedForInKind::Array { name, elem_ty } => {
                    let interno = forin_pattern(name, usado(name));
                    out.push_str(&format!("for {interno} in {base}.iter() {{\n"));
                    emit_forin_binding(out, name, elem_ty, &interno, block, usado(name), depth + 1);
                }
                TypedForInKind::Map {
                    key_name,
                    key_ty,
                    value_name,
                    value_ty,
                } => {
                    let chave = forin_pattern(key_name, usado(key_name));
                    let valor = forin_pattern(value_name, usado(value_name));
                    out.push_str(&format!("for ({chave}, {valor}) in {base}.iter() {{\n"));
                    emit_forin_binding(
                        out,
                        key_name,
                        key_ty,
                        &chave,
                        block,
                        usado(key_name),
                        depth + 1,
                    );
                    emit_forin_binding(
                        out,
                        value_name,
                        value_ty,
                        &valor,
                        block,
                        usado(value_name),
                        depth + 1,
                    );
                }
            }
            emit_block_stats(out, block, depth + 1, ctx);
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
        // `match e with ... end` (T77) → o `match` do Rust. O escrutinado sai
        // **emprestado** (`match &e { .. }`), e não por valor: com valor, um
        // padrão que liga campos moveria o escrutinado para dentro do braço, e
        // `local a: Cor = Vermelho; local b: Cor = a; match a with` — que o
        // checker aceita — deixaria de compilar por E0382 sobre código que o
        // usuário não escreveu. Emprestar faz cada campo ligado chegar como
        // referência, e [`emit_arm_bindings`] os traz de volta para valor com
        // a mesma regra do `for`-in (ADR 0006): clone para quem tem buffer
        // próprio, deref para escalar.
        TypedStat::Match { exp, arms, .. } => {
            indent(out, depth);
            out.push_str("match &");
            out.push_str(&emit_exp(exp, ctx));
            out.push_str(" {\n");
            for arm in arms {
                indent(out, depth + 1);
                out.push_str(&emit_pattern(&arm.pattern, &arm.body));
                out.push_str(" => {\n");
                emit_arm_bindings(out, &arm.pattern, &arm.body, depth + 2, ctx);
                emit_block_stats(out, &arm.body, depth + 2, ctx);
                indent(out, depth + 1);
                out.push_str("}\n");
            }
            indent(out, depth);
            out.push_str("}\n");
        }
    }
}

/// Liga o nome do usuário, por valor, ao que o iterador do `for`-in (T71)
/// entregou por referência.
///
/// Compostos e `String` clonam (ADR 0006: cada nome é dono da sua cópia, e
/// mutar a variável do laço não pode alcançar o container); o resto
/// desreferencia. `let` sem `mut` de propósito: o fix-up de mutabilidade não
/// alcança variáveis de laço, e uma atribuição a elas é `SymbolKind::ForVar`
/// no checker — que a T71 não precisou abrir, porque o corpo que atribui à
/// variável do laço cai no `unused_mut`/`immutable` do rustc antes. Uma fase
/// futura que queira permitir `x = x + 1` sobre a variável ligada faz disso
/// um `let mut`.
fn emit_forin_binding(
    out: &mut String,
    name: &str,
    ty: &Type,
    interno: &str,
    block: &TypedStat,
    usado: bool,
    depth: usize,
) {
    if !usado {
        return;
    }
    indent(out, depth);
    let rust_ty = rust_type_name(ty);
    // `let mut` só quando o corpo escreve na variável — senão o rustc
    // reclamaria de `unused_mut`, e o critério é Rust sem warnings.
    let bind = if assigns_to_name(block, name) {
        "let mut"
    } else {
        "let"
    };
    if valor_com_buffer_proprio(ty) || *ty == Type::String {
        out.push_str(&format!("{bind} {name}: {rust_ty} = {interno}.clone();\n"));
    } else {
        out.push_str(&format!("{bind} {name}: {rust_ty} = *{interno};\n"));
    }
}

/// Padrão que o `for` do Rust liga para uma variável do `for`-in: o nome
/// interno quando o corpo usa a variável, `_` quando não — o descarte tem de
/// estar no padrão, e não só na ligação omitida, senão o próprio
/// `titan_forin_*` fica sem uso.
fn forin_pattern(name: &str, usado: bool) -> String {
    if usado {
        format!("titan_forin_{name}")
    } else {
        "_".to_string()
    }
}

/// Padrão Rust de um braço de `match` (T77): `Enum::Variante(a, b)` ou `_`.
///
/// O nome de um campo que o corpo nunca usa sai como `_`, e não como o nome
/// ligado: o escrutinado é emprestado, então a ligação não é descartável de
/// graça — um nome sem uso dispararia `unused_variables`, e o critério da
/// tarefa é Rust gerado **sem warnings**. O `_` no padrão também dispensa
/// [`emit_arm_bindings`] de emitir a ligação correspondente, e as duas
/// decisões usam a mesma pergunta ([`campo_usado`]) para não divergirem.
fn emit_pattern(pattern: &TypedPattern, body: &impl UsaNome) -> String {
    let TypedPattern::Variant {
        enum_name,
        name,
        fields,
        ..
    } = pattern
    else {
        return "_".to_string();
    };
    if fields.is_empty() {
        return format!("{enum_name}::{name}");
    }
    let ligados: Vec<String> = fields
        .iter()
        .map(|(fname, _)| {
            if campo_usado(fname, body) {
                format!("{}{fname}", PREFIXO_CAMPO)
            } else {
                "_".to_string()
            }
        })
        .collect();
    format!("{enum_name}::{name}({})", ligados.join(", "))
}

/// Prefixo do nome que o **padrão** liga, para o `let` de
/// [`emit_arm_bindings`] poder dar ao usuário o nome sem prefixo sem
/// sombrear a si mesmo no próprio inicializador. Mesmo papel que o
/// `titan_forin_` do `for`-in (T71).
const PREFIXO_CAMPO: &str = "titan_match_";

/// Traz cada campo ligado pelo padrão de referência para valor, no topo do
/// braço (T77) — o análogo de [`emit_forin_binding`], e pela mesma razão: o
/// escrutinado é emprestado, mas em Titan o nome ligado é um valor comum
/// (ADR 0006), que o corpo pode passar adiante e cujo container não deve
/// enxergar mutação.
///
/// Composto e `string` clonam; escalar desreferencia. Campo **encaixotado**
/// (T77) chega como `&Box<T>` e precisa de um deref a mais — `(**b).clone()`
/// para o enum recursivo, que é o caso central da tarefa.
fn emit_arm_bindings(
    out: &mut String,
    pattern: &TypedPattern,
    body: &impl UsaNome,
    depth: usize,
    ctx: Ctx,
) {
    let TypedPattern::Variant { name, fields, .. } = pattern else {
        return;
    };
    for (i, (fname, fty)) in fields.iter().enumerate() {
        if !campo_usado(fname, body) {
            continue;
        }
        let interno = format!("{PREFIXO_CAMPO}{fname}");
        // `let mut` só quando o corpo escreve **através** do nome
        // (`p.campo = 9` sobre um campo record): atribuir ao nome inteiro é
        // proibido pelo checker, que trata o ligado como parâmetro. Sem a
        // pergunta, todo campo composto sairia `mut` e o `unused_mut` do
        // rustc reclamaria.
        let bind = if body.escreve_atraves_de(fname) {
            "let mut"
        } else {
            "let"
        };
        let rust_ty = rust_type_name(fty);
        let boxeado = ctx.e_boxeado(name, i);
        let valor = if valor_com_buffer_proprio(fty) || *fty == Type::String {
            // Um deref a mais no encaixotado: o padrão ligou `&Box<T>`, e
            // `interno.clone()` cru daria um `Box<T>` onde se espera `T`.
            if boxeado {
                format!("(**{interno}).clone()")
            } else {
                format!("{interno}.clone()")
            }
        } else if boxeado {
            format!("**{interno}")
        } else {
            format!("*{interno}")
        };
        indent(out, depth);
        out.push_str(&format!("{bind} {fname}: {rust_ty} = {valor};\n"));
    }
}

/// `true` se o corpo do braço lê o nome ligado, ou escreve através dele.
fn campo_usado(nome: &str, body: &impl UsaNome) -> bool {
    body.le(nome) || body.escreve_atraves_de(nome)
}

/// O que a emissão de um braço precisa perguntar ao seu corpo, que é um
/// [`TypedStat`] no `match`-comando e um [`TypedExp`] no `match`-expressão
/// (T77). Trait em vez de dois pares de funções porque as perguntas são as
/// mesmas e as respostas vêm das mesmas travessias já existentes.
trait UsaNome {
    /// O corpo lê o nome em alguma expressão.
    fn le(&self, nome: &str) -> bool;
    /// O corpo escreve **através** do nome (`p.campo = 9`, `xs[1] = 9`) — o
    /// que exige `let mut` na ligação. Atribuir ao nome inteiro não entra:
    /// o checker o trata como parâmetro e já recusou.
    fn escreve_atraves_de(&self, nome: &str) -> bool;
}

impl UsaNome for TypedStat {
    fn le(&self, nome: &str) -> bool {
        referenced_names(self).contains(nome)
    }

    fn escreve_atraves_de(&self, nome: &str) -> bool {
        escreve_atraves_no_stat(self, nome)
    }
}

impl UsaNome for TypedExp {
    fn le(&self, nome: &str) -> bool {
        let mut names = HashSet::new();
        collect_referenced_names_exp(self, &mut names);
        names.contains(nome)
    }

    /// Uma expressão não contém atribuição em Titan — não há `=` dentro de
    /// expressão nesta linguagem —, então nada escreve através de nome
    /// nenhum no corpo de um braço de `match`-expressão.
    fn escreve_atraves_de(&self, _nome: &str) -> bool {
        false
    }
}

/// Alguma atribuição no comando escreve **através** de `nome` — isto é, o
/// alvo é `nome[i]` ou `nome.campo` (em qualquer profundidade), e não `nome`
/// inteiro.
fn escreve_atraves_no_stat(stat: &TypedStat, nome: &str) -> bool {
    fn raiz_e_o_nome(target: &TypedLValue, nome: &str) -> bool {
        match target {
            // Atribuição ao nome inteiro não é escrita *através* dele.
            TypedLValue::Name(_) => false,
            TypedLValue::Index { base, .. } | TypedLValue::Field { base, .. } => {
                raiz_de_exp(base) == Some(nome)
            }
        }
    }
    fn raiz_de_exp(exp: &TypedExp) -> Option<&str> {
        match &exp.kind {
            TypedExpKind::Var(n) => Some(n.as_str()),
            TypedExpKind::Index { base, .. } | TypedExpKind::Field { base, .. } => {
                raiz_de_exp(base)
            }
            _ => None,
        }
    }
    fn no_stat(stat: &TypedStat, nome: &str) -> bool {
        match stat {
            TypedStat::Block { stats, .. } => stats.iter().any(|s| no_stat(s, nome)),
            TypedStat::Assign { target, .. } => raiz_e_o_nome(target, nome),
            TypedStat::AssignMulti { targets, .. } => {
                targets.iter().any(|t| raiz_e_o_nome(t, nome))
            }
            TypedStat::If {
                thens, elsestat, ..
            } => {
                thens.iter().any(|t| no_stat(&t.block, nome))
                    || elsestat.as_ref().is_some_and(|e| no_stat(e, nome))
            }
            TypedStat::While { block, .. }
            | TypedStat::Repeat { block, .. }
            | TypedStat::For { block, .. }
            | TypedStat::ForIn { block, .. } => no_stat(block, nome),
            TypedStat::Match { arms, .. } => arms.iter().any(|arm| no_stat(&arm.body, nome)),
            TypedStat::Decl { .. }
            | TypedStat::DeclMulti { .. }
            | TypedStat::Call { .. }
            | TypedStat::Return { .. }
            | TypedStat::Break { .. }
            | TypedStat::Continue { .. } => false,
        }
    }
    no_stat(stat, nome)
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
        // Construção de variante (T77) → `Enum::Variante(args)`.
        TypedExpKind::VariantLit {
            enum_name,
            variant,
            args,
        } => emit_variant_lit(enum_name, variant, args, ctx),
        // `match` como expressão (T77) → o `match` do Rust, que já é uma
        // expressão — nenhum temporário, nenhum bloco em volta.
        TypedExpKind::Match {
            exp: scrutinee,
            arms,
        } => emit_match_exp(scrutinee, arms, true, ctx),
        TypedExpKind::Nil if matches!(exp.ty, Type::Option { .. }) => "None".to_string(),
        TypedExpKind::Nil => "()".to_string(),
        TypedExpKind::Bool(v) => v.to_string(),
        TypedExpKind::Integer(v) => v.to_string(),
        TypedExpKind::Float(v) => format_float_literal(*v),
        TypedExpKind::String(v) => format_string_literal(v),
        TypedExpKind::Var(name) => name.clone(),
        TypedExpKind::Concat(exps) => emit_concat(exps, ctx),
        TypedExpKind::Call { callee, args } => emit_call(callee, args, &exp.ty, ctx),
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
        // Cast `as` (T70).
        TypedExpKind::Cast { kind, exp: inner } => {
            format!("({})", emit_cast(*kind, inner, &exp.ty, ctx))
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
        // `x as float` (T70) vira `x as f64`, que em posição já delimitada
        // dispensa os parênteses externos — o `unused_parens` do rustc
        // reclamaria deles.
        TypedExpKind::Cast { kind, exp: inner } => emit_cast(*kind, inner, &exp.ty, ctx),
        // `match` como expressão (T77), pela mesma razão do `Cast`: em
        // posição delimitada os parênteses que [`emit_match_exp`] põe viram
        // `unused_parens`. Em posição de operando eles são obrigatórios, e é
        // lá que `emit_exp` os mantém.
        TypedExpKind::Match {
            exp: scrutinee,
            arms,
        } => emit_match_exp(scrutinee, arms, false, ctx),
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
    } else if valor_com_buffer_proprio(slot_ty) && precisa_clone(value) {
        format!("{}.clone()", emit_exp(value, ctx))
    } else {
        emit_delimited_exp(value, ctx)
    }
}

/// Tipos que caem na regra de clone do ADR 0006: os compostos de
/// [`is_composite`] **mais** os tipos soma (T77).
///
/// Um `enum` não entra em [`is_composite`], e de propósito: `is_composite`
/// responde "sai por `&mut` em posição de parâmetro" (ADR 0007), e um valor de
/// tipo soma sai por valor — o checker já tipa a chamada assim, e um `Exp`
/// recursivo é um `Box` no bolso, não uma `Vec` para mutar no lugar. Mas ele
/// é dono de buffer próprio como qualquer record, então `local b: Exp = a`
/// tem de clonar, ou o Rust moveria `a` e a semântica de valor do Titan
/// deixaria de valer para exatamente um tipo.
fn valor_com_buffer_proprio(ty: &Type) -> bool {
    is_composite(ty) || matches!(ty, Type::Sum { .. })
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
        TypedExpKind::Var(name) if ctx.e_parametro_composto(name) => name.clone(),
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
        && ctx.e_parametro_composto(name)
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
        TypedExpKind::Var(name) if ctx.e_parametro_composto(name) => format!("(*{name})"),
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

/// Cast `as` (T70). Quatro formas, uma por [`CastKind`]:
///
/// - **numérica** → o `as` do próprio Rust (`as f64` / `as i64`). É a única
///   que não passa pelo runtime: são instruções de conversão, não lógica.
///   `float as integer` **trunca** em direção a zero, que é a semântica do
///   `as` do Rust e a que o README documenta como diferente do `//` da T61.
/// - **subida a `value`** → constrói o `titan_runtime::Value` da variante
///   correspondente ao tipo de origem, recursivamente para compostos.
/// - **descida de `value`** → chama o extrator do runtime, que aborta com
///   mensagem em português quando a variante guardada não é a pedida.
fn emit_cast(kind: CastKind, inner: &TypedExp, target: &Type, ctx: Ctx) -> String {
    match kind {
        CastKind::IntToFloat => format!("{} as f64", emit_exp(inner, ctx)),
        CastKind::FloatToInt => format!("{} as i64", emit_exp(inner, ctx)),
        CastKind::ToValue => emit_to_value(&inner.ty, inner, ctx),
        CastKind::FromValue => emit_from_value(target, inner, ctx),
    }
}

/// Empacota uma expressão de tipo `origem` num `titan_runtime::Value`.
///
/// Compostos são convertidos **elemento a elemento** em tempo de execução,
/// porque um `Vec<i64>` e um `Vec<Value>` são tipos Rust distintos — não há
/// reinterpretação possível, e o ADR 0006 já diz que a conversão copia. O
/// `map` vira `Vec<(Value, Value)>` ordenado pela iteração do `HashMap`, que
/// é não especificada; isso não é observável, porque a comparação de
/// `Value::Map` do runtime é feita como conjunto.
fn emit_to_value(origem: &Type, exp: &TypedExp, ctx: Ctx) -> String {
    let val = |s: String| format!("titan_runtime::Value::{s}");
    match origem {
        Type::Nil => val("Nil".to_string()),
        Type::Boolean => val(format!("Boolean({})", emit_delimited_exp(exp, ctx))),
        Type::Integer => val(format!("Integer({})", emit_delimited_exp(exp, ctx))),
        Type::Float => val(format!("Float({})", emit_delimited_exp(exp, ctx))),
        Type::String => val(format!("String({})", emit_owned_string(exp, ctx))),
        // `.iter()` já empresta sozinho: o receptor sai por [`emit_exp`] cru,
        // sem o `&` de [`borrow_composite`], que aqui viraria `(&v).iter()`
        // — legal, mas ruído — ou, pior, um `&` a mais sobre um parâmetro
        // que já é `&mut T`.
        Type::Array { elem } => val(format!(
            "Array({}.iter().map(|titan_e| {}).collect())",
            emit_exp(exp, ctx),
            emit_to_value_of_var(elem, "titan_e")
        )),
        Type::Map { keys, values } => val(format!(
            "Map({}.iter().map(|(titan_k, titan_v)| ({}, {})).collect())",
            emit_exp(exp, ctx),
            emit_to_value_of_var(keys, "titan_k"),
            emit_to_value_of_var(values, "titan_v")
        )),
        Type::Record { name, fields } => {
            let campos = fields
                .iter()
                .map(|(fname, fty)| {
                    format!(
                        "(\"{fname}\".to_string(), {})",
                        emit_to_value_of_field(fty, &format!("titan_r.{fname}"))
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "{{ let titan_r = {}; {} }}",
                borrow_composite(exp, ctx),
                val(format!(
                    "Record {{ nome: \"{name}\".to_string(), campos: vec![{campos}] }}"
                ))
            )
        }
        // `T?` preenchido vira `Option(Box<..>)`; vazio vira `Nil`, para
        // `value` continuar com um único "ausente".
        Type::Option { base } => format!(
            "match {} {{ Some(titan_o) => {}, None => {} }}",
            borrow_composite(exp, ctx),
            val(format!(
                "Option(Box::new({}))",
                emit_to_value_of_var(base, "titan_o")
            )),
            val("Nil".to_string())
        ),
        // `Value as value` é identidade e `check_cast` já o devolveu sem
        // construir nó nenhum; `Function`/`Opaque`/`Invalid` não chegam aqui
        // porque o checker não os aceita como operando de `as`.
        outro => unreachable!(
            "tipo '{outro:?}' não deveria chegar a `emit_to_value` — checker deveria ter rejeitado antes"
        ),
    }
}

/// Como [`emit_to_value`], mas para um valor que já está numa **variável**
/// Rust (o ligado por um `map`/`match` do código emitido acima), e sempre por
/// referência. Recursivo: composto dentro de composto desce por aqui.
///
/// Existe separado porque [`emit_to_value`] trabalha sobre um `TypedExp` — há
/// expressão Titan por trás —, e aqui só há um nome Rust que o próprio
/// backend inventou.
fn emit_to_value_of_var(ty: &Type, var: &str) -> String {
    let val = |s: String| format!("titan_runtime::Value::{s}");
    match ty {
        Type::Nil => val("Nil".to_string()),
        Type::Boolean => val(format!("Boolean(*{var})")),
        Type::Integer => val(format!("Integer(*{var})")),
        Type::Float => val(format!("Float(*{var})")),
        Type::String => val(format!("String({var}.clone())")),
        Type::Array { elem } => val(format!(
            "Array({var}.iter().map(|titan_e2| {}).collect())",
            emit_to_value_of_var(elem, "titan_e2")
        )),
        Type::Map { keys, values } => val(format!(
            "Map({var}.iter().map(|(titan_k2, titan_v2)| ({}, {})).collect())",
            emit_to_value_of_var(keys, "titan_k2"),
            emit_to_value_of_var(values, "titan_v2")
        )),
        Type::Record { name, fields } => {
            let campos = fields
                .iter()
                .map(|(fname, fty)| {
                    format!(
                        "(\"{fname}\".to_string(), {})",
                        emit_to_value_of_field(fty, &format!("{var}.{fname}"))
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            val(format!(
                "Record {{ nome: \"{name}\".to_string(), campos: vec![{campos}] }}"
            ))
        }
        Type::Option { base } => format!(
            "match {var} {{ Some(titan_o2) => {}, None => {} }}",
            val(format!(
                "Option(Box::new({}))",
                emit_to_value_of_var(base, "titan_o2")
            )),
            val("Nil".to_string())
        ),
        Type::Value => format!("{var}.clone()"),
        outro => unreachable!(
            "tipo '{outro:?}' não deveria chegar a `emit_to_value_of_var` — checker deveria ter rejeitado antes"
        ),
    }
}

/// Como [`emit_to_value_of_var`], mas para um **campo** (`titan_r.x`) em vez
/// de um nome ligado por `map`/`match`.
///
/// A diferença é um `*`: o binding de um `.iter()` é uma referência, e ler o
/// primitivo de dentro dele exige deref; um campo alcançado através de
/// `&Ponto` já é o valor, por auto-deref do Rust, e o `*` ali seria erro de
/// tipo. O resto das variantes é idêntico, então elas delegam.
fn emit_to_value_of_field(ty: &Type, place: &str) -> String {
    let val = |s: String| format!("titan_runtime::Value::{s}");
    match ty {
        Type::Boolean => val(format!("Boolean({place})")),
        Type::Integer => val(format!("Integer({place})")),
        Type::Float => val(format!("Float({place})")),
        // Composto/opcional/`string`/`nil` não usam `*` em `_of_var` — o
        // caminho é o mesmo, e o `&` que `.iter()`/`match` pedem sai de lá.
        outro => emit_to_value_of_var(outro, place),
    }
}

/// Desempacota um `value` para `alvo`, abortando em tempo de execução se a
/// variante guardada não corresponder.
///
/// Só primitivas descem: `value as {integer}` seria uma conversão
/// elemento a elemento com falha no meio — metade do array já convertido
/// quando o erro aparece —, e o checker a recusa antes de chegar aqui, o que
/// mantém a descida com um ponto de falha só.
fn emit_from_value(alvo: &Type, exp: &TypedExp, ctx: Ctx) -> String {
    let func = match alvo {
        Type::Boolean => "value_to_boolean",
        Type::Integer => "value_to_integer",
        Type::Float => "value_to_float",
        Type::String => "value_to_string",
        outro => unreachable!(
            "tipo '{outro:?}' não deveria chegar a `emit_from_value` — checker deveria ter rejeitado antes"
        ),
    };
    format!("titan_runtime::{func}(&{})", emit_exp(exp, ctx))
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
fn emit_call(callee: &Callee, args: &[TypedExp], ret: &Type, ctx: Ctx) -> String {
    match callee {
        Callee::Direct(name) => {
            if let Some(builtin) = crate::builtins::lookup(name) {
                let rendered_args = emit_args_by_param(args, builtin.params, ctx);
                return format!("{}({})", builtin.rust_path, rendered_args.join(", "));
            }
            let rendered_args: Vec<String> = args
                .iter()
                // Um argumento de tipo soma (T77) segue por **valor**, como
                // escalar, mas é dono de buffer próprio como record: sem o
                // `.clone()`, `tinge(c) + tinge(c)` moveria `c` na primeira
                // chamada e o rustc recusaria a segunda em inglês. É
                // `emit_slot_value` quem já sabe essa regra — o parâmetro é
                // um slot como qualquer outro.
                .map(|a| {
                    if a.ty == Type::String {
                        emit_owned_string(a, ctx)
                    } else if is_composite(&a.ty) {
                        emit_place_mut(a, ctx)
                    } else {
                        emit_slot_value(&a.ty, a, ctx)
                    }
                })
                .collect();
            format!("{}({})", mangle_fn_name(name), rendered_args.join(", "))
        }
        // `abs(-7)` com `abs` declarada por `foreign function` (T73). Três
        // diferenças em relação a `Callee::Direct`, todas de emissão:
        //
        // 1. **sem mangling** — o nome é o símbolo que o linker procura;
        // 2. **`unsafe`** — o rustc exige, e é o ponto do desenho: a
        //    responsabilidade pela assinatura é de quem escreveu a
        //    declaração, e o Rust gerado diz isso em voz alta;
        // 3. **`string` vira `CString`** — e a `CString` precisa continuar
        //    viva durante a chamada, daí a ligação `let` num bloco em vez de
        //    um `.as_ptr()` sobre um temporário, que seria um ponteiro
        //    pendurado assim que a expressão do argumento terminasse.
        //
        // Sem nenhum argumento `string`, o bloco não aparece: a chamada sai
        // como `unsafe { abs(-7) }`, sem ruído.
        Callee::Foreign(name) => emit_foreign_call(name, args, ret, ctx),
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

/// Chamada a uma `foreign function` (T73) — ver o braço `Callee::Foreign`
/// de [`emit_call`] para o porquê de cada parte.
///
/// O nome das ligações temporárias leva o prefixo `__titan_ffi_`, que nenhum
/// identificador Titan pode ter (o lexer não aceita `_` inicial seguido do
/// resto, e mesmo que aceitasse o mangling de `mangle_fn_name` afastaria a
/// colisão) — então a ligação nunca sombreia um nome do programa.
fn emit_foreign_call(name: &str, args: &[TypedExp], ret: &Type, ctx: Ctx) -> String {
    let mut bindings: Vec<String> = Vec::new();
    let rendered_args: Vec<String> = args
        .iter()
        .enumerate()
        .map(|(i, a)| {
            if a.ty == Type::String {
                let temp = format!("__titan_ffi_{i}");
                bindings.push(format!(
                    "let {temp} = titan_runtime::ffi_cstring(&{});",
                    emit_owned_string(a, ctx)
                ));
                format!("{temp}.as_ptr()")
            } else {
                emit_delimited_exp(a, ctx)
            }
        })
        .collect();

    // Retorno `string` chega como `*const c_char` e vira `String` dentro do
    // mesmo `unsafe` — desreferenciar o ponteiro é tão inseguro quanto a
    // chamada, e separá-los em dois blocos não acrescentaria garantia
    // nenhuma. `ffi_string` checa o nulo e aborta em português (o resto do
    // runtime faz igual com índice fora de faixa).
    let chamada = if *ret == Type::String {
        format!(
            "unsafe {{ titan_runtime::ffi_string({name}({})) }}",
            rendered_args.join(", ")
        )
    } else {
        format!("unsafe {{ {name}({}) }}", rendered_args.join(", "))
    };
    if bindings.is_empty() {
        return chamada;
    }
    // Parênteses em volta do bloco: em posição de statement (`f(s);`) um
    // `{ ... }` cru seria lido como bloco-statement, e o `;` que vem depois
    // viraria statement vazio — o valor da chamada se perderia em silêncio.
    format!("({{ {} {chamada} }})", bindings.join(" "))
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
            // Tipo soma (T77) cai na regra de slot, que clona a fonte que
            // sobrevive à chamada — nenhum builtin/capability de hoje recebe
            // um `enum`, mas a assinatura é quem manda, não a lista atual.
            p => emit_slot_value(p, a, ctx),
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

/// `ExpInteger(42)` como construção de variante (T77) →
/// `Exp::ExpInteger(42)`, e `Vermelho` sem campo → `Cor::Vermelho`, sem
/// parênteses, como a declaração de [`emit_enum`] os escreveu.
///
/// Cada argumento passa pela regra de slot do **campo** (`emit_slot_value`),
/// e não pela do próprio argumento: é o campo que define se a variante fica
/// dona de uma `String`/composto ou não, exatamente como em
/// [`emit_record_lit`]. O campo encaixotado ganha o `Box::new` por fora — e
/// é aqui que o `Box` da declaração se paga: `ExpBinop("+", a, b)` aloca os
/// dois operandos e o valor resultante tem tamanho finito.
fn emit_variant_lit(enum_name: &str, variant: &str, args: &[TypedExp], ctx: Ctx) -> String {
    if args.is_empty() {
        return format!("{enum_name}::{variant}");
    }
    let rendered: Vec<String> = args
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let valor = emit_slot_value(&a.ty, a, ctx);
            if ctx.e_boxeado(variant, i) {
                format!("Box::new({valor})")
            } else {
                valor
            }
        })
        .collect();
    format!("{enum_name}::{variant}({})", rendered.join(", "))
}

/// `match e with ... end` em posição de expressão (T77) → o `match` do Rust,
/// que já é uma expressão.
///
/// Mesma tradução do braço `TypedStat::Match` de [`emit_stat`] — escrutinado
/// emprestado, campos religados no topo do braço —, com a única diferença de
/// o corpo ser uma expressão, e não uma lista de comandos. Cada braço é um
/// bloco (`{ let ..; exp }`) e não só a expressão, porque as religações
/// precisam de um lugar onde morar; sem campo usado o bloco sai com a
/// expressão sozinha, que é o que o rustc quer ver quando não há `let`.
///
/// Em posição de **operando** o `match` sai parentetizado, pela mesma razão
/// que o bloco de [`emit_foreign_call`]: o rustc recusa um `match` cru como
/// operando de `+`, e em posição de comando um `{` inicial seria lido como
/// bloco-statement. Em posição já **delimitada** (valor de `let`, argumento,
/// `return`) os parênteses viram `unused_parens`, e `parentetizado` é `false`
/// — a mesma divisão que [`emit_exp`] e [`emit_delimited_exp`] já fazem para
/// binop, unop e `as`.
fn emit_match_exp(
    scrutinee: &TypedExp,
    arms: &[TypedMatchArm<TypedExp>],
    parentetizado: bool,
    ctx: Ctx,
) -> String {
    let mut braços = Vec::with_capacity(arms.len());
    for arm in arms {
        let mut ligacoes = String::new();
        emit_arm_bindings(&mut ligacoes, &arm.pattern, &arm.body, 0, ctx);
        // As ligações saem de `emit_arm_bindings` uma por linha, com `\n` no
        // fim; numa expressão inline o que se quer é um espaço entre elas.
        let ligacoes = ligacoes.replace('\n', " ");
        let corpo = emit_slot_value(&arm.body.ty, &arm.body, ctx);
        braços.push(format!(
            "{} => {{ {ligacoes}{corpo} }}",
            emit_pattern(&arm.pattern, &arm.body)
        ));
    }
    let nu = format!("match &{} {{ {} }}", emit_exp(scrutinee, ctx), braços.join(" "));
    if parentetizado {
        format!("({nu})")
    } else {
        nu
    }
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
        // `enum Nome` (T77) → o `enum` Rust de mesmo nome, exatamente como o
        // record: os dois são tipos nominais no mesmo namespace (ADR 0009).
        // Sem recursão sobre as variantes, e não é só economia: um `Sum`
        // aninhado chega do checker como placeholder de variantes vazias, e o
        // nome é tudo de que a emissão precisa.
        Type::Sum { name, .. } => name.clone(),
        // `T?` (T69) → `Option<T>`. O braço que a T68 deixou faltando: até
        // aqui um tipo opcional caía no `unreachable!` abaixo, e por isso
        // `generate` tinha de recusar o programa inteiro antes de emitir
        // qualquer coisa. O `base` passa por esta mesma função, então
        // composto dentro de opcional sai `Option<Vec<i64>>` e segue o ADR
        // 0006/0007 como qualquer outro composto.
        Type::Option { base } => format!("Option<{}>", rust_type_name(base)),
        // `value` (T70) → o enum boxado do runtime. A T25 rejeitava o tipo no
        // checker justamente porque este braço não existia e o
        // `unreachable!` abaixo viraria panic.
        Type::Value => "titan_runtime::Value".to_string(),
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

    // ---- T71: `for`-in como `for` nativo do Rust sobre `.iter()` --------

    /// O `for`-in **não** reusa o template de `loop` do `for` numérico (ADR
    /// 0022/0024): emite o `for` nativo do Rust, onde o iterador já avança
    /// sozinho antes de cada volta.
    #[test]
    fn for_in_sobre_array_emite_for_nativo_do_rust() {
        let source = r#"function main(args: {string}): integer
    local v: {integer} = {1, 2}
    local s: integer = 0
    for x in v do
        s = s + x
    end
    return 0
end"#;
        let rust = generate_source(source);
        assert!(rust.contains("for titan_forin_x in v.iter() {"), "{rust}");
        // O nome do usuário nasce por valor dentro do corpo — escalar
        // desreferencia, sem clone.
        assert!(rust.contains("let x: i64 = *titan_forin_x;"), "{rust}");
        // Nenhuma peça do template do `for` numérico aparece.
        assert!(!rust.contains("titan_for_inc"), "{rust}");
        assert!(!rust.contains("titan_for_primeira"), "{rust}");
        // `.iter()`, nunca `.iter_mut()`: o checker já recusou mutação.
        assert!(!rust.contains("iter_mut"), "{rust}");
    }

    /// Sobre um map o iterador liga um par, e cada metade vira um nome do
    /// usuário — a chave `String` **clona** (ADR 0006), o valor escalar não.
    #[test]
    fn for_in_sobre_map_emite_par_e_clona_so_a_chave() {
        let source = r#"function main(args: {string}): integer
    local m: {string: integer} = {["a"] = 1}
    local s: integer = 0
    local t: string = ""
    for k, n in m do
        s = s + n
        t = k
    end
    return 0
end"#;
        let rust = generate_source(source);
        assert!(
            rust.contains("for (titan_forin_k, titan_forin_n) in m.iter() {"),
            "{rust}"
        );
        assert!(
            rust.contains("let k: String = titan_forin_k.clone();"),
            "{rust}"
        );
        assert!(rust.contains("let n: i64 = *titan_forin_n;"), "{rust}");
    }

    /// Nome que o corpo nunca lê é descartado com `_` **no padrão** do
    /// iterador, e não ganha ligação nenhuma — senão o Rust gerado sairia com
    /// `unused_variables`, contra o critério herdado da T69.
    #[test]
    fn nome_nao_lido_do_for_in_vira_underscore_no_padrao() {
        let source = r#"function main(args: {string}): integer
    local m: {string: integer} = {["a"] = 1}
    local s: integer = 0
    for k, n in m do
        s = s + n
    end
    return 0
end"#;
        let rust = generate_source(source);
        assert!(
            rust.contains("for (_, titan_forin_n) in m.iter() {"),
            "{rust}"
        );
        assert!(!rust.contains("let k:"), "{rust}");
        assert!(!rust.contains("titan_forin_k"), "{rust}");
    }

    /// Elemento composto entra clonado, para que escrever na variável do laço
    /// não alcance o container (ADR 0006/0024).
    #[test]
    fn elemento_composto_do_for_in_entra_clonado() {
        let source = r#"function main(args: {string}): integer
    local matriz: {{integer}} = {{1, 2}}
    local s: integer = 0
    for linha in matriz do
        s = s + linha[1]
    end
    return 0
end"#;
        let rust = generate_source(source);
        assert!(
            rust.contains("let linha: Vec<i64> = titan_forin_linha.clone();"),
            "{rust}"
        );
    }

    /// Escrever na variável do laço é aceito pelo checker (ela é
    /// `SymbolKind::ForVar`, como a do `for` numérico), e a ligação precisa
    /// sair `let mut` — senão o Rust gerado tem um `x = ...` para um `x` que
    /// não foi declarado, e o `rustc` recusa o programa em inglês. É
    /// atribuição a uma **cópia**: não alcança o container (ADR 0024).
    #[test]
    fn atribuir_a_variavel_do_for_in_emite_let_mut_e_nao_toca_o_container() {
        let source = r#"function main(args: {string}): integer
    local v: {integer} = {1, 2}
    local s: integer = 0
    for x in v do
        x = x * 2
        s = s + x
    end
    print("s: " .. s)
    print("v1: " .. v[1])
    return 0
end"#;
        let rust = generate_source(source);
        assert!(rust.contains("let mut x: i64 = *titan_forin_x;"), "{rust}");
        let (avisos, output) = compila_e_executa(&rust, "for-in-atribui-t71");
        assert!(
            avisos.is_empty(),
            "warnings no Rust gerado:\n{avisos}\n{rust}"
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        // 1*2 + 2*2 = 6, e o container segue intacto.
        assert_eq!(stdout, "s: 6\nv1: 1\n", "stdout: {stdout}");
    }

    /// Variável só escrita, nunca lida, ainda precisa da ligação: é o caso
    /// que sairia como `x = 5;` sem declaração nenhuma se o codegen olhasse
    /// apenas as **leituras** do corpo.
    #[test]
    fn variavel_do_for_in_apenas_escrita_ainda_ganha_ligacao() {
        let source = r#"function main(args: {string}): integer
    local v: {integer} = {1, 2}
    for x in v do
        x = 5
    end
    return 0
end"#;
        let rust = generate_source(source);
        assert!(rust.contains("for titan_forin_x in v.iter() {"), "{rust}");
        assert!(rust.contains("let mut x: i64 = *titan_forin_x;"), "{rust}");
        // Nunca um `x = 5;` órfão, que o rustc recusaria em inglês.
        assert!(!rust.contains("for _ in v.iter()"), "{rust}");
    }

    /// Ponta a ponta com o `rustc` de verdade: as duas formas, `break` e
    /// `continue` dentro, e — o critério herdado da T69 — **zero warnings**.
    #[test]
    fn for_in_compila_sem_warnings_e_roda() {
        let source = r#"function main(args: {string}): integer
    local v: {integer} = {10, 20, 30}
    local soma: integer = 0
    for x in v do
        soma = soma + x
    end
    print("soma: " .. soma)
    local m: {string: integer} = {["a"] = 5, ["b"] = 7}
    local total: integer = 0
    for k, n in m do
        total = total + n
    end
    print("total: " .. total)
    for y in v do
        if y == 20 then
            continue
        end
        if y == 30 then
            break
        end
        print("bc: " .. y)
    end
    return 0
end"#;
        let rust = generate_source(source);
        let (avisos, output) = compila_e_executa(&rust, "for-in-t71");
        assert!(
            avisos.is_empty(),
            "warnings no Rust gerado:\n{avisos}\n{rust}"
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout, "soma: 60\ntotal: 12\nbc: 10\n", "stdout: {stdout}");
        assert_eq!(output.status.code(), Some(0));
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
        let ctx = EmitCtx {
            params: HashSet::new(),
            boxed: BoxedFields::new(),
        };
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

    // ---- T70: cast `as` --------------------------------------------------

    /// O critério de aceite da T70, por **execução real**: `1 as float` é
    /// 1.0, `3.9 as integer` é 3 e `-3.9 as integer` é -3 — truncagem em
    /// direção a zero, não piso. O `//` da T61 daria -4 para o mesmo número,
    /// e é justamente essa diferença que o README documenta.
    #[test]
    fn t70_cast_numerico_executa_e_trunca_em_direcao_a_zero() {
        let rust = generate_source(
            "function main(args: {string}): integer\n\
             \x20   local a: float = 1 as float\n\
             \x20   local b: integer = 3.9 as integer\n\
             \x20   local c: integer = -3.9 as integer\n\
             \x20   local d: integer = -3 // 2\n\
             \x20   print(\"a-\" .. a)\n\
             \x20   print(\"b-\" .. b)\n\
             \x20   print(\"c-\" .. c)\n\
             \x20   print(\"d-\" .. d)\n\
             \x20   return 0\n\
             end",
        );

        let (avisos, output) = compila_e_executa(&rust, "t70_numerico");
        assert!(
            avisos.is_empty(),
            "warnings no Rust gerado:\n{avisos}\n{rust}"
        );
        // `c` trunca (-3) enquanto `d`, que é o `//`, faz piso (-2 seria
        // truncagem; -2 é o piso de -1.5). Os dois lado a lado provam que as
        // duas operações não são a mesma.
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "a-1\nb-3\nc--3\nd--2\n"
        );
        assert_eq!(output.status.code(), Some(0));
    }

    /// Subida e descida de `value` com primitivas, ponta a ponta.
    #[test]
    fn t70_value_sobe_e_desce_com_primitivas() {
        let rust = generate_source(
            "function main(args: {string}): integer\n\
             \x20   local i: value = 42 as value\n\
             \x20   local s: value = \"oi\" as value\n\
             \x20   local f: value = 2.5 as value\n\
             \x20   local b: value = true as value\n\
             \x20   print(\"i-\" .. (i as integer))\n\
             \x20   print(\"s-\" .. (s as string))\n\
             \x20   print(\"f-\" .. (f as float))\n\
             \x20   if (b as boolean) then\n\
             \x20       print(\"b-sim\")\n\
             \x20   end\n\
             \x20   return 0\n\
             end",
        );

        let (avisos, output) = compila_e_executa(&rust, "t70_value_primitivas");
        assert!(
            avisos.is_empty(),
            "warnings no Rust gerado:\n{avisos}\n{rust}"
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "i-42\ns-oi\nf-2.5\nb-sim\n"
        );
        assert_eq!(output.status.code(), Some(0));
    }

    /// Composto e record sobem para `value` copiando elemento a elemento — e
    /// o original continua utilizável depois (ADR 0006: converter copia).
    #[test]
    fn t70_composto_e_record_sobem_para_value() {
        let rust = generate_source(
            "record Ponto\n\
             \x20   x: integer\n\
             \x20   y: integer\n\
             end\n\
             function usa(v: value): integer\n\
             \x20   return 1\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   local a: {integer} = {1, 2, 3}\n\
             \x20   local m: {string: integer} = {[\"a\"] = 1}\n\
             \x20   local p: Ponto = {x = 1, y = 2}\n\
             \x20   local n: integer = usa(a as value) + usa(m as value) + usa(p as value)\n\
             \x20   print(\"n-\" .. n)\n\
             \x20   print(\"a-\" .. a[1])\n\
             \x20   print(\"p-\" .. p.x)\n\
             \x20   return 0\n\
             end",
        );

        let (avisos, output) = compila_e_executa(&rust, "t70_value_composto");
        assert!(
            avisos.is_empty(),
            "warnings no Rust gerado:\n{avisos}\n{rust}"
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout), "n-3\na-1\np-1\n");
        assert_eq!(output.status.code(), Some(0));
    }

    /// Descer para o tipo errado aborta em português, com código 1 e **sem**
    /// `panic!` cru do Rust vazando para o usuário.
    #[test]
    fn t70_descida_de_value_errada_aborta_em_portugues() {
        let rust = generate_source(
            "function main(args: {string}): integer\n\
             \x20   local s: value = \"oi\" as value\n\
             \x20   print(\"n-\" .. (s as integer))\n\
             \x20   return 0\n\
             end",
        );

        let (_, output) = compila_e_executa(&rust, "t70_value_descida_errada");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("`value` não guarda um integer: guarda um string"),
            "stderr inesperado: {stderr}"
        );
        assert!(!stderr.contains("panicked"), "vazou panic do Rust: {stderr}");
        assert_eq!(output.status.code(), Some(1));
    }

    /// `value` atravessa parâmetro e retorno, e o cast encadeia sem
    /// parênteses.
    #[test]
    fn t70_value_atravessa_funcao_e_cast_encadeia() {
        let rust = generate_source(
            "function embrulha(n: integer): value\n\
             \x20   return n as value\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   local r: integer = embrulha(9) as integer\n\
             \x20   local e: value = 3 as float as value\n\
             \x20   print(\"r-\" .. r)\n\
             \x20   print(\"e-\" .. (e as float))\n\
             \x20   return 0\n\
             end",
        );

        let (avisos, output) = compila_e_executa(&rust, "t70_value_funcao");
        assert!(
            avisos.is_empty(),
            "warnings no Rust gerado:\n{avisos}\n{rust}"
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout), "r-9\ne-3\n");
        assert_eq!(output.status.code(), Some(0));
    }

    /// `T?` sobe para `value` nos dois estados: preenchido vira
    /// `Value::Option`, vazio vira `Value::Nil` — `value` tem um "ausente" só.
    #[test]
    fn t70_opcional_sobe_para_value_nos_dois_estados() {
        let rust = generate_source(
            "function usa(v: value): integer\n\
             \x20   return 1\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   local cheio: integer? = 7\n\
             \x20   local vazio: integer? = nil\n\
             \x20   local n: integer = usa(cheio as value) + usa(vazio as value)\n\
             \x20   print(\"n-\" .. n)\n\
             \x20   return 0\n\
             end",
        );

        let (avisos, output) = compila_e_executa(&rust, "t70_value_opcional");
        assert!(
            avisos.is_empty(),
            "warnings no Rust gerado:\n{avisos}\n{rust}"
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout), "n-2\n");
        assert_eq!(output.status.code(), Some(0));
    }

    /// O cast de identidade não emite conversão nenhuma — nem `as i64`, nem
    /// chamada de runtime.
    #[test]
    fn t70_cast_de_identidade_nao_emite_conversao() {
        let rust = generate_source(
            "function main(args: {string}): integer\n\
             \x20   local x: integer = 5 as integer\n\
             \x20   return x\n\
             end",
        );
        assert!(rust.contains("let x: i64 = 5;"), "{rust}");
    }

    // ---- T73: `foreign function` -----------------------------------------

    #[test]
    fn t73_foreign_function_emite_bloco_extern_c() {
        let rust = generate_source(
            "foreign function abs(n: integer): integer\n\n\
             function main(args: {string}): integer\n\
             \x20   return abs(-7)\n\
             end",
        );
        assert!(
            rust.contains("unsafe extern \"C\" {\n    fn abs(n: i64) -> i64;\n}"),
            "{rust}"
        );
    }

    /// O nome externo **não** passa pelo mangling: é o símbolo que o linker
    /// procura em libc. O `titan_` só vale para funções escritas em Titan.
    #[test]
    fn t73_foreign_function_nao_sofre_mangling() {
        let rust = generate_source(
            "foreign function abs(n: integer): integer\n\n\
             function main(args: {string}): integer\n\
             \x20   return abs(-7)\n\
             end",
        );
        assert!(rust.contains("fn abs(n: i64) -> i64;"), "{rust}");
        assert!(!rust.contains("titan_abs"), "{rust}");
        // A função Titan ao lado continua manglada, para o contraste ficar
        // registrado em vez de subentendido.
        assert!(rust.contains("pub fn titan_main"), "{rust}");
    }

    #[test]
    fn t73_chamada_a_foreign_function_sai_dentro_de_unsafe() {
        let rust = generate_source(
            "foreign function abs(n: integer): integer\n\n\
             function main(args: {string}): integer\n\
             \x20   return abs(-7)\n\
             end",
        );
        assert!(rust.contains("unsafe { abs(-7) }"), "{rust}");
    }

    /// Sem argumento `string`, nenhuma ligação temporária aparece — o
    /// `unsafe { ... }` sai limpo.
    #[test]
    fn t73_chamada_sem_string_nao_gera_ligacao_temporaria() {
        let rust = generate_source(
            "foreign function abs(n: integer): integer\n\n\
             function main(args: {string}): integer\n\
             \x20   return abs(-7)\n\
             end",
        );
        assert!(!rust.contains("__titan_ffi_"), "{rust}");
    }

    /// `string` na fronteira vira `*const c_char` no `extern`, e a `CString`
    /// que o alimenta fica presa a uma ligação `let` — um `.as_ptr()` sobre
    /// temporário seria ponteiro pendurado.
    #[test]
    fn t73_string_na_fronteira_vira_c_char_com_cstring_viva() {
        let rust = generate_source(
            "foreign function strlen(s: string): integer\n\n\
             function main(args: {string}): integer\n\
             \x20   return strlen(\"titan\")\n\
             end",
        );
        assert!(
            rust.contains("fn strlen(s: *const std::os::raw::c_char) -> i64;"),
            "{rust}"
        );
        assert!(
            rust.contains("let __titan_ffi_0 = titan_runtime::ffi_cstring("),
            "{rust}"
        );
        assert!(rust.contains("strlen(__titan_ffi_0.as_ptr())"), "{rust}");
    }

    /// Retorno `string` volta como ponteiro e é convertido dentro do mesmo
    /// `unsafe` — `ffi_string` checa o nulo e aborta em português.
    #[test]
    fn t73_retorno_string_passa_por_ffi_string() {
        let rust = generate_source(
            "foreign function getenv(nome: string): string\n\n\
             function main(args: {string}): integer\n\
             \x20   print(getenv(\"PATH\"))\n\
             \x20   return 0\n\
             end",
        );
        assert!(
            rust.contains(
                "fn getenv(nome: *const std::os::raw::c_char) -> *const std::os::raw::c_char;"
            ),
            "{rust}"
        );
        assert!(rust.contains("titan_runtime::ffi_string(getenv("), "{rust}");
    }

    /// Retorno omitido é o `void` do C: nenhum `->` no `extern`.
    #[test]
    fn t73_foreign_function_sem_retorno_nao_emite_seta() {
        let rust = generate_source(
            "foreign function sync()\n\n\
             function main(args: {string}): integer\n\
             \x20   sync()\n\
             \x20   return 0\n\
             end",
        );
        assert!(rust.contains("fn sync();"), "{rust}");
    }

    /// O critério de aceite da T73: um `.titan` que chama a libc compila com
    /// `rustc` de verdade e imprime o valor certo. `abs(-7)` é `7`.
    #[test]
    fn t73_chamada_a_libc_compila_e_roda_com_o_valor_correto() {
        let rust = generate_source(
            "foreign function abs(n: integer): integer\n\n\
             function main(args: {string}): integer\n\
             \x20   print(\"abs=\" .. abs(-7))\n\
             \x20   return 0\n\
             end",
        );

        let (avisos, output) = compila_e_executa(&rust, "t73_abs");
        assert!(
            avisos.is_empty(),
            "warnings no Rust gerado:\n{avisos}\n{rust}"
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout), "abs=7\n");
        assert_eq!(output.status.code(), Some(0));
    }

    /// A ponta `string` da fronteira, ida e volta, também contra a libc de
    /// verdade: `strlen("titan")` é `5`.
    #[test]
    fn t73_string_na_fronteira_compila_e_roda_contra_a_libc() {
        let rust = generate_source(
            "foreign function strlen(s: string): integer\n\n\
             function main(args: {string}): integer\n\
             \x20   print(\"n=\" .. strlen(\"titan\"))\n\
             \x20   return 0\n\
             end",
        );

        let (avisos, output) = compila_e_executa(&rust, "t73_strlen");
        assert!(
            avisos.is_empty(),
            "warnings no Rust gerado:\n{avisos}\n{rust}"
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout), "n=5\n");
        assert_eq!(output.status.code(), Some(0));
    }

    /// `float` na fronteira é o `double` do C — `sqrt` da libm prova os dois
    /// sentidos de uma vez (argumento e retorno).
    #[test]
    fn t73_float_na_fronteira_compila_e_roda_contra_a_libm() {
        let rust = generate_source(
            "foreign function sqrt(x: float): float\n\n\
             function main(args: {string}): integer\n\
             \x20   print(\"r=\" .. sqrt(9.0))\n\
             \x20   return 0\n\
             end",
        );
        assert!(rust.contains("fn sqrt(x: f64) -> f64;"), "{rust}");

        let (avisos, output) = compila_e_executa(&rust, "t73_sqrt");
        assert!(
            avisos.is_empty(),
            "warnings no Rust gerado:\n{avisos}\n{rust}"
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout), "r=3\n");
        assert_eq!(output.status.code(), Some(0));
    }

    /// Chamadas aninhadas, as duas com argumento `string`: os dois blocos
    /// nomeiam a ligação `__titan_ffi_0`, e isso é correto — a ligação
    /// interna vive só dentro do bloco que é o inicializador da externa, e
    /// morre antes de a externa nascer. O caso existe como teste porque a
    /// colisão *parece* um problema à primeira leitura.
    #[test]
    fn t73_chamadas_aninhadas_com_string_compilam_e_rodam() {
        let rust = generate_source(
            "foreign function strlen(s: string): integer\n\
             foreign function getenv(nome: string): string\n\n\
             function main(args: {string}): integer\n\
             \x20   print(\"n=\" .. strlen(getenv(\"PATH\")))\n\
             \x20   return 0\n\
             end",
        );

        let (_, output) = compila_e_executa(&rust, "t73_aninhada");
        assert_eq!(output.status.code(), Some(0));
        assert!(
            String::from_utf8_lossy(&output.stdout).starts_with("n="),
            "obteve: {}",
            String::from_utf8_lossy(&output.stdout)
        );
    }

    /// Uma chamada em posição de statement (valor descartado) com argumento
    /// `string`: é o caso que os parênteses em volta do bloco protegem — sem
    /// eles o `{ ... };` seria lido como bloco-statement seguido de statement
    /// vazio.
    #[test]
    fn t73_chamada_com_string_em_posicao_de_statement_compila() {
        let rust = generate_source(
            "foreign function strlen(s: string): integer\n\n\
             function main(args: {string}): integer\n\
             \x20   strlen(\"titan\")\n\
             \x20   return 0\n\
             end",
        );

        let (_, output) = compila_e_executa(&rust, "t73_strlen_stmt");
        assert_eq!(output.status.code(), Some(0));
    }

    /// O ciclo mais longo que a travessia de [`campos_boxeados`] tem de
    /// fechar: record → record → enum → record. Nenhum ciclo só de records
    /// chega aqui (o checker o rejeita), então é sempre um `enum` que fecha a
    /// volta — e é o `visitados` do braço `Sum` que faz a busca terminar.
    ///
    /// Sem a guarda, este fonte faria o codegen girar para sempre; com ela,
    /// os dois campos de variante saem encaixotados e os `struct` ficam com
    /// tamanho finito, o que o `rustc` confirma ao compilar.
    #[test]
    fn t77_ciclo_por_dois_records_termina_e_encaixota() {
        let rust = generate_source(
            "record A\n\
             \x20   e: E\n\
             end\n\
             record B\n\
             \x20   a: A\n\
             end\n\
             enum E\n\
             \x20   ENil\n\
             \x20   EB(B)\n\
             \x20   EA(A)\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   return 0\n\
             end",
        );

        assert!(rust.contains("EB(Box<B>)"), "gerado: {rust}");
        assert!(rust.contains("EA(Box<A>)"), "gerado: {rust}");

        let (avisos, output) = compila_e_executa(&rust, "t77_ciclo_longo");
        assert!(avisos.is_empty(), "warnings:\n{avisos}\n{rust}");
        assert_eq!(output.status.code(), Some(0));
    }

    /// **O critério de aceite da Parte A** (PRD.md, T77): uma mini-AST
    /// recursiva construída e avaliada por `match` recursivo imprime o
    /// resultado correto, e o Rust gerado compila **sem warnings**.
    ///
    /// É o caso que justifica a fase inteira: `ExpBinop(string, Exp, Exp)`
    /// só existe em Rust porque a emissão encaixota os dois campos
    /// recursivos.
    #[test]
    fn t77_mini_ast_recursiva_avalia_pelo_match_e_imprime_o_resultado() {
        let rust = generate_source(
            "enum Exp\n\
             \x20   ExpInteger(integer)\n\
             \x20   ExpBinop(string, Exp, Exp)\n\
             end\n\
             function aplica(op: string, a: integer, b: integer): integer\n\
             \x20   if op == \"+\" then\n\
             \x20       return a + b\n\
             \x20   end\n\
             \x20   if op == \"*\" then\n\
             \x20       return a * b\n\
             \x20   end\n\
             \x20   return 0\n\
             end\n\
             function avalia(e: Exp): integer\n\
             \x20   local r: integer = match e with\n\
             \x20       ExpInteger(n) then\n\
             \x20           n\n\
             \x20       ExpBinop(op, l, r) then\n\
             \x20           aplica(op, avalia(l), avalia(r))\n\
             \x20   end\n\
             \x20   return r\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   local e: Exp = ExpBinop(\"+\", ExpInteger(2), ExpBinop(\"*\", ExpInteger(3), ExpInteger(4)))\n\
             \x20   print(\"resultado: \" .. avalia(e))\n\
             \x20   return 0\n\
             end",
        );

        // O `Box` só nos dois campos recursivos: a `string` do operador
        // continua `String`.
        assert!(
            rust.contains("ExpBinop(String, Box<Exp>, Box<Exp>)"),
            "gerado: {rust}"
        );

        let (avisos, output) = compila_e_executa(&rust, "t77_mini_ast");
        assert!(
            avisos.is_empty(),
            "warnings no Rust gerado:\n{avisos}\n{rust}"
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "resultado: 14\n",
            "gerado: {rust}"
        );
    }

    /// O ciclo **indireto** que a checagem de record não vê (`checker.rs` só
    /// olha record → record): `enum Exp ExpNo(Caixa) end` com
    /// `record Caixa e: Exp end` é um tamanho infinito em Rust tanto quanto
    /// o ciclo direto, e é o campo da **variante** que ganha o `Box` —
    /// encaixotar o campo do record mudaria o tipo que todo `c.e` do
    /// programa enxerga.
    #[test]
    fn t77_ciclo_indireto_por_record_encaixota_o_campo_da_variante() {
        let rust = generate_source(
            "record Caixa\n\
             \x20   e: Exp\n\
             \x20   peso: integer\n\
             end\n\
             enum Exp\n\
             \x20   ExpNil\n\
             \x20   ExpNo(Caixa)\n\
             end\n\
             function peso(e: Exp): integer\n\
             \x20   local r: integer = match e with\n\
             \x20       ExpNil then\n\
             \x20           0\n\
             \x20       ExpNo(c) then\n\
             \x20           c.peso + peso(c.e)\n\
             \x20   end\n\
             \x20   return r\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   local cf: Caixa = {e = ExpNil, peso = 5}\n\
             \x20   local folha: Exp = ExpNo(cf)\n\
             \x20   local cr: Caixa = {e = folha, peso = 7}\n\
             \x20   local raiz: Exp = ExpNo(cr)\n\
             \x20   print(\"peso: \" .. peso(raiz))\n\
             \x20   return 0\n\
             end",
        );

        assert!(rust.contains("ExpNo(Box<Caixa>)"), "gerado: {rust}");
        // O campo do record **não** é encaixotado: `Box` no lado do enum
        // basta para o tamanho fechar, e o record segue com o tipo que o
        // resto do programa lê em `c.e`.
        assert!(rust.contains("pub e: Exp,"), "gerado: {rust}");

        let (avisos, output) = compila_e_executa(&rust, "t77_ciclo_indireto");
        assert!(avisos.is_empty(), "warnings:\n{avisos}\n{rust}");
        assert_eq!(String::from_utf8_lossy(&output.stdout), "peso: 12\n");
    }

    /// Recursão **mútua** entre dois enums: o `Sum` aninhado chega do checker
    /// como placeholder de variantes vazias, então só a tabela do programa
    /// mostra que `A` volta a `A` passando por `B` — é a razão de
    /// [`campos_boxeados`] atravessar `Sum` pelo nome, e não pelas variantes
    /// embutidas no tipo.
    #[test]
    fn t77_recursao_mutua_entre_enums_encaixota_os_dois_lados() {
        let rust = generate_source(
            "enum A\n\
             \x20   ANil\n\
             \x20   AB(B)\n\
             end\n\
             enum B\n\
             \x20   BNil\n\
             \x20   BA(A)\n\
             end\n\
             function fundo(a: A): integer\n\
             \x20   local r: integer = match a with\n\
             \x20       ANil then\n\
             \x20           0\n\
             \x20       AB(b) then\n\
             \x20           1 + fundo_b(b)\n\
             \x20   end\n\
             \x20   return r\n\
             end\n\
             function fundo_b(b: B): integer\n\
             \x20   local r: integer = match b with\n\
             \x20       BNil then\n\
             \x20           0\n\
             \x20       BA(a) then\n\
             \x20           1 + fundo(a)\n\
             \x20   end\n\
             \x20   return r\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   local folha: B = BA(ANil)\n\
             \x20   print(\"fundo: \" .. fundo(AB(folha)))\n\
             \x20   return 0\n\
             end",
        );

        assert!(rust.contains("AB(Box<B>)"), "gerado: {rust}");
        assert!(rust.contains("BA(Box<A>)"), "gerado: {rust}");

        let (avisos, output) = compila_e_executa(&rust, "t77_mutua");
        assert!(avisos.is_empty(), "warnings:\n{avisos}\n{rust}");
        assert_eq!(String::from_utf8_lossy(&output.stdout), "fundo: 2\n");
    }

    /// O outro lado da armadilha: `{Exp}` **não** leva `Box`. `Vec<Exp>` já
    /// põe os elementos no heap, então o tamanho de `Exp` não depende do
    /// deles — encaixotar compilaria e só acrescentaria uma alocação por
    /// elemento.
    #[test]
    fn t77_recursao_por_array_nao_leva_box() {
        let rust = generate_source(
            "enum Exp\n\
             \x20   ExpFolha(integer)\n\
             \x20   ExpNo({Exp})\n\
             end\n\
             function soma(e: Exp): integer\n\
             \x20   local total: integer = 0\n\
             \x20   match e with\n\
             \x20       ExpFolha(n) then\n\
             \x20           total = n\n\
             \x20       ExpNo(filhos) then\n\
             \x20           for filho in filhos do\n\
             \x20               total = total + soma(filho)\n\
             \x20           end\n\
             \x20   end\n\
             \x20   return total\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   local filhos: {Exp} = {ExpFolha(2), ExpFolha(3)}\n\
             \x20   print(\"soma: \" .. soma(ExpNo(filhos)))\n\
             \x20   return 0\n\
             end",
        );

        assert!(rust.contains("ExpNo(Vec<Exp>)"), "gerado: {rust}");
        assert!(!rust.contains("Box<"), "não deveria encaixotar: {rust}");

        let (avisos, output) = compila_e_executa(&rust, "t77_array");
        assert!(avisos.is_empty(), "warnings:\n{avisos}\n{rust}");
        assert_eq!(String::from_utf8_lossy(&output.stdout), "soma: 5\n");
    }

    /// Semântica de valor (ADR 0006) valendo para tipo soma: o escrutinado
    /// sai emprestado (`match &e`) e a atribuição clona, então `local b = a`
    /// seguido de `match a` compila — com `match a` por valor, o padrão que
    /// liga campos moveria `a` e o rustc recusaria em inglês.
    #[test]
    fn t77_escrutinado_emprestado_mantem_o_valor_vivo_depois_do_match() {
        let rust = generate_source(
            "enum Exp\n\
             \x20   ExpNil\n\
             \x20   ExpTexto(string)\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   local a: Exp = ExpTexto(\"oi\")\n\
             \x20   local b: Exp = a\n\
             \x20   local vezes: integer = 0\n\
             \x20   match a with\n\
             \x20       ExpNil then\n\
             \x20           vezes = vezes - 1\n\
             \x20       ExpTexto(s) then\n\
             \x20           print(s)\n\
             \x20           vezes = vezes + 1\n\
             \x20   end\n\
             \x20   match b with\n\
             \x20       ExpNil then\n\
             \x20           vezes = vezes - 1\n\
             \x20       ExpTexto(s) then\n\
             \x20           print(s)\n\
             \x20           vezes = vezes + 1\n\
             \x20   end\n\
             \x20   print(\"vezes: \" .. vezes)\n\
             \x20   return 0\n\
             end",
        );

        assert!(rust.contains("match &a {"), "gerado: {rust}");
        assert!(rust.contains("let b: Exp = a.clone();"), "gerado: {rust}");

        let (avisos, output) = compila_e_executa(&rust, "t77_valor");
        assert!(avisos.is_empty(), "warnings:\n{avisos}\n{rust}");
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "oi\noi\nvezes: 2\n"
        );
    }

    /// Um argumento de tipo soma é dono de buffer próprio como record, mas
    /// segue por **valor** como escalar (não entra em `is_composite`, ADR
    /// 0007) — então a chamada tem de clonar a fonte que sobrevive a ela.
    /// Sem o clone, `tinge(c) + tinge(c)` moveria `c` na primeira chamada.
    #[test]
    fn t77_argumento_de_tipo_soma_clona_a_fonte_que_sobrevive_a_chamada() {
        let rust = generate_source(
            "enum Cor\n\
             \x20   Vermelho\n\
             \x20   Verde\n\
             end\n\
             function tinge(c: Cor): integer\n\
             \x20   local r: integer = match c with\n\
             \x20       Vermelho then\n\
             \x20           1\n\
             \x20       Verde then\n\
             \x20           2\n\
             \x20   end\n\
             \x20   return r\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   local c: Cor = Verde\n\
             \x20   print(\"dobro: \" .. tinge(c) + tinge(c))\n\
             \x20   return 0\n\
             end",
        );

        assert!(rust.contains("titan_tinge(c.clone())"), "gerado: {rust}");

        let (avisos, output) = compila_e_executa(&rust, "t77_arg_clone");
        assert!(avisos.is_empty(), "warnings:\n{avisos}\n{rust}");
        assert_eq!(String::from_utf8_lossy(&output.stdout), "dobro: 4\n");
    }

    /// `_` é o `_` do Rust, e um campo que o corpo nunca usa sai como `_` no
    /// padrão: o escrutinado é emprestado, então a ligação não é descartável
    /// de graça — um nome ligado sem uso dispararia `unused_variables`, e o
    /// critério da tarefa é Rust sem warnings.
    #[test]
    fn t77_curinga_e_campo_sem_uso_saem_como_underscore() {
        let rust = generate_source(
            "enum Exp\n\
             \x20   ExpNil\n\
             \x20   ExpInteger(integer)\n\
             \x20   ExpTexto(string)\n\
             end\n\
             function classifica(e: Exp): integer\n\
             \x20   local r: integer = match e with\n\
             \x20       ExpInteger(n) then\n\
             \x20           n\n\
             \x20       _ then\n\
             \x20           0\n\
             \x20   end\n\
             \x20   return r\n\
             end\n\
             function ignora(e: Exp): integer\n\
             \x20   local r: integer = match e with\n\
             \x20       ExpTexto(s) then\n\
             \x20           1\n\
             \x20       _ then\n\
             \x20           0\n\
             \x20   end\n\
             \x20   return r\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   print(\"soma: \" .. classifica(ExpInteger(7)) + ignora(ExpNil))\n\
             \x20   return 0\n\
             end",
        );

        assert!(rust.contains("_ => { 0 }"), "gerado: {rust}");
        // `s` não é lido no corpo do braço: sai `_` no padrão, e sem `let`.
        assert!(rust.contains("Exp::ExpTexto(_) =>"), "gerado: {rust}");

        let (avisos, output) = compila_e_executa(&rust, "t77_curinga");
        assert!(avisos.is_empty(), "warnings:\n{avisos}\n{rust}");
        assert_eq!(String::from_utf8_lossy(&output.stdout), "soma: 7\n");
    }

    /// Campo composto ligado por um padrão é **mutável dentro do braço**: o
    /// checker o trata como parâmetro (não dá para atribuir ao nome inteiro),
    /// mas `c.peso = 9` é permitido — e escreve na cópia do braço, não no
    /// escrutinado, que o `match &e` manteve intocado.
    #[test]
    fn t77_campo_composto_ligado_e_mutavel_dentro_do_braco() {
        let rust = generate_source(
            "record Ponto\n\
             \x20   x: integer\n\
             end\n\
             enum Forma\n\
             \x20   Vazia\n\
             \x20   Um(Ponto)\n\
             end\n\
             function desloca(f: Forma): integer\n\
             \x20   local r: integer = -1\n\
             \x20   match f with\n\
             \x20       Vazia then\n\
             \x20           r = r + 1\n\
             \x20       Um(p) then\n\
             \x20           p.x = p.x + 10\n\
             \x20           r = p.x\n\
             \x20   end\n\
             \x20   return r\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   local p: Ponto = {x = 1}\n\
             \x20   local f: Forma = Um(p)\n\
             \x20   print(\"x: \" .. desloca(f) .. \"/\" .. desloca(f))\n\
             \x20   return 0\n\
             end",
        );

        assert!(rust.contains("let mut p: Ponto ="), "gerado: {rust}");

        let (avisos, output) = compila_e_executa(&rust, "t77_mut_ligado");
        assert!(avisos.is_empty(), "warnings:\n{avisos}\n{rust}");
        // O segundo `desloca(f)` vê o mesmo `1` do primeiro: a escrita ficou
        // na cópia do braço.
        assert_eq!(String::from_utf8_lossy(&output.stdout), "x: 11/11\n");
    }

    /// `match` como expressão em posição de **operando** precisa dos
    /// parênteses (`1 + (match ..)` — o rustc recusa o `match` cru ali), e em
    /// posição já delimitada precisa ficar **sem** eles (`unused_parens`). A
    /// divisão é a mesma que binop, unop e `as` já seguiam.
    #[test]
    fn t77_match_expressao_parentetiza_so_em_posicao_de_operando() {
        let rust = generate_source(
            "enum Cor\n\
             \x20   Vermelho\n\
             \x20   Verde\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   local c: Cor = Verde\n\
             \x20   local delimitado: integer = match c with\n\
             \x20       Vermelho then\n\
             \x20           1\n\
             \x20       Verde then\n\
             \x20           2\n\
             \x20   end\n\
             \x20   local operando: integer = 10 + match c with\n\
             \x20       Vermelho then\n\
             \x20           1\n\
             \x20       Verde then\n\
             \x20           2\n\
             \x20   end\n\
             \x20   print(\"soma: \" .. delimitado + operando)\n\
             \x20   return 0\n\
             end",
        );

        assert!(
            rust.contains("let delimitado: i64 = match &c {"),
            "gerado: {rust}"
        );
        assert!(rust.contains("10 + (match &c {"), "gerado: {rust}");

        let (avisos, output) = compila_e_executa(&rust, "t77_parens");
        assert!(avisos.is_empty(), "warnings:\n{avisos}\n{rust}");
        assert_eq!(String::from_utf8_lossy(&output.stdout), "soma: 14\n");
    }
}
