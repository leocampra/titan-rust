//! Análise semântica e verificação de tipos do Titan.
//!
//! Espelha a estratégia de `titan/titan-compiler/checker.lua` (1662 linhas),
//! reduzida ao subconjunto das Fases 0 e 1 (T5, T12 e T13 do PRD.md):
//!
//! - **Duas passadas**, como o Titan: (1) coleta as assinaturas top-level,
//!   permitindo chamada antes da declaração; (2) verifica os corpos,
//!   produzindo uma AST anotada com o tipo resolvido de cada `Exp`.
//! - Símbolos com escopo em pilha, espelhando `titan/titan-compiler/symtab.lua`.
//! - `print` é registrado no escopo global como
//!   `Function{params:[String], rettypes:[Nil]}`, originado do runtime — não é
//!   palavra-chave.
//! - A assinatura de `main` é validada: `main(args: {string}): integer`
//!   (`checker.lua:1593-1607`, `checker.has_main`).
//! - Statements da Fase 1: `if`/`while` com condição `boolean`
//!   (`checker.lua:365-368` e `447-457`), `for` numérico espelhando
//!   `checkfor` (`checker.lua:239-288`) e atribuição single-target
//!   (`checker.lua:378-410`).
//! - Operadores da Fase 1 (T13): regras de tipo espelhando
//!   `checker.lua:910-1122` (sem bitwise/gradual typing), com a coerção
//!   int→float centralizada em `numeric_result`. O checker **não** emite nó
//!   de cast: o codegen decide o `as f64` comparando o tipo do operando com
//!   o tipo do resultado.
//! - **Rastreio de mutabilidade** (decisão 6 da Fase 1): cada `local` recebe
//!   um id; atribuições registram o id do símbolo resolvido (mesmo espírito
//!   do `var._decl._assigned = true` do original) e um fix-up ao final do
//!   corpo da função seta `mutable` nos `TypedStat::Decl` correspondentes —
//!   o codegen (T14) emite `let mut` só quando há reatribuição.
//!
//! Como `ast::Exp` é um valor imutável (ao contrário do Lua, que anexa
//! `_type` dinamicamente ao nó), o checker produz uma **AST tipada paralela**
//! (`TypedProgram` e companhia) em vez de mutar a árvore original — é o que
//! `codegen.rs` (T6) vai consumir.
//!
//! Tudo fora do subconjunto (records, maps, arrays manipuláveis, `import`,
//! `foreign import`, métodos, retornos múltiplos, `repeat`, `Option`/`?`)
//! produz um erro semântico claro — nunca panic.

use std::collections::{HashMap, HashSet};

use crate::ast::{self, Args, Exp, Loc, Program, Stat, TopLevel, Var};
use crate::types::Type;

/// Erro semântico com posição (no espírito de `checker.typeerror`).
#[derive(Debug, Clone, PartialEq)]
pub struct CheckError {
    pub message: String,
    pub loc: Loc,
}

impl std::fmt::Display for CheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "erro de tipo (linha {}, coluna {}): {}",
            self.loc.line, self.loc.col, self.message
        )
    }
}

impl std::error::Error for CheckError {}

// ---- Símbolos em pilha (`symtab.lua`) ----------------------------------

/// Identificador único de uma declaração `local`, usado pelo rastreio de
/// mutabilidade (decisão 6 da Fase 1).
type DeclId = usize;

/// Como um nome foi introduzido no escopo — decide o que uma atribuição a
/// ele significa.
#[derive(Debug, Clone, PartialEq)]
enum SymbolKind {
    /// Função top-level ou do runtime (`print`). Atribuição é rejeitada
    /// ("trying to assign to a function", `checker.lua:401`).
    Global,
    /// Parâmetro de função. O original permite atribuir, mas aqui não há
    /// rastreio de `mut` para parâmetros (o fix-up só alcança
    /// `TypedStat::Decl`), e o Rust gerado não compilaria — rejeitado com
    /// erro claro até uma fase futura rastrear parâmetros também.
    Param,
    /// Variável de controle de `for`. Atribuição é permitida sem rastreio:
    /// ela é sempre `mut` no template desaçucarado do T15.
    ForVar,
    /// Local declarada com `local`; atribuições registram o `DeclId` para o
    /// fix-up de mutabilidade ao final do corpo da função.
    Local { decl_id: DeclId },
    /// Módulo trazido por `import` (T38). Não é um `Type` (decisão 7 da
    /// Fase 3): não pode ser anotação de variável nem alvo de atribuição —
    /// só existe para `data.f(...)`/`data.DataFrame` resolverem contra a
    /// tabela de capabilities (`capabilities.rs`, T37).
    Module { name: String },
}

/// Entrada da tabela de símbolos: o tipo, a origem do nome e onde foi
/// declarado (T49 — go-to-definition precisa do local da declaração, que a
/// symtab descartava antes de existir um consumidor).
#[derive(Debug, Clone, PartialEq)]
struct Symbol {
    ty: Type,
    kind: SymbolKind,
    def_loc: Loc,
}

/// Pilha de escopos léxicos. Cada bloco é um `HashMap` de nome → símbolo.
struct SymTab {
    blocks: Vec<HashMap<String, Symbol>>,
}

impl SymTab {
    fn new() -> Self {
        SymTab {
            blocks: vec![HashMap::new()],
        }
    }

    fn open_block(&mut self) {
        self.blocks.push(HashMap::new());
    }

    fn close_block(&mut self) {
        self.blocks.pop();
    }

    fn add_symbol(&mut self, name: &str, ty: Type, kind: SymbolKind, def_loc: Loc) {
        self.blocks
            .last_mut()
            .expect("symtab sempre tem pelo menos um bloco")
            .insert(
                name.to_string(),
                Symbol {
                    ty,
                    kind,
                    def_loc,
                },
            );
    }

    fn find_symbol(&self, name: &str) -> Option<&Symbol> {
        self.blocks.iter().rev().find_map(|block| block.get(name))
    }
}

// ---- AST tipada ---------------------------------------------------------

/// Programa já verificado: cada função top-level com seu tipo resolvido.
pub type TypedProgram = Vec<TypedTopLevel>;

#[derive(Debug, Clone, PartialEq)]
pub enum TypedTopLevel {
    Func {
        loc: Loc,
        islocal: bool,
        name: String,
        params: Vec<(String, Type)>,
        rettypes: Vec<Type>,
        /// `Box` para a variante não inflar `TypedTopLevel` inteiro (clippy
        /// `large_enum_variant`) — mesmo espírito do `Box` em
        /// `TypedStat::For.inc`.
        body: Box<TypedStat>,
    },
    /// Declaração de `record` (T25 — estrutural: T26 é quem passa a aceitar
    /// `record` na passada 1; até lá, `collect_signature` continua
    /// rejeitando-o com erro claro, e esta variante nunca é construída).
    Record {
        loc: Loc,
        name: String,
        fields: Vec<(String, Type)>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypedStat {
    Block {
        loc: Loc,
        stats: Vec<TypedStat>,
    },
    Decl {
        loc: Loc,
        name: String,
        ty: Type,
        value: TypedExp,
        /// Id interno da declaração, usado só pelo fix-up de mutabilidade —
        /// torna a correspondência atribuição → declaração explícita (e
        /// robusta a shadowing) em vez de depender da ordem de travessia.
        decl_id: usize,
        /// `true` quando alguma atribuição alcança esta declaração; o
        /// codegen (T14) emite `let mut` somente nesse caso.
        mutable: bool,
    },
    Call {
        loc: Loc,
        call: TypedExp,
    },
    Return {
        loc: Loc,
        exps: Vec<TypedExp>,
    },
    If {
        loc: Loc,
        thens: Vec<TypedThen>,
        elsestat: Option<Box<TypedStat>>,
    },
    While {
        loc: Loc,
        condition: TypedExp,
        block: Box<TypedStat>,
    },
    For {
        loc: Loc,
        name: String,
        ty: Type,
        start: TypedExp,
        finish: TypedExp,
        /// Sempre presente: quando omitido no fonte, vira `1`/`1.0` conforme
        /// o tipo da variável (como `checkfor`, `checker.lua:258-268`).
        /// `Box` para a variante não inflar o `TypedStat` inteiro
        /// (clippy `large_enum_variant`).
        inc: Box<TypedExp>,
        block: Box<TypedStat>,
    },
    Assign {
        loc: Loc,
        target: TypedLValue,
        value: TypedExp,
    },
}

/// Alvo de uma atribuição já verificado (T25 — estrutural; T29/T30 são quem
/// passam a construir `Index`/`Field`). `Name` é o único alvo que a passada 2
/// constrói nesta fase — `v[i] = x` e `p.campo = x` seguem rejeitados em
/// `check_assign` até essas tarefas.
#[derive(Debug, Clone, PartialEq)]
pub enum TypedLValue {
    Name(String),
    Index {
        base: Box<TypedExp>,
        index: Box<TypedExp>,
    },
    Field {
        base: Box<TypedExp>,
        name: String,
    },
}

/// Ramo `then` já verificado de um `TypedStat::If`.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedThen {
    pub loc: Loc,
    pub condition: TypedExp,
    pub block: TypedStat,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedExp {
    pub loc: Loc,
    pub ty: Type,
    pub kind: TypedExpKind,
}

/// A quem uma chamada (`TypedExpKind::Call`) resolve (T39). Três formas —
/// `f(x)`/`print(x)` direto, `data.read_csv(x)` qualificado por módulo
/// (T39) e `df.soma(x)` método sobre um tipo opaco (T40) — que o codegen
/// (T42) emite de três jeitos distintos.
#[derive(Debug, Clone, PartialEq)]
pub enum Callee {
    Direct(String),
    Module {
        module: String,
        name: String,
    },
    Method {
        recv: Box<TypedExp>,
        module: String,
        name: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypedExpKind {
    Nil,
    Bool(bool),
    Integer(i64),
    Float(f64),
    String(String),
    Var(String),
    Call {
        callee: Callee,
        args: Vec<TypedExp>,
    },
    Concat(Vec<TypedExp>),
    Binop {
        op: BinOp,
        lhs: Box<TypedExp>,
        rhs: Box<TypedExp>,
    },
    Unop {
        op: UnOp,
        exp: Box<TypedExp>,
    },
    /// `v[i]` (T25 — estrutural; T29 é quem passa a construir este nó em
    /// `check_var`, que hoje rejeita `VarBracket` com erro claro).
    Index {
        base: Box<TypedExp>,
        index: Box<TypedExp>,
    },
    /// `p.campo` (T25 — estrutural; T30 é quem passa a construir este nó em
    /// `check_var`, que hoje rejeita `VarDot` com erro claro).
    Field {
        base: Box<TypedExp>,
        name: String,
    },
    /// `{1, 2, 3}` desambiguado como array pelo checker (T25 — estrutural;
    /// T31 constrói). Três nós de literal distintos, não um `InitList`
    /// genérico, porque a desambiguação já aconteceu aqui — o codegen ganha
    /// `match` exaustivo em vez de reinspecionar os campos.
    ArrayLit(Vec<TypedExp>),
    /// `Nome{x = 1, y = 2}` desambiguado como record (T25 — estrutural; T32
    /// constrói).
    RecordLit {
        type_name: String,
        fields: Vec<(String, TypedExp)>,
    },
    /// `{["a"] = 1}` desambiguado como map (T25 — estrutural; T33 constrói).
    MapLit(Vec<(TypedExp, TypedExp)>),
}

/// Operador binário já resolvido (T13). Enum, não `String`, para o `match`
/// do codegen ser exaustivo; a conversão a partir da grafia do fonte
/// acontece uma única vez, em `check_binop`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    And,
    Or,
}

impl BinOp {
    /// Grafia do operador no fonte Titan — as mesmas strings que o parser
    /// coloca em `ExpBinop.op`. `None` para operadores fora do subconjunto
    /// (bitwise, `//`), que viram erro claro no chamador.
    fn from_source(op: &str) -> Option<BinOp> {
        Some(match op {
            "+" => BinOp::Add,
            "-" => BinOp::Sub,
            "*" => BinOp::Mul,
            "/" => BinOp::Div,
            "%" => BinOp::Mod,
            "^" => BinOp::Pow,
            "==" => BinOp::Eq,
            "~=" => BinOp::Ne,
            "<" => BinOp::Lt,
            ">" => BinOp::Gt,
            "<=" => BinOp::Le,
            ">=" => BinOp::Ge,
            "and" => BinOp::And,
            "or" => BinOp::Or,
            _ => return None,
        })
    }
}

/// Uma ocorrência de nome resolvida com sucesso pela passada 2 — índice
/// colateral para hover e go-to-definition (PRD.md, T49). Não influencia a
/// checagem de tipos: só registra, no momento em que já é conhecido, o que
/// a symtab (uma pilha de `HashMap` que some ao fechar o bloco) descartaria.
///
/// `use_loc`/`name` dão o range clicável no LSP (`use_loc` até
/// `use_loc + name.chars().count()`); `def_loc` é para onde
/// go-to-definition salta; `type_name` é o texto pronto para hover, já
/// formatado por [`type_name`] — a mesma função que o checker usa nas
/// mensagens de erro.
#[derive(Debug, Clone, PartialEq)]
pub struct SymbolUse {
    pub use_loc: Loc,
    pub def_loc: Loc,
    pub name: String,
    pub type_name: String,
}

/// Saída completa de [`check`]: a AST tipada (o que `codegen` consome) mais
/// o índice de usos que o LSP consome (T49). Separado de `TypedProgram` para
/// não obrigar `codegen`, que não precisa do índice, a lidar com ele.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckedProgram {
    pub program: TypedProgram,
    pub uses: Vec<SymbolUse>,
}

/// Operador unário já resolvido (T13; `Len` acrescentado estruturalmente na
/// T25 — `#v`/`#s`, sem produtor ainda: `check_unop` só mapeia `-`/`not` do
/// parser).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
    Len,
}

// ---- Checker -------------------------------------------------------------

struct Checker {
    st: SymTab,
    errors: Vec<CheckError>,
    /// Próximo id de declaração `local` — os ids são globais e únicos, então
    /// não precisam de reset entre funções.
    next_decl_id: DeclId,
    /// Ids das declarações que receberam alguma atribuição (mesmo espírito
    /// do `var._decl._assigned = true` do original, `checker.lua:404`).
    assigned: HashSet<DeclId>,
    /// Tabela de tipos nomeados (T25 — estrutural; T26 é quem passa a
    /// popular isto em `collect_signature`, que hoje rejeita `record` com
    /// erro claro antes de chegar aqui). Não há tabela de tipos nomeados
    /// hoje — só primitivas e compostos estruturais são resolvidos em
    /// `resolve_type`.
    records: HashMap<String, Type>,
    /// Módulos importados (`import data`, T38), no molde de `records`:
    /// nome Titan → entrada da tabela de capabilities (`capabilities.rs`,
    /// T37), consultada por `resolve_type` (`data.DataFrame`) e por
    /// `check_call`/`check_var` (T39/T40) para membros do módulo.
    modules: HashMap<String, &'static crate::capabilities::Capability>,
    /// Índice colateral de usos resolvidos, para hover e go-to-definition
    /// (T49) — ver [`SymbolUse`].
    uses: Vec<SymbolUse>,
    /// Local de declaração do *nome* de cada record (a chave do `record ...
    /// end`), separado de `records` porque `Type::Record` não carrega `Loc`.
    record_def_locs: HashMap<String, Loc>,
    /// Local de declaração de cada campo de record — `(nome do record, nome
    /// do campo) -> Loc`, pela mesma razão de `record_def_locs`.
    field_def_locs: HashMap<(String, String), Loc>,
}

/// Loc sentinela para símbolos sem declaração em arquivo Titan (builtins da
/// stdlib, módulos `import`ados): não há para onde saltar no fonte do
/// usuário, então go-to-definition sobre eles é ignorado pelo LSP em vez de
/// apontar para um local sem sentido.
const NO_DEF_LOC: Loc = Loc { line: 0, col: 0 };

impl Checker {
    fn new() -> Self {
        let mut st = SymTab::new();
        // Funções da stdlib vêm do runtime, registradas no escopo global —
        // não são palavras-chave (PRD.md, T5; tabela unificada na T25).
        for b in crate::builtins::BUILTINS {
            st.add_symbol(
                b.titan_name,
                Type::Function {
                    params: b.params.to_vec(),
                    rettypes: vec![b.rettype.clone()],
                },
                SymbolKind::Global,
                NO_DEF_LOC,
            );
        }
        Checker {
            st,
            errors: Vec::new(),
            next_decl_id: 0,
            assigned: HashSet::new(),
            records: HashMap::new(),
            modules: HashMap::new(),
            uses: Vec::new(),
            record_def_locs: HashMap::new(),
            field_def_locs: HashMap::new(),
        }
    }

    /// Registra um uso resolvido (T49) — chamado do único lugar onde um nome
    /// vira `TypedExpKind::Var`/`Callee`/`Field`, sempre com a `Loc` de
    /// declaração já em mãos.
    fn record_use(&mut self, use_loc: Loc, def_loc: Loc, name: &str, ty: &Type) {
        if def_loc == NO_DEF_LOC {
            return;
        }
        self.uses.push(SymbolUse {
            use_loc,
            def_loc,
            name: name.to_string(),
            type_name: type_name(ty),
        });
    }

    fn error(&mut self, loc: Loc, message: impl Into<String>) {
        self.errors.push(CheckError {
            message: message.into(),
            loc,
        });
    }

    // ---- Passada 1: assinaturas top-level ------------------------------

    /// Nomes que colidiriam com tipos do prelúdio do Rust se virassem o nome
    /// de uma `struct` gerada — lista fechada dada pelo PRD.md (T29).
    const RESERVED_RUST_NAMES: [&'static str; 5] = ["String", "Vec", "Option", "Box", "Result"];

    /// Registra o nome e os campos **brutos** de todos os records do
    /// programa (sem resolver tipos ainda) — passo 1 da checagem de records
    /// (T29). Rejeita nome duplicado, nome reservado do Rust e campo sem
    /// tipo/duplicado. Devolve `false` se algum desses erros ocorreu (o
    /// chamador então pula a detecção de ciclo e a resolução de tipos, que
    /// pressupõem uma lista limpa).
    fn collect_record_names(&mut self, program: &Program) -> HashMap<String, Vec<ast::Decl>> {
        let mut raw: HashMap<String, Vec<ast::Decl>> = HashMap::new();
        for node in program {
            let TopLevel::TopLevelRecord { loc, name, fields } = node else {
                continue;
            };
            if raw.contains_key(name) {
                self.error(*loc, format!("'{name}' já foi declarado antes."));
                continue;
            }
            if Self::RESERVED_RUST_NAMES.contains(&name.as_str()) {
                self.error(
                    *loc,
                    format!(
                        "'{name}' é um nome reservado do Rust; escolha outro nome de record."
                    ),
                );
                continue;
            }
            let mut seen = HashSet::new();
            let mut ok = true;
            for field in fields {
                if !seen.insert(field.name.clone()) {
                    self.error(
                        field.loc,
                        format!("campo '{}' duplicado no record '{name}'.", field.name),
                    );
                    ok = false;
                }
                if field.r#type.is_none() {
                    self.error(
                        field.loc,
                        format!("campo '{}' precisa de um tipo explícito.", field.name),
                    );
                    ok = false;
                }
            }
            if ok {
                // Go-to-definition (T49): local do nome do record e de cada
                // campo, perdido depois que `Type::Record` guarda só nomes.
                self.record_def_locs.insert(name.clone(), *loc);
                for field in fields {
                    self.field_def_locs
                        .insert((name.clone(), field.name.clone()), field.loc);
                }
                raw.insert(name.clone(), fields.clone());
            }
        }
        raw
    }

    /// Detecta recursão direta ou indireta no grafo de dependência de
    /// records (um campo `TypeName` de um record para outro é uma aresta) —
    /// um record recursivo seria infinitamente grande em Rust sem `Box`
    /// (PRD.md, T29). DFS com três cores; devolve o nome do primeiro record
    /// já registrado envolvido em um ciclo, se houver.
    fn find_recursive_record(raw: &HashMap<String, Vec<ast::Decl>>) -> Option<String> {
        #[derive(Clone, Copy, PartialEq)]
        enum Color {
            White,
            Gray,
            Black,
        }
        fn dependencies(fields: &[ast::Decl]) -> Vec<&str> {
            fields
                .iter()
                .filter_map(|f| match &f.r#type {
                    Some(ast::Type::TypeName { name, .. }) => Some(name.as_str()),
                    _ => None,
                })
                .collect()
        }
        fn visit(
            name: &str,
            raw: &HashMap<String, Vec<ast::Decl>>,
            colors: &mut HashMap<String, Color>,
        ) -> bool {
            match colors.get(name).copied().unwrap_or(Color::White) {
                Color::Black => return false,
                Color::Gray => return true,
                Color::White => {}
            }
            colors.insert(name.to_string(), Color::Gray);
            if let Some(fields) = raw.get(name) {
                for dep in dependencies(fields) {
                    if raw.contains_key(dep) && visit(dep, raw, colors) {
                        return true;
                    }
                }
            }
            colors.insert(name.to_string(), Color::Black);
            false
        }

        let mut colors = HashMap::new();
        for name in raw.keys() {
            if visit(name, raw, &mut colors) {
                return Some(name.clone());
            }
        }
        None
    }

    /// Records primeiro, funções depois (T29): uma função pode receber um
    /// record declarado mais adiante no arquivo. `self.records` precisa
    /// estar completo antes de `resolve_param_types`/`resolve_types`
    /// resolverem qualquer `TypeName`.
    fn collect_records(&mut self, program: &Program) {
        let raw = self.collect_record_names(program);

        if let Some(cycle_name) = Self::find_recursive_record(&raw) {
            let loc = program
                .iter()
                .find_map(|node| match node {
                    TopLevel::TopLevelRecord { loc, name, .. } if name == &cycle_name => {
                        Some(*loc)
                    }
                    _ => None,
                })
                .unwrap_or(Loc { line: 1, col: 1 });
            self.error(
                loc,
                format!(
                    "o record '{cycle_name}' é recursivo (direta ou indiretamente); \
                     esta fase não suporta indireção para quebrar o ciclo."
                ),
            );
            return;
        }

        // Placeholders (campos ainda vazios) para todo record, **antes** de
        // resolver qualquer campo: um record que se referencia só através de
        // um composto (`filhos: {No}`) não é recursão real (`Vec<No>` tem
        // tamanho finito, diferente de um campo `No` direto, já rejeitado
        // acima) — mas sem o nome já presente em `self.records`,
        // `resolve_type` não teria como resolver o `TypeName("No")` dentro
        // do `{No}` enquanto o próprio `No` ainda está sendo processado.
        for name in raw.keys() {
            self.records.insert(
                name.clone(),
                Type::Record {
                    name: name.clone(),
                    fields: Vec::new(),
                },
            );
        }

        // Ordem topológica (dependências diretas antes de quem as usa) só
        // para achar uma ordem de resolução estável; não é mais estritamente
        // necessária para correção (os placeholders acima já cobrem
        // qualquer ordem), mas mantém a mensagem de erro determinística.
        for name in Self::topological_order(&raw) {
            let fields = &raw[&name];
            let mut typed_fields = Vec::with_capacity(fields.len());
            let mut ok = true;
            for field in fields {
                // `unwrap`: `collect_record_names` já garantiu que todo
                // campo aqui tem `r#type: Some(..)`.
                let annotated = field.r#type.as_ref().unwrap();
                match self.resolve_type(annotated) {
                    Some(ty) => typed_fields.push((field.name.clone(), ty)),
                    None => ok = false,
                }
            }
            if ok {
                let record_ty = Type::Record {
                    name: name.clone(),
                    fields: typed_fields,
                };
                // Hover sobre o próprio nome do record e sobre cada campo na
                // declaração `record ... end` (T49) — só dá para registrar
                // aqui, depois que os campos têm `Type` resolvido.
                if let Some(&def_loc) = self.record_def_locs.get(&name) {
                    self.record_use(def_loc, def_loc, &name, &record_ty);
                }
                if let Type::Record { fields, .. } = &record_ty {
                    for (fname, fty) in fields {
                        if let Some(&floc) =
                            self.field_def_locs.get(&(name.clone(), fname.clone()))
                        {
                            self.record_use(floc, floc, fname, fty);
                        }
                    }
                }
                self.records.insert(name.clone(), record_ty);
            } else {
                // Resolução falhou (erro já reportado por `resolve_type`) —
                // remove o placeholder para não deixar um record "fantasma"
                // com campos vazios disponível ao resto do checker.
                self.records.remove(&name);
            }
        }
    }

    /// Ordem pós-ordem de uma DFS sobre o grafo de dependência de records —
    /// garante que um record só é resolvido depois de todo record que ele
    /// referencia por `TypeName` (só é chamada quando já não há ciclo).
    fn topological_order(raw: &HashMap<String, Vec<ast::Decl>>) -> Vec<String> {
        fn visit(name: &str, raw: &HashMap<String, Vec<ast::Decl>>, visited: &mut HashSet<String>, order: &mut Vec<String>) {
            if !visited.insert(name.to_string()) {
                return;
            }
            if let Some(fields) = raw.get(name) {
                for field in fields {
                    if let Some(ast::Type::TypeName { name: dep, .. }) = &field.r#type
                        && raw.contains_key(dep)
                    {
                        visit(dep, raw, visited, order);
                    }
                }
            }
            order.push(name.to_string());
        }

        let mut visited = HashSet::new();
        let mut order = Vec::with_capacity(raw.len());
        for name in raw.keys() {
            visit(name, raw, &mut visited, &mut order);
        }
        order
    }

    fn collect_signature(&mut self, node: &TopLevel) {
        match node {
            TopLevel::TopLevelFunc {
                loc,
                name,
                params,
                rettypes,
                ..
            } => {
                if self.st.find_symbol(name).is_some() {
                    self.error(*loc, format!("'{name}' já foi declarado antes."));
                    return;
                }
                let param_types = match self.resolve_param_types(params) {
                    Some(types) => types,
                    None => return,
                };
                let ret_types = match self.resolve_types(rettypes) {
                    Some(types) => types,
                    None => return,
                };
                let fn_ty = Type::Function {
                    params: param_types,
                    rettypes: ret_types,
                };
                self.st
                    .add_symbol(name, fn_ty.clone(), SymbolKind::Global, *loc);
                // A própria declaração conta como "uso" de si mesma (T49):
                // hover sobre o nome no `function nome(...)` também funciona,
                // não só sobre as chamadas.
                self.record_use(*loc, *loc, name, &fn_ty);
            }
            TopLevel::TopLevelVar { loc, .. } => {
                self.error(
                    *loc,
                    "declaração de variável no nível de topo não é suportada nesta fase.",
                );
            }
            // Já processado por `collect_records`, que roda antes (T29 —
            // duas sub-passadas: records primeiro, funções depois).
            TopLevel::TopLevelRecord { .. } => {}
            TopLevel::TopLevelImport {
                loc, modname, ..
            } => {
                if self.st.find_symbol(modname).is_some() || self.modules.contains_key(modname) {
                    self.error(*loc, format!("'{modname}' já foi declarado antes."));
                    return;
                }
                match crate::capabilities::lookup_module(modname) {
                    Some(capability) => {
                        self.modules.insert(modname.clone(), capability);
                        self.st.add_symbol(
                            modname,
                            Type::Invalid,
                            SymbolKind::Module {
                                name: modname.clone(),
                            },
                            NO_DEF_LOC,
                        );
                    }
                    None => {
                        let available = crate::capabilities::available_module_names().join(", ");
                        self.error(
                            *loc,
                            format!(
                                "capability '{modname}' não existe; disponíveis: {available}."
                            ),
                        );
                    }
                }
            }
            TopLevel::TopLevelForeignImport { loc, .. } => {
                self.error(*loc, "`foreign import` não é suportado nesta fase.");
            }
            TopLevel::TopLevelMethod { loc, .. } | TopLevel::TopLevelStatic { loc, .. } => {
                self.error(*loc, "métodos não são suportados nesta fase.");
            }
        }
    }

    fn resolve_param_types(&mut self, params: &[ast::Decl]) -> Option<Vec<Type>> {
        let mut result = Vec::with_capacity(params.len());
        for param in params {
            let Some(annotated) = &param.r#type else {
                self.error(
                    param.loc,
                    format!("parâmetro '{}' precisa de um tipo explícito.", param.name),
                );
                return None;
            };
            result.push(self.resolve_type(annotated)?);
        }
        Some(result)
    }

    fn resolve_types(&mut self, types: &[ast::Type]) -> Option<Vec<Type>> {
        let mut result = Vec::with_capacity(types.len());
        for t in types {
            result.push(self.resolve_type(t)?);
        }
        Some(result)
    }

    /// Converte uma anotação de tipo escrita no código-fonte (`ast::Type`) no
    /// tipo semântico correspondente (`types::Type`).
    fn resolve_type(&mut self, t: &ast::Type) -> Option<Type> {
        match t {
            ast::Type::TypeNil { .. } => Some(Type::Nil),
            ast::Type::TypeBoolean { .. } => Some(Type::Boolean),
            ast::Type::TypeInteger { .. } => Some(Type::Integer),
            ast::Type::TypeFloat { .. } => Some(Type::Float),
            ast::Type::TypeString { .. } => Some(Type::String),
            // T22 fez `value` chegar ao parser/checker, mas o codegen não
            // sabe emiti-lo (cairia no `unreachable!` de
            // `rust_type_name:625`, que é panic e violaria a convenção).
            // Rejeitado explicitamente até uma fase futura dar suporte.
            ast::Type::TypeValue { loc } => {
                self.error(*loc, "tipo `value` não é suportado nesta fase.");
                None
            }
            ast::Type::TypeArray { subtype, .. } => {
                let elem = self.resolve_type(subtype)?;
                Some(Type::Array {
                    elem: Box::new(elem),
                })
            }
            ast::Type::TypeMap {
                loc,
                keystype,
                valuestype,
            } => {
                let keys = self.resolve_type(keystype)?;
                if !matches!(keys, Type::Integer | Type::String | Type::Boolean) {
                    // O `HashMap` do Rust exige `Eq + Hash`, que `f64` não
                    // tem e que `Vec`/struct não derivam nesta fase (T29).
                    self.error(
                        *loc,
                        format!(
                            "chave de `map` precisa ser integer, string ou boolean, encontrado {}.",
                            type_name(&keys)
                        ),
                    );
                    return None;
                }
                let values = self.resolve_type(valuestype)?;
                Some(Type::Map {
                    keys: Box::new(keys),
                    values: Box::new(values),
                })
            }
            ast::Type::TypeFunction { loc, .. } => {
                self.error(
                    *loc,
                    "tipo de função como anotação não é suportado nesta fase.",
                );
                None
            }
            ast::Type::TypeOption { loc, .. } => {
                self.error(*loc, "tipo opcional (`?`) não é suportado nesta fase.");
                None
            }
            ast::Type::TypeName { loc, name } => match self.records.get(name).cloned() {
                Some(ty) => {
                    // Go-to-definition (T49): anotação `x: Nome` salta para
                    // o `record Nome ... end`.
                    if let Some(&def_loc) = self.record_def_locs.get(name) {
                        self.record_use(*loc, def_loc, name, &ty);
                    }
                    Some(ty)
                }
                None => {
                    self.error(*loc, format!("tipo '{name}' desconhecido."));
                    None
                }
            },
            ast::Type::TypeQualName { loc, module, name } => {
                let Some(capability) = self.modules.get(module) else {
                    self.error(*loc, format!("módulo '{module}' não foi importado."));
                    return None;
                };
                match capability.find_opaque(name) {
                    Some(opaque) => Some(Type::Opaque {
                        module: module.clone(),
                        name: name.clone(),
                        rust_path: opaque.rust_path.to_string(),
                    }),
                    None => {
                        self.error(
                            *loc,
                            format!("o módulo '{module}' não tem o tipo '{name}'."),
                        );
                        None
                    }
                }
            }
        }
    }

    // ---- Validação de `main` -------------------------------------------

    fn check_has_main(&mut self, program: &Program) {
        let has_valid_main = program.iter().any(|node| match node {
            TopLevel::TopLevelFunc {
                name,
                params,
                rettypes,
                ..
            } if name == "main" => {
                params.len() == 1
                    && matches!(&params[0].r#type, Some(ast::Type::TypeArray { subtype, .. })
                        if matches!(**subtype, ast::Type::TypeString { .. }))
                    && rettypes.len() == 1
                    && matches!(rettypes[0], ast::Type::TypeInteger { .. })
            }
            _ => false,
        });

        if !has_valid_main {
            let loc = program
                .iter()
                .find_map(|node| match node {
                    TopLevel::TopLevelFunc { name, loc, .. } if name == "main" => Some(*loc),
                    _ => None,
                })
                .unwrap_or(Loc { line: 1, col: 1 });
            self.error(
                loc,
                "função 'main' precisa ter a assinatura main(args: {string}): integer.",
            );
        }
    }

    // ---- Passada 2: corpos ----------------------------------------------

    fn check_toplevel(&mut self, node: &TopLevel) -> Option<TypedTopLevel> {
        match node {
            TopLevel::TopLevelFunc {
                loc,
                islocal,
                name,
                params,
                block,
                ..
            } => {
                let Some(Symbol {
                    ty:
                        Type::Function {
                            params: param_types,
                            rettypes: ret_types,
                        },
                    ..
                }) = self.st.find_symbol(name).cloned()
                else {
                    // Assinatura já rejeitada na passada 1.
                    return None;
                };

                self.st.open_block();
                for (param, ty) in params.iter().zip(param_types.iter()) {
                    self.st.add_symbol(
                        &param.name,
                        ty.clone(),
                        SymbolKind::Param,
                        param.loc,
                    );
                    // Hover sobre o próprio parâmetro na assinatura (T49).
                    self.record_use(param.loc, param.loc, &param.name, ty);
                }

                let body = self.check_stat(block, &ret_types);

                self.st.close_block();

                let mut body = body?;
                // Fix-up de mutabilidade (decisão 6): agora que todas as
                // atribuições do corpo foram vistas, marca as declarações
                // reatribuídas.
                fixup_mutability(&mut body, &self.assigned);
                let named_params = params
                    .iter()
                    .zip(param_types)
                    .map(|(p, t)| (p.name.clone(), t))
                    .collect();

                Some(TypedTopLevel::Func {
                    loc: *loc,
                    islocal: *islocal,
                    name: name.clone(),
                    params: named_params,
                    rettypes: ret_types,
                    body: Box::new(body),
                })
            }
            TopLevel::TopLevelRecord { loc, name, .. } => {
                // A resolução de verdade já aconteceu em `collect_records`
                // (passada 1); aqui só reaproveitamos o resultado — se o
                // record não está em `self.records`, ele já foi rejeitado
                // com erro claro lá (nome duplicado, campo inválido, ciclo).
                let Some(Type::Record { fields, .. }) = self.records.get(name).cloned() else {
                    return None;
                };
                Some(TypedTopLevel::Record {
                    loc: *loc,
                    name: name.clone(),
                    fields,
                })
            }
            // Já reportado como erro na passada 1.
            _ => None,
        }
    }

    fn check_stat(&mut self, stat: &Stat, rettypes: &[Type]) -> Option<TypedStat> {
        match stat {
            Stat::StatBlock { loc, stats } => {
                self.st.open_block();
                let mut typed_stats = Vec::with_capacity(stats.len());
                let mut ok = true;
                for s in stats {
                    match self.check_stat(s, rettypes) {
                        Some(typed) => typed_stats.push(typed),
                        None => ok = false,
                    }
                }
                self.st.close_block();
                if ok {
                    Some(TypedStat::Block {
                        loc: *loc,
                        stats: typed_stats,
                    })
                } else {
                    None
                }
            }
            Stat::StatDecl { loc, decls, exps } => {
                if decls.len() != 1 || exps.len() != 1 {
                    self.error(
                        *loc,
                        "declaração múltipla (`local a, b = ...`) não é suportada nesta fase.",
                    );
                    return None;
                }
                let decl = &decls[0];
                // Resolvido antes de tipar o valor (T29): `{...}` precisa do
                // tipo anotado como contexto para se desambiguar.
                let declared = match &decl.r#type {
                    Some(annotated) => Some(self.resolve_type(annotated)?),
                    None => None,
                };
                let value = self.check_exp(&exps[0], declared.as_ref())?;

                let ty = match declared {
                    Some(declared) => {
                        if !declared.compatible(&value.ty) {
                            self.error(
                                decl.loc,
                                format!(
                                    "tipos incompatíveis na declaração de '{}': esperado {}, encontrado {}.",
                                    decl.name,
                                    type_name(&declared),
                                    type_name(&value.ty)
                                ),
                            );
                            return None;
                        }
                        declared
                    }
                    None => value.ty.clone(),
                };

                let decl_id = self.next_decl_id;
                self.next_decl_id += 1;
                self.st.add_symbol(
                    &decl.name,
                    ty.clone(),
                    SymbolKind::Local { decl_id },
                    decl.loc,
                );
                // Hover sobre o próprio `local nome: tipo = ...` (T49).
                self.record_use(decl.loc, decl.loc, &decl.name, &ty);

                Some(TypedStat::Decl {
                    loc: *loc,
                    name: decl.name.clone(),
                    ty,
                    value,
                    decl_id,
                    mutable: false,
                })
            }
            Stat::StatCall { loc, callexp } => {
                let call = self.check_exp(callexp, None)?;
                Some(TypedStat::Call { loc: *loc, call })
            }
            Stat::StatReturn { loc, exps } => {
                let mut typed_exps = Vec::with_capacity(exps.len());
                let mut ok = true;
                for (i, e) in exps.iter().enumerate() {
                    match self.check_exp(e, rettypes.get(i)) {
                        Some(typed) => typed_exps.push(typed),
                        None => ok = false,
                    }
                }
                if !ok {
                    return None;
                }

                if typed_exps.len() != rettypes.len() {
                    self.error(
                        *loc,
                        format!(
                            "retornou {} valor(es), mas a função espera {}.",
                            typed_exps.len(),
                            rettypes.len()
                        ),
                    );
                    return None;
                }

                for (found, expected) in typed_exps.iter().zip(rettypes) {
                    if !expected.compatible(&found.ty) {
                        self.error(
                            found.loc,
                            format!(
                                "retorno incompatível: esperado {}, encontrado {}.",
                                type_name(expected),
                                type_name(&found.ty)
                            ),
                        );
                        return None;
                    }
                }

                Some(TypedStat::Return {
                    loc: *loc,
                    exps: typed_exps,
                })
            }
            Stat::StatIf {
                loc,
                thens,
                elsestat,
            } => {
                // Defensivo: o parser (T11) sempre produz ao menos um ramo.
                if thens.is_empty() {
                    self.error(*loc, "um `if` precisa de ao menos uma condição.");
                    return None;
                }
                let mut typed_thens = Vec::with_capacity(thens.len());
                let mut ok = true;
                for then in thens {
                    let condition = self.check_condition(&then.condition, "if");
                    let block = self.check_stat(&then.block, rettypes);
                    match (condition, block) {
                        (Some(condition), Some(block)) => typed_thens.push(TypedThen {
                            loc: then.loc,
                            condition,
                            block,
                        }),
                        _ => ok = false,
                    }
                }
                let typed_else = match elsestat {
                    Some(stat) => match self.check_stat(stat, rettypes) {
                        Some(typed) => Some(Box::new(typed)),
                        None => {
                            ok = false;
                            None
                        }
                    },
                    None => None,
                };
                if !ok {
                    return None;
                }
                Some(TypedStat::If {
                    loc: *loc,
                    thens: typed_thens,
                    elsestat: typed_else,
                })
            }
            Stat::StatWhile {
                loc,
                condition,
                block,
            } => {
                let condition = self.check_condition(condition, "while");
                let block = self.check_stat(block, rettypes);
                Some(TypedStat::While {
                    loc: *loc,
                    condition: condition?,
                    block: Box::new(block?),
                })
            }
            Stat::StatRepeat { loc, .. } => {
                self.error(*loc, "`repeat` não é suportado nesta fase.");
                None
            }
            Stat::StatFor {
                loc,
                decl,
                start,
                finish,
                inc,
                block,
            } => self.check_for(*loc, decl, start, finish, inc.as_deref(), block, rettypes),
            Stat::StatAssign { loc, vars, exps } => {
                if vars.len() != 1 || exps.len() != 1 {
                    // Defensivo: o parser (T11) só produz single-target.
                    self.error(
                        *loc,
                        "atribuição múltipla (`a, b = ...`) não é suportada nesta fase.",
                    );
                    return None;
                }
                self.check_assign(*loc, &vars[0], &exps[0])
            }
        }
    }

    /// Condição de `if`/`elseif`/`while`: precisa ser `Boolean` (ou `Value`,
    /// via `compatible` — gradual typing), como o `checkexp(cond, ...,
    /// types.Boolean())` do original.
    fn check_condition(&mut self, exp: &Exp, contexto: &str) -> Option<TypedExp> {
        let typed = self.check_exp(exp, Some(&Type::Boolean))?;
        if !Type::Boolean.compatible(&typed.ty) {
            self.error(
                typed.loc,
                format!(
                    "a condição do `{contexto}` precisa ser boolean, encontrado {}.",
                    type_name(&typed.ty)
                ),
            );
            return None;
        }
        Some(typed)
    }

    /// `for` numérico, espelhando `checkfor` (`checker.lua:239-288`):
    /// expressões verificadas **antes** de declarar a variável (elas não
    /// podem referenciá-la), tipo da variável vindo da anotação ou inferido
    /// de `start`, e — decisão 5 da Fase 1 — `start`/`finish`/`inc` com tipo
    /// **idêntico** ao da variável (sem coerção int→float).
    #[allow(clippy::too_many_arguments)]
    fn check_for(
        &mut self,
        loc: Loc,
        decl: &ast::Decl,
        start: &Exp,
        finish: &Exp,
        inc: Option<&Exp>,
        block: &Stat,
        rettypes: &[Type],
    ) -> Option<TypedStat> {
        let typed_start = self.check_exp(start, None)?;
        let typed_finish = self.check_exp(finish, None)?;
        let typed_inc = match inc {
            Some(exp) => Some(self.check_exp(exp, None)?),
            None => None,
        };

        let var_ty = match &decl.r#type {
            Some(annotated) => self.resolve_type(annotated)?,
            None => typed_start.ty.clone(),
        };

        if !matches!(var_ty, Type::Integer | Type::Float) {
            self.error(
                decl.loc,
                format!(
                    "a variável de controle do `for` precisa ser integer ou float, encontrado {}.",
                    type_name(&var_ty)
                ),
            );
            return None;
        }

        let mut ok = true;
        for (typed, papel) in [(&typed_start, "valor inicial"), (&typed_finish, "limite")]
            .into_iter()
            .chain(typed_inc.iter().map(|t| (t, "passo")))
        {
            if !typed.ty.equals(&var_ty) {
                self.error(
                    typed.loc,
                    format!(
                        "o {papel} do `for` precisa ter o mesmo tipo da variável de controle ({}), encontrado {}.",
                        type_name(&var_ty),
                        type_name(&typed.ty)
                    ),
                );
                ok = false;
            }
        }
        if !ok {
            return None;
        }

        // `inc` omitido vira `1`/`1.0` conforme o tipo, com o `loc` do
        // limite (como `ast.ExpInteger(node.finish.loc, 1)` no original).
        let typed_inc = typed_inc.unwrap_or_else(|| TypedExp {
            loc: typed_finish.loc,
            ty: var_ty.clone(),
            kind: match var_ty {
                Type::Integer => TypedExpKind::Integer(1),
                _ => TypedExpKind::Float(1.0),
            },
        });

        // A variável de controle vive num bloco próprio que não vaza para
        // fora do laço (o corpo `StatBlock` abre o seu por cima).
        self.st.open_block();
        self.st.add_symbol(
            &decl.name,
            var_ty.clone(),
            SymbolKind::ForVar,
            decl.loc,
        );
        // Hover sobre a própria variável de controle no `for nome: tipo =
        // ...` (T49).
        self.record_use(decl.loc, decl.loc, &decl.name, &var_ty);
        let typed_block = self.check_stat(block, rettypes);
        self.st.close_block();

        Some(TypedStat::For {
            loc,
            name: decl.name.clone(),
            ty: var_ty,
            start: typed_start,
            finish: typed_finish,
            inc: Box::new(typed_inc),
            block: Box::new(typed_block?),
        })
    }

    /// Atribuição single-target `nome = exp` | `v[i] = exp` | `p.campo = exp`
    /// (`checker.lua:378-410`, estendido na T29 para `Index`/`Field`).
    fn check_assign(&mut self, loc: Loc, var: &Var, exp: &Exp) -> Option<TypedStat> {
        match var {
            Var::VarName {
                loc: var_loc, name, ..
            } => {
                let Some(symbol) = self.st.find_symbol(name).cloned() else {
                    self.error(*var_loc, format!("'{name}' não foi declarado."));
                    return None;
                };

                match symbol.kind {
                    // Globais nesta fase são sempre funções (`print` e as
                    // top-level) — "trying to assign to a function"
                    // (`checker.lua:401`).
                    SymbolKind::Global => {
                        self.error(*var_loc, "não é possível atribuir a uma função.");
                        return None;
                    }
                    // Módulo (T38): `data = 1` não faz sentido — o nome
                    // designa o módulo importado, não um valor.
                    SymbolKind::Module { .. } => {
                        self.error(
                            *var_loc,
                            format!("não é possível atribuir ao módulo '{name}'."),
                        );
                        return None;
                    }
                    SymbolKind::Param if is_composite(&symbol.ty) => {
                        // T29: parâmetro composto aceita `xs[i] = v` (via o
                        // braço `VarBracket`/`VarDot` abaixo — `check_assign`
                        // só é chamado com o `Var` inteiro, então esta rota
                        // (`VarName`) é sempre a atribuição ao parâmetro
                        // **inteiro**, que segue proibida mesmo composto.
                        self.error(
                            *var_loc,
                            format!(
                                "não é possível atribuir ao parâmetro composto '{name}' inteiro; modifique seus elementos/campos."
                            ),
                        );
                        return None;
                    }
                    SymbolKind::Param => {
                        self.error(
                            *var_loc,
                            format!("não é possível atribuir ao parâmetro '{name}' nesta fase."),
                        );
                        return None;
                    }
                    // `ForVar` é sempre `mut` no template do T15 (nada a
                    // rastrear); `Local` é registrada mais abaixo, após a
                    // atribuição validar.
                    SymbolKind::ForVar | SymbolKind::Local { .. } => {}
                }

                let value = self.check_exp(exp, Some(&symbol.ty))?;
                if !symbol.ty.compatible(&value.ty) {
                    self.error(
                        value.loc,
                        format!(
                            "atribuição incompatível para '{name}': esperado {}, encontrado {}.",
                            type_name(&symbol.ty),
                            type_name(&value.ty)
                        ),
                    );
                    return None;
                }

                if let SymbolKind::Local { decl_id } = symbol.kind {
                    self.assigned.insert(decl_id);
                }

                Some(TypedStat::Assign {
                    loc,
                    target: TypedLValue::Name(name.clone()),
                    value,
                })
            }
            Var::VarBracket { .. } | Var::VarDot { .. } => {
                // A variável-raiz da cadeia de índices/campos precisa ser um
                // parâmetro composto ou uma local — nunca uma função nem um
                // parâmetro escalar.
                let Some(root_name) = root_var_name(var) else {
                    // Defensivo: `VarBracket`/`VarDot` sempre têm uma
                    // `VarName` na raiz da cadeia (o parser só produz `[`/`.`
                    // como sufixo de uma expressão primária).
                    self.error(loc, "alvo de atribuição inválido.");
                    return None;
                };
                let Some(root_symbol) = self.st.find_symbol(&root_name).cloned() else {
                    self.error(loc, format!("'{root_name}' não foi declarado."));
                    return None;
                };
                match root_symbol.kind {
                    SymbolKind::Global => {
                        self.error(loc, "não é possível atribuir a uma função.");
                        return None;
                    }
                    SymbolKind::Module { .. } => {
                        self.error(
                            loc,
                            format!("não é possível atribuir ao módulo '{root_name}'."),
                        );
                        return None;
                    }
                    SymbolKind::Param if !is_composite(&root_symbol.ty) => {
                        self.error(
                            loc,
                            format!(
                                "não é possível atribuir através do parâmetro escalar '{root_name}' nesta fase."
                            ),
                        );
                        return None;
                    }
                    SymbolKind::Param | SymbolKind::ForVar | SymbolKind::Local { .. } => {}
                }

                let target = self.check_var(&loc, var)?;
                let target_ty = target.ty.clone();
                let value = self.check_exp(exp, Some(&target_ty))?;
                if !target_ty.compatible(&value.ty) {
                    self.error(
                        value.loc,
                        format!(
                            "atribuição incompatível: esperado {}, encontrado {}.",
                            type_name(&target_ty),
                            type_name(&value.ty)
                        ),
                    );
                    return None;
                }

                if let SymbolKind::Local { decl_id } = root_symbol.kind {
                    self.assigned.insert(decl_id);
                }

                let target_lvalue = match target.kind {
                    TypedExpKind::Index { base, index } => TypedLValue::Index { base, index },
                    TypedExpKind::Field { base, name } => TypedLValue::Field { base, name },
                    // Inatingível: `check_var` só produz `Index`/`Field` para
                    // `VarBracket`/`VarDot`, os únicos braços deste `match`.
                    _ => unreachable!("check_var produziu um TypedExpKind inesperado"),
                };

                Some(TypedStat::Assign {
                    loc,
                    target: target_lvalue,
                    value,
                })
            }
        }
    }

    /// Tipa uma expressão. `context`, acrescentado na T29 (PRD.md), é o tipo
    /// esperado nesta posição quando conhecido de antemão (anotação de
    /// `local`, tipo de parâmetro/retorno, elemento de array/map, campo de
    /// record) — só `ExpInitList` o consome (a desambiguação de `{...}`
    /// depende dele), mas ele precisa atravessar todo `check_exp` para
    /// chegar até um `{...}` aninhado em qualquer posição.
    fn check_exp(&mut self, exp: &Exp, context: Option<&Type>) -> Option<TypedExp> {
        match exp {
            Exp::ExpNil { loc } => Some(TypedExp {
                loc: *loc,
                ty: Type::Nil,
                kind: TypedExpKind::Nil,
            }),
            Exp::ExpBool { loc, value } => Some(TypedExp {
                loc: *loc,
                ty: Type::Boolean,
                kind: TypedExpKind::Bool(*value),
            }),
            Exp::ExpInteger { loc, value } => Some(TypedExp {
                loc: *loc,
                ty: Type::Integer,
                kind: TypedExpKind::Integer(*value),
            }),
            Exp::ExpFloat { loc, value } => Some(TypedExp {
                loc: *loc,
                ty: Type::Float,
                kind: TypedExpKind::Float(*value),
            }),
            Exp::ExpString { loc, value } => Some(TypedExp {
                loc: *loc,
                ty: Type::String,
                kind: TypedExpKind::String(value.clone()),
            }),
            Exp::ExpVar { loc, var } => self.check_var(loc, var),
            Exp::ExpConcat { loc, exps } => {
                let mut typed_exps = Vec::with_capacity(exps.len());
                let mut ok = true;
                for e in exps {
                    match self.check_exp(e, None) {
                        Some(typed) => {
                            // Decisão 4 da Fase 1: `..` coage número→string
                            // (espírito do `trytostr` do original) — a
                            // conversão em si fica no codegen. `Boolean` e
                            // `Nil` seguem rejeitados.
                            if !matches!(
                                typed.ty,
                                Type::String | Type::Integer | Type::Float | Type::Value
                            ) {
                                self.error(
                                    typed.loc,
                                    format!(
                                        "operando de `..` precisa ser string, integer ou float, encontrado {}.",
                                        type_name(&typed.ty)
                                    ),
                                );
                                ok = false;
                            }
                            typed_exps.push(typed);
                        }
                        None => ok = false,
                    }
                }
                if !ok {
                    return None;
                }
                Some(TypedExp {
                    loc: *loc,
                    ty: Type::String,
                    kind: TypedExpKind::Concat(typed_exps),
                })
            }
            Exp::ExpCall { loc, exp, args } => self.check_call(loc, exp, args),
            Exp::ExpInitList { loc, fields } => self.check_init_list(*loc, fields, context),
            Exp::ExpUnop { loc, op, exp } => self.check_unop(*loc, op, exp),
            Exp::ExpBinop { loc, lhs, op, rhs } => self.check_binop(*loc, op, lhs, rhs),
            Exp::ExpCast { loc, .. } => {
                self.error(*loc, "cast de tipo (`as`) não é suportado nesta fase.");
                None
            }
            Exp::ExpAdjust { loc, .. } | Exp::ExpExtra { loc, .. } => {
                self.error(
                    *loc,
                    "múltiplos valores de retorno não são suportados nesta fase.",
                );
                None
            }
        }
    }

    /// Desambigua e tipa `{...}` (T29), espelhando `checker.lua:646-662`:
    /// contexto primeiro, senão a forma do primeiro campo decide.
    fn check_init_list(
        &mut self,
        loc: Loc,
        fields: &[ast::Field],
        context: Option<&Type>,
    ) -> Option<TypedExp> {
        // Contexto explícito manda, senão a forma do primeiro campo decide
        // (checker.lua:646-662). `{}` vazio sem contexto não tem como
        // decidir — erro claro.
        match context {
            Some(Type::Array { elem }) => self.check_array_lit(loc, fields, Some(elem.as_ref())),
            Some(Type::Map { keys, values }) => {
                self.check_map_lit(loc, fields, Some((keys.as_ref(), values.as_ref())))
            }
            Some(Type::Record {
                name: rname,
                fields: rfields,
            }) => self.check_record_lit(loc, fields, rname, rfields),
            Some(other) => {
                self.error(
                    loc,
                    format!(
                        "não é possível usar `{{...}}` onde se espera {}.",
                        type_name(other)
                    ),
                );
                None
            }
            None => match fields.first() {
                None => {
                    self.error(
                        loc,
                        "não é possível inferir o tipo de `{}` vazio; anote o tipo.",
                    );
                    None
                }
                Some(field) => match &field.name {
                    ast::FieldName::Key(_) => self.check_map_lit(loc, fields, None),
                    ast::FieldName::Name(_) => {
                        self.error(
                            loc,
                            "não é possível inferir o tipo do record; anote o tipo.",
                        );
                        None
                    }
                    ast::FieldName::None => self.check_array_lit(loc, fields, None),
                },
            },
        }
    }

    /// `{1, 2, 3}` como array (T29), espelhando `checker.lua:664-700`.
    fn check_array_lit(
        &mut self,
        loc: Loc,
        fields: &[ast::Field],
        econtext: Option<&Type>,
    ) -> Option<TypedExp> {
        let mut typed_elems = Vec::with_capacity(fields.len());
        let mut ok = true;
        for field in fields {
            if !matches!(field.name, ast::FieldName::None) {
                self.error(
                    field.loc,
                    "campo nomeado não é válido dentro de um literal de array.",
                );
                ok = false;
                continue;
            }
            match self.check_exp(&field.exp, econtext) {
                Some(typed) => typed_elems.push(typed),
                None => ok = false,
            }
        }
        if !ok {
            return None;
        }

        let elem_ty = match econtext {
            Some(ty) => ty.clone(),
            None => match typed_elems.first() {
                Some(first) => first.ty.clone(),
                // `fields` vazio só chega aqui vindo de `check_init_list`
                // com `econtext` `None`, que já rejeitou `{}` antes de
                // chamar este método — mantido por robustez.
                None => Type::Integer,
            },
        };

        for elem in &typed_elems {
            if !elem_ty.compatible(&elem.ty) {
                self.error(
                    elem.loc,
                    format!(
                        "elemento do array incompatível: esperado {}, encontrado {}.",
                        type_name(&elem_ty),
                        type_name(&elem.ty)
                    ),
                );
                return None;
            }
        }

        Some(TypedExp {
            loc,
            ty: Type::Array {
                elem: Box::new(elem_ty),
            },
            kind: TypedExpKind::ArrayLit(typed_elems),
        })
    }

    /// `{["a"] = 1}` como map (T29), espelhando `checker.lua:701-737`.
    fn check_map_lit(
        &mut self,
        loc: Loc,
        fields: &[ast::Field],
        context: Option<(&Type, &Type)>,
    ) -> Option<TypedExp> {
        let (kcontext, vcontext) = match context {
            Some((k, v)) => (Some(k), Some(v)),
            None => (None, None),
        };

        let mut typed_entries = Vec::with_capacity(fields.len());
        let mut ok = true;
        for field in fields {
            let ast::FieldName::Key(key_exp) = &field.name else {
                self.error(
                    field.loc,
                    "campo posicional ou nomeado não é válido dentro de um literal de map; use `[chave] = valor`.",
                );
                ok = false;
                continue;
            };
            let typed_key = self.check_exp(key_exp, kcontext);
            let typed_value = self.check_exp(&field.exp, vcontext);
            match (typed_key, typed_value) {
                (Some(k), Some(v)) => typed_entries.push((k, v)),
                _ => ok = false,
            }
        }
        if !ok {
            return None;
        }

        let key_ty = match kcontext {
            Some(ty) => ty.clone(),
            None => match typed_entries.first() {
                Some((k, _)) => k.ty.clone(),
                None => Type::Integer,
            },
        };
        let value_ty = match vcontext {
            Some(ty) => ty.clone(),
            None => match typed_entries.first() {
                Some((_, v)) => v.ty.clone(),
                None => Type::Integer,
            },
        };

        for (k, v) in &typed_entries {
            if !key_ty.compatible(&k.ty) {
                self.error(
                    k.loc,
                    format!(
                        "chave de map incompatível: esperado {}, encontrado {}.",
                        type_name(&key_ty),
                        type_name(&k.ty)
                    ),
                );
                return None;
            }
            if !value_ty.compatible(&v.ty) {
                self.error(
                    v.loc,
                    format!(
                        "valor de map incompatível: esperado {}, encontrado {}.",
                        type_name(&value_ty),
                        type_name(&v.ty)
                    ),
                );
                return None;
            }
        }

        Some(TypedExp {
            loc,
            ty: Type::Map {
                keys: Box::new(key_ty),
                values: Box::new(value_ty),
            },
            kind: TypedExpKind::MapLit(typed_entries),
        })
    }

    /// `Nome{x = 1, y = 2}` como record (T29), espelhando
    /// `checker.lua:738-794`. Exaustivo: todo campo presente, nenhum extra,
    /// nenhum posicional.
    fn check_record_lit(
        &mut self,
        loc: Loc,
        fields: &[ast::Field],
        rname: &str,
        rfields: &[(String, Type)],
    ) -> Option<TypedExp> {
        let mut seen = HashSet::new();
        let mut typed_by_name: Vec<(String, TypedExp)> = Vec::with_capacity(fields.len());
        let mut ok = true;
        for field in fields {
            let fname = match &field.name {
                ast::FieldName::Name(n) => n,
                ast::FieldName::None => {
                    self.error(
                        field.loc,
                        format!(
                            "record '{rname}' não aceita campo posicional; use `nome = valor`."
                        ),
                    );
                    ok = false;
                    continue;
                }
                ast::FieldName::Key(_) => {
                    self.error(
                        field.loc,
                        format!("record '{rname}' não aceita chave-expressão (`[...] = ...`)."),
                    );
                    ok = false;
                    continue;
                }
            };
            let Some((_, expected_ty)) = rfields.iter().find(|(n, _)| n == fname) else {
                self.error(
                    field.loc,
                    format!("campo '{fname}' não existe no record '{rname}'."),
                );
                ok = false;
                continue;
            };
            if !seen.insert(fname.clone()) {
                self.error(
                    field.loc,
                    format!("campo '{fname}' duplicado no construtor de '{rname}'."),
                );
                ok = false;
                continue;
            }
            // Go-to-definition (T49): `nome = valor` num construtor de
            // record também é um uso do campo, não só `p.campo`.
            if let Some(&def_loc) = self.field_def_locs.get(&(rname.to_string(), fname.clone())) {
                self.record_use(field.loc, def_loc, fname, expected_ty);
            }
            match self.check_exp(&field.exp, Some(expected_ty)) {
                Some(typed) => {
                    if !expected_ty.compatible(&typed.ty) {
                        self.error(
                            typed.loc,
                            format!(
                                "campo '{fname}' incompatível: esperado {}, encontrado {}.",
                                type_name(expected_ty),
                                type_name(&typed.ty)
                            ),
                        );
                        ok = false;
                    } else {
                        typed_by_name.push((fname.clone(), typed));
                    }
                }
                None => ok = false,
            }
        }

        for (fname, _) in rfields {
            if !seen.contains(fname) {
                self.error(
                    loc,
                    format!("falta o campo '{fname}' no construtor de '{rname}'."),
                );
                ok = false;
            }
        }

        if !ok {
            return None;
        }

        // Ordem canônica dos campos do record, não a ordem escrita no
        // construtor — o codegen (T32) emite os campos na ordem da
        // declaração do `record`.
        let ordered_fields = rfields
            .iter()
            .map(|(n, _)| {
                let typed = typed_by_name
                    .iter()
                    .find(|(name, _)| name == n)
                    .map(|(_, t)| t.clone())
                    .expect("exaustividade já garantida acima");
                (n.clone(), typed)
            })
            .collect();

        Some(TypedExp {
            loc,
            ty: Type::Record {
                name: rname.to_string(),
                fields: rfields.to_vec(),
            },
            kind: TypedExpKind::RecordLit {
                type_name: rname.to_string(),
                fields: ordered_fields,
            },
        })
    }

    /// Regras de tipo dos operadores binários (T13), espelhando
    /// `checker.lua:910-1122` sem bitwise nem gradual typing.
    fn check_binop(&mut self, loc: Loc, op_str: &str, lhs: &Exp, rhs: &Exp) -> Option<TypedExp> {
        let Some(op) = BinOp::from_source(op_str) else {
            self.error(
                loc,
                format!("operador `{op_str}` não é suportado nesta fase."),
            );
            return None;
        };

        let lhs = self.check_exp(lhs, None)?;
        let rhs = self.check_exp(rhs, None)?;

        let ty = match op {
            // Ambos numéricos; int/int → int, qualquer float promove a float.
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Mod => {
                if !self.check_numeric_operands(op_str, &lhs, &rhs) {
                    return None;
                }
                numeric_result(&lhs.ty, &rhs.ty)
            }
            // `/` e `^` sempre coagem ambos para float — mesmo int/int
            // (`checker.lua:975-994`).
            BinOp::Div | BinOp::Pow => {
                if !self.check_numeric_operands(op_str, &lhs, &rhs) {
                    return None;
                }
                Type::Float
            }
            // Igualdade: número com número (com coerção int→float),
            // string/string ou boolean/boolean.
            BinOp::Eq | BinOp::Ne => {
                let both_numeric = is_numeric(&lhs.ty) && is_numeric(&rhs.ty);
                let same_primitive =
                    lhs.ty.equals(&rhs.ty) && matches!(lhs.ty, Type::String | Type::Boolean);
                if !(both_numeric || same_primitive) {
                    self.error(
                        loc,
                        format!(
                            "não é possível comparar {} com {} usando `{op_str}`.",
                            type_name(&lhs.ty),
                            type_name(&rhs.ty)
                        ),
                    );
                    return None;
                }
                Type::Boolean
            }
            // Ordem: número com número (com coerção) ou string com string —
            // nunca boolean (`checker.lua:1010-1043`).
            BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => {
                let both_numeric = is_numeric(&lhs.ty) && is_numeric(&rhs.ty);
                let both_string = lhs.ty.equals(&Type::String) && rhs.ty.equals(&Type::String);
                if !(both_numeric || both_string) {
                    self.error(
                        loc,
                        format!(
                            "`{op_str}` compara número com número ou string com string, encontrado {} e {}.",
                            type_name(&lhs.ty),
                            type_name(&rhs.ty)
                        ),
                    );
                    return None;
                }
                Type::Boolean
            }
            // Decisão 7 da Fase 1: `and`/`or` boolean estrito nos dois lados,
            // resultado boolean (viram `&&`/`||` no codegen). Divergência
            // deliberada do truthy/falsy do original (`checker.lua:996-1008`):
            // sem `Value`/`Option` em uso nesta fase, o tipo-união que o Lua
            // devolveria não tem representação útil aqui.
            BinOp::And | BinOp::Or => {
                let mut ok = true;
                for side in [&lhs, &rhs] {
                    if !side.ty.equals(&Type::Boolean) {
                        self.error(
                            side.loc,
                            format!(
                                "operando de `{op_str}` precisa ser boolean, encontrado {}.",
                                type_name(&side.ty)
                            ),
                        );
                        ok = false;
                    }
                }
                if !ok {
                    return None;
                }
                Type::Boolean
            }
        };

        Some(TypedExp {
            loc,
            ty,
            kind: TypedExpKind::Binop {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            },
        })
    }

    /// Reporta um erro por lado não-numérico de um operador aritmético,
    /// apontando o `loc` do operando culpado.
    fn check_numeric_operands(&mut self, op_str: &str, lhs: &TypedExp, rhs: &TypedExp) -> bool {
        let mut ok = true;
        for side in [lhs, rhs] {
            if !is_numeric(&side.ty) {
                self.error(
                    side.loc,
                    format!(
                        "operando de `{op_str}` precisa ser numérico (integer ou float), encontrado {}.",
                        type_name(&side.ty)
                    ),
                );
                ok = false;
            }
        }
        ok
    }

    /// Regras de tipo dos operadores unários (T13/T29): `-` numérico preserva
    /// o tipo do operando; `not` é boolean → boolean (`checker.lua:1100-1122`);
    /// `#` (`checker.lua:852-859`) sobre `Array`/`String` resulta `Integer` —
    /// `parser::parse_unary_exp` produz `#` como prefixo de expressão desde a
    /// T30 (lacuna do parser fechada ali; `check_unop` já sabia mapear `"#"`
    /// desde a T29).
    fn check_unop(&mut self, loc: Loc, op_str: &str, exp: &Exp) -> Option<TypedExp> {
        let op = match op_str {
            "-" => UnOp::Neg,
            "not" => UnOp::Not,
            "#" => UnOp::Len,
            // O parser (T11) só produz `-` e `not`; defensivo para AST
            // montada à mão (`~`).
            _ => {
                self.error(
                    loc,
                    format!("operador unário `{op_str}` não é suportado nesta fase."),
                );
                return None;
            }
        };

        let exp = self.check_exp(exp, None)?;
        let ty = match op {
            UnOp::Neg => {
                if !is_numeric(&exp.ty) {
                    self.error(
                        exp.loc,
                        format!(
                            "operando de `-` unário precisa ser numérico (integer ou float), encontrado {}.",
                            type_name(&exp.ty)
                        ),
                    );
                    return None;
                }
                exp.ty.clone()
            }
            UnOp::Not => {
                if !exp.ty.equals(&Type::Boolean) {
                    self.error(
                        exp.loc,
                        format!(
                            "operando de `not` precisa ser boolean, encontrado {}.",
                            type_name(&exp.ty)
                        ),
                    );
                    return None;
                }
                Type::Boolean
            }
            UnOp::Len => {
                if !matches!(exp.ty, Type::Array { .. } | Type::String) {
                    self.error(
                        exp.loc,
                        format!(
                            "`#` espera um array ou string, encontrado {}.",
                            type_name(&exp.ty)
                        ),
                    );
                    return None;
                }
                Type::Integer
            }
        };

        Some(TypedExp {
            loc,
            ty,
            kind: TypedExpKind::Unop {
                op,
                exp: Box::new(exp),
            },
        })
    }

    /// Tipa um `Var` em posição de leitura (`ExpVar`) — `VarBracket`/`VarDot`
    /// espelham `checker.lua:541-564` e `:482-539` (T29). Chamada/acesso
    /// qualificados a membro de módulo (`data.f(...)`, `df.f(...)`) seguem
    /// fora de escopo até T39/T40.
    fn check_var(&mut self, _loc: &Loc, var: &Var) -> Option<TypedExp> {
        match var {
            Var::VarName { loc, name } => match self.st.find_symbol(name).cloned() {
                // Módulo (T38): só existe para `data.f(...)`/`data.Tipo`
                // (T39/T40) resolverem contra a tabela de capabilities — não
                // é um valor, então usá-lo sozinho (`local x = data`) é
                // erro claro em vez de vazar `Type::Invalid`.
                Some(Symbol {
                    kind: SymbolKind::Module { .. },
                    ..
                }) => {
                    self.error(*loc, format!("'{name}' é um módulo, não um valor."));
                    None
                }
                Some(symbol) => {
                    self.record_use(*loc, symbol.def_loc, name, &symbol.ty);
                    Some(TypedExp {
                        loc: *loc,
                        ty: symbol.ty,
                        kind: TypedExpKind::Var(name.clone()),
                    })
                }
                None => {
                    self.error(*loc, format!("'{name}' não foi declarado."));
                    None
                }
            },
            Var::VarBracket { loc, exp1, exp2 } => {
                let base = self.check_exp(exp1, None)?;
                let (keys_ty, result_ty) = match &base.ty {
                    Type::Array { elem } => (Type::Integer, elem.as_ref().clone()),
                    Type::Map { keys, values } => (keys.as_ref().clone(), values.as_ref().clone()),
                    Type::String => {
                        self.error(
                            base.loc,
                            "não é possível indexar uma string com `[]` nesta fase.",
                        );
                        return None;
                    }
                    other => {
                        self.error(
                            base.loc,
                            format!("não é possível indexar {}.", type_name(other)),
                        );
                        return None;
                    }
                };
                let index = self.check_exp(exp2, Some(&keys_ty))?;
                if !keys_ty.compatible(&index.ty) {
                    self.error(
                        index.loc,
                        format!(
                            "índice incompatível: esperado {}, encontrado {}.",
                            type_name(&keys_ty),
                            type_name(&index.ty)
                        ),
                    );
                    return None;
                }
                Some(TypedExp {
                    loc: *loc,
                    // Decisão 3 do PRD.md (T29): o resultado é `T`, não `T?`
                    // — sem `Option` nesta fase.
                    ty: result_ty,
                    kind: TypedExpKind::Index {
                        base: Box::new(base),
                        index: Box::new(index),
                    },
                })
            }
            Var::VarDot { loc, exp, name } => {
                let base = self.check_exp(exp, None)?;
                if let Type::Opaque { .. } = &base.ty {
                    self.error(
                        base.loc,
                        format!(
                            "'{}' não tem campos acessíveis: é um tipo opaco, só é \
                             possível chamar métodos sobre ele.",
                            type_name(&base.ty)
                        ),
                    );
                    return None;
                }
                let Type::Record { name: rname, fields } = &base.ty else {
                    self.error(
                        base.loc,
                        format!(
                            "só é possível acessar campo de um record, encontrado {}.",
                            type_name(&base.ty)
                        ),
                    );
                    return None;
                };
                let Some((_, field_ty)) = fields.iter().find(|(fname, _)| fname == name) else {
                    self.error(
                        *loc,
                        format!("o record '{rname}' não tem campo '{name}'."),
                    );
                    return None;
                };
                let field_ty = field_ty.clone();
                // Go-to-definition (T49): salta para a declaração do campo
                // dentro do `record ... end`, não para o record inteiro.
                if let Some(&def_loc) = self
                    .field_def_locs
                    .get(&(rname.clone(), name.clone()))
                {
                    self.record_use(*loc, def_loc, name, &field_ty);
                }
                Some(TypedExp {
                    loc: *loc,
                    ty: field_ty,
                    kind: TypedExpKind::Field {
                        base: Box::new(base),
                        name: name.clone(),
                    },
                })
            }
        }
    }

    /// Resolve a quem uma chamada se refere — extraído de `check_call` (T39)
    /// antes de acrescentar o segundo braço (`VarDot` de módulo), no
    /// espírito do risco 4 do PRD.md: a função já tinha ~120 linhas.
    /// Devolve o `Callee` já resolvido, um nome para mensagens de erro, e a
    /// assinatura (`params`/`rettypes`) contra a qual tipar os argumentos.
    fn resolve_callee(
        &mut self,
        loc: &Loc,
        callee: &Exp,
    ) -> Option<(Callee, String, Vec<Type>, Vec<Type>)> {
        let Exp::ExpVar { var, .. } = callee else {
            self.error(
                *loc,
                "só é possível chamar um nome de função diretamente nesta fase.",
            );
            return None;
        };
        match var.as_ref() {
            Var::VarName { loc: name_loc, name } => {
                let Some(symbol) = self.st.find_symbol(name).cloned() else {
                    self.error(*loc, format!("função '{name}' não foi declarada."));
                    return None;
                };
                let def_loc = symbol.def_loc;
                let Type::Function { params, rettypes } = symbol.ty else {
                    self.error(*loc, format!("'{name}' não é uma função."));
                    return None;
                };
                // `*name_loc` (posição do nome), não `*loc` (posição de toda
                // a expressão de chamada, `f(...)` — T49 precisa do range do
                // identificador, não do parêntese em diante).
                self.record_use(
                    *name_loc,
                    def_loc,
                    name,
                    &Type::Function {
                        params: params.clone(),
                        rettypes: rettypes.clone(),
                    },
                );
                Some((Callee::Direct(name.clone()), name.clone(), params, rettypes))
            }
            // `data.read_csv(...)` (T39): base é o símbolo de um módulo
            // importado — resolve contra a tabela de capabilities em vez da
            // pilha de escopos.
            Var::VarDot { exp, name, .. } if self.dot_base_module(exp).is_some() => {
                let module = self.dot_base_module(exp).expect("checado acima");
                let capability = *self
                    .modules
                    .get(&module)
                    .expect("dot_base_module só devolve módulo importado");
                let Some(function) = capability.find_function(name) else {
                    self.error(
                        *loc,
                        format!("o módulo '{module}' não tem função '{name}'."),
                    );
                    return None;
                };
                Some((
                    Callee::Module {
                        module: module.clone(),
                        name: name.clone(),
                    },
                    format!("{module}.{name}"),
                    function.params.to_vec(),
                    vec![requalify_rettype(&function.rettype, &module)],
                ))
            }
            // `df.soma(...)` (T40): base é uma expressão cujo *tipo* é
            // `Opaque` — resolve o método contra a capability do módulo que
            // originou o opaco (`Type::Opaque::module`, preenchido por
            // `requalify_rettype` em T39). O receptor conta como uso mutável
            // pela mesma regra de `checker.rs:2079-2110` (`is_composite`
            // inclui `Opaque` — ver `is_composite` abaixo).
            Var::VarDot { exp, name, .. } => {
                let receiver = self.check_exp(exp, None)?;
                let Type::Opaque {
                    module,
                    name: type_name_,
                    ..
                } = &receiver.ty
                else {
                    self.error(
                        *loc,
                        format!(
                            "só é possível chamar um nome de função diretamente nesta fase, \
                             encontrado {}.",
                            type_name(&receiver.ty)
                        ),
                    );
                    return None;
                };
                let capability = crate::capabilities::lookup_module(module)
                    .expect("Opaque só é construído com módulo de capability existente");
                let Some(method) = capability.find_method(type_name_, name) else {
                    self.error(
                        *loc,
                        format!("o tipo '{module}.{type_name_}' não tem método '{name}'."),
                    );
                    return None;
                };
                let module = module.clone();
                let recv_name = format!("{module}.{type_name_}");
                if let Exp::ExpVar { var, .. } = exp.as_ref()
                    && let Some(root_name) = root_var_name(var)
                    && let Some(Symbol {
                        kind: SymbolKind::Local { decl_id },
                        ..
                    }) = self.st.find_symbol(&root_name)
                {
                    self.assigned.insert(*decl_id);
                }
                Some((
                    Callee::Method {
                        recv: Box::new(receiver),
                        module: module.clone(),
                        name: name.clone(),
                    },
                    format!("{recv_name}.{name}"),
                    method.params.to_vec(),
                    vec![requalify_rettype(&method.rettype, &module)],
                ))
            }
            _ => {
                self.error(
                    *loc,
                    "só é possível chamar um nome de função diretamente nesta fase.",
                );
                None
            }
        }
    }

    /// Se `exp` é `ExpVar(VarName(nome))` e `nome` está registrado como
    /// módulo importado, devolve o nome do módulo — usado por
    /// `resolve_callee` para reconhecer a base de `data.read_csv(...)`
    /// (T39) e, mais adiante, distingui-la da base opaca de `df.soma(...)`
    /// (T40).
    fn dot_base_module(&self, exp: &Exp) -> Option<String> {
        let Exp::ExpVar { var, .. } = exp else {
            return None;
        };
        let Var::VarName { name, .. } = var.as_ref() else {
            return None;
        };
        match self.st.find_symbol(name) {
            Some(Symbol {
                kind: SymbolKind::Module { .. },
                ..
            }) => Some(name.clone()),
            _ => None,
        }
    }

    fn check_call(&mut self, loc: &Loc, callee: &Exp, args: &Args) -> Option<TypedExp> {
        let Args::ArgsFunc { args: arg_exps, .. } = args else {
            self.error(*loc, "chamada de método não é suportada nesta fase.");
            return None;
        };

        let (callee, name, params, rettypes) = self.resolve_callee(loc, callee)?;

        let mut typed_args = Vec::with_capacity(arg_exps.len());
        let mut ok = true;
        for (i, arg) in arg_exps.iter().enumerate() {
            match self.check_exp(arg, params.get(i)) {
                Some(typed) => typed_args.push(typed),
                None => ok = false,
            }
        }
        if !ok {
            return None;
        }

        // Duplo empréstimo mutável (T29): passar a mesma variável composta
        // duas vezes na mesma chamada (`f(xs, xs)`) geraria `cannot borrow
        // as mutable more than once` no Rust gerado — rejeitado aqui com
        // mensagem em português em vez de deixar o `rustc` recusar. Também
        // marca cada raiz composta como usada mutavelmente (mesmo
        // espírito de `check_assign`): passar um array/map/record a uma
        // função é uso mutável sob `&mut`.
        let mut seen_composite_roots: Vec<String> = Vec::new();
        for (arg_exp, typed_arg) in arg_exps.iter().zip(&typed_args) {
            if !is_composite(&typed_arg.ty) {
                continue;
            }
            let Exp::ExpVar { var, .. } = arg_exp else {
                continue;
            };
            let Some(root_name) = root_var_name(var) else {
                continue;
            };
            if seen_composite_roots.contains(&root_name) {
                self.error(
                    typed_arg.loc,
                    format!(
                        "não é possível passar '{root_name}' duas vezes na mesma chamada: \
                         empréstimo mutável duplicado."
                    ),
                );
                return None;
            }
            if let Some(Symbol {
                kind: SymbolKind::Local { decl_id },
                ..
            }) = self.st.find_symbol(&root_name)
            {
                self.assigned.insert(*decl_id);
            }
            seen_composite_roots.push(root_name);
        }

        if typed_args.len() != params.len() {
            self.error(
                *loc,
                format!(
                    "'{name}' espera {} argumento(s), mas recebeu {}.",
                    params.len(),
                    typed_args.len()
                ),
            );
            return None;
        }

        for (arg, expected) in typed_args.iter().zip(&params) {
            if !expected.compatible(&arg.ty) {
                self.error(
                    arg.loc,
                    format!(
                        "argumento incompatível em chamada de '{name}': esperado {}, encontrado {}.",
                        type_name(expected),
                        type_name(&arg.ty)
                    ),
                );
                return None;
            }
        }

        // A Fase 0 só produz uma função de retorno único; `rettypes[0]`
        // sempre existe porque toda assinatura coletada tem ao menos um tipo
        // de retorno (`TypeNil` quando omitido).
        let ty = rettypes.first().cloned().unwrap_or(Type::Nil);

        Some(TypedExp {
            loc: *loc,
            ty,
            kind: TypedExpKind::Call {
                callee,
                args: typed_args,
            },
        })
    }
}

/// Fix-up de mutabilidade (decisão 6 da Fase 1): percorre o corpo tipado da
/// função marcando `mutable = true` nas declarações cujo `decl_id` recebeu
/// alguma atribuição. Shadowing é respeitado naturalmente — o id registrado
/// veio do símbolo resolvido na pilha de escopos.
fn fixup_mutability(stat: &mut TypedStat, assigned: &HashSet<DeclId>) {
    match stat {
        TypedStat::Block { stats, .. } => {
            for s in stats {
                fixup_mutability(s, assigned);
            }
        }
        TypedStat::Decl {
            decl_id, mutable, ..
        } => {
            *mutable = assigned.contains(decl_id);
        }
        TypedStat::If {
            thens, elsestat, ..
        } => {
            for then in thens {
                fixup_mutability(&mut then.block, assigned);
            }
            if let Some(stat) = elsestat {
                fixup_mutability(stat, assigned);
            }
        }
        TypedStat::While { block, .. } | TypedStat::For { block, .. } => {
            fixup_mutability(block, assigned);
        }
        TypedStat::Call { .. } | TypedStat::Return { .. } | TypedStat::Assign { .. } => {}
    }
}

/// `true` para os tipos que participam da aritmética e da coerção int→float.
fn is_numeric(ty: &Type) -> bool {
    matches!(ty, Type::Integer | Type::Float)
}

/// `true` para os tipos passados por `&mut` no Rust gerado (T29): um
/// parâmetro composto aceita `xs[i] = v`, e passá-lo a outra função é uso
/// mutável (`check_call` insere seu `DeclId` em `assigned`). Consulta apenas
/// o tipo, sem inflar `SymbolKind` com mais uma variante. `Opaque` entra na
/// T40: o receptor de `df.soma(...)` é `&mut` pelo mesmo motivo.
fn is_composite(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Array { .. } | Type::Map { .. } | Type::Record { .. } | Type::Opaque { .. }
    )
}

/// Preenche o placeholder `Type::Opaque` vazio de `CapabilityFn::rettype`
/// (`capabilities.rs`, T39: uma declaração `const` não constrói `String`
/// não-vazia) com o `module` real da chamada — `name`/`rust_path` vêm do
/// tipo opaco correspondente na mesma capability. Tipos não-opacos (`Float`
/// em `soma`, por exemplo) passam adiante sem mudança.
fn requalify_rettype(rettype: &Type, module: &str) -> Type {
    let Type::Opaque { name, .. } = rettype else {
        return rettype.clone();
    };
    let capability =
        crate::capabilities::lookup_module(module).expect("módulo já resolvido pelo chamador");
    let opaque = capability
        .find_opaque(name)
        .or_else(|| capability.opaque_types.first())
        .expect("capability com rettype opaco precisa expor ao menos um tipo opaco");
    Type::Opaque {
        module: module.to_string(),
        name: opaque.titan_name.to_string(),
        rust_path: opaque.rust_path.to_string(),
    }
}

/// Desce a cadeia de `VarBracket`/`VarDot` (`v[i]`, `p.campo`,
/// `m[i].campo[j]`) até achar o `VarName` raiz — usado por `check_assign`
/// para descobrir qual variável-raiz uma atribuição indexada/de campo
/// alcança (T29, decisão de mutabilidade composta: `v[i]=x` e `p.campo=x`
/// marcam a variável-raiz como mutável, cobrindo aninhamento).
fn root_var_name(var: &Var) -> Option<String> {
    match var {
        Var::VarName { name, .. } => Some(name.clone()),
        Var::VarBracket { exp1, .. } => root_exp_var_name(exp1),
        Var::VarDot { exp, .. } => root_exp_var_name(exp),
    }
}

fn root_exp_var_name(exp: &Exp) -> Option<String> {
    match exp {
        Exp::ExpVar { var, .. } => root_var_name(var),
        _ => None,
    }
}

/// Coerção numérica int→float centralizada (T13): o tipo resultante de
/// combinar dois operandos **já validados** como numéricos — `Integer` só
/// quando os dois lados são `Integer`; qualquer `Float` promove o resultado
/// a `Float`. O checker não emite nó de cast: o codegen compara o tipo do
/// operando com o do resultado para decidir o `as f64`.
fn numeric_result(lhs: &Type, rhs: &Type) -> Type {
    if lhs.equals(&Type::Integer) && rhs.equals(&Type::Integer) {
        Type::Integer
    } else {
        Type::Float
    }
}

/// Formata um tipo para mensagem de erro — e, desde a T49, para hover do
/// LSP, que quer exatamente o mesmo texto que o checker já usa.
pub fn type_name(ty: &Type) -> String {
    match ty {
        Type::Invalid => "<inválido>".to_string(),
        Type::Nil => "nil".to_string(),
        Type::Boolean => "boolean".to_string(),
        Type::Integer => "integer".to_string(),
        Type::Float => "float".to_string(),
        Type::String => "string".to_string(),
        Type::Value => "value".to_string(),
        Type::Function { .. } => "function".to_string(),
        Type::Array { elem } => format!("{{{}}}", type_name(elem)),
        Type::Map { keys, values } => format!("map {{{}: {}}}", type_name(keys), type_name(values)),
        Type::Record { name, .. } => name.clone(),
        Type::Option { base } => format!("{}?", type_name(base)),
        Type::Opaque { module, name, .. } => format!("{}.{}", module, name),
    }
}

/// Verifica o programa por completo, produzindo a AST tipada (mais o índice
/// de usos da T49, em [`CheckedProgram::uses`]) em caso de sucesso.
///
/// Nunca panic: qualquer construção fora do subconjunto suportado, ou erro de
/// tipo, vira uma entrada em `Err`.
pub fn check(program: &Program) -> Result<CheckedProgram, Vec<CheckError>> {
    let mut checker = Checker::new();

    // Records primeiro (T29): uma função pode receber um record declarado
    // mais adiante no arquivo.
    checker.collect_records(program);
    for node in program {
        checker.collect_signature(node);
    }

    checker.check_has_main(program);

    let mut typed_program = Vec::with_capacity(program.len());
    for node in program {
        if let Some(typed) = checker.check_toplevel(node) {
            typed_program.push(typed);
        }
    }

    if checker.errors.is_empty() {
        Ok(CheckedProgram {
            program: typed_program,
            uses: checker.uses,
        })
    } else {
        Err(checker.errors)
    }
}

/// Módulos importados por `import` no programa (T43), na ordem em que
/// aparecem — usado pelo driver para montar as dependências do `Cargo.toml`
/// gerado. Só é chamado depois de `check` ter aceitado o programa (nenhum
/// erro de import), então cada `modname` resolve contra
/// [`crate::capabilities::lookup_module`]; duplicatas (rejeitadas pelo
/// checker) não se repetem aqui.
pub fn imported_capabilities(program: &Program) -> Vec<&'static crate::capabilities::Capability> {
    let mut seen = std::collections::HashSet::new();
    program
        .iter()
        .filter_map(|node| match node {
            TopLevel::TopLevelImport { modname, .. } => {
                if !seen.insert(modname.clone()) {
                    return None;
                }
                crate::capabilities::lookup_module(modname)
            }
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::Decl;
    use crate::lexer::lex;
    use crate::parser::parse;

    fn check_source(source: &str) -> Result<TypedProgram, Vec<CheckError>> {
        let tokens =
            lex(source).unwrap_or_else(|e| panic!("fonte não deveria ter erro léxico: {e}"));
        let program =
            parse(&tokens).unwrap_or_else(|e| panic!("fonte não deveria ter erro sintático: {e}"));
        check(&program).map(|checked| checked.program)
    }

    #[test]
    fn type_name_de_opaco_e_qualificado_por_modulo() {
        let df = Type::Opaque {
            module: "data".to_string(),
            name: "DataFrame".to_string(),
            rust_path: "titan_data::DataFrame".to_string(),
        };
        assert_eq!(type_name(&df), "data.DataFrame");
    }

    #[test]
    fn aceita_hello_titan() {
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/hello.titan"
        ))
        .expect("examples/hello.titan deve existir");

        let typed = check_source(&source).unwrap_or_else(|errs| {
            panic!(
                "esperava sucesso, obteve erros: {}",
                errs.iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        });

        assert_eq!(typed.len(), 1);
        let TypedTopLevel::Func {
            name,
            params,
            rettypes,
            ..
        } = &typed[0]
        else {
            panic!("esperava TypedTopLevel::Func, obteve {:?}", typed[0]);
        };
        assert_eq!(name, "main");
        assert_eq!(params.len(), 1);
        assert_eq!(
            params[0].1,
            Type::Array {
                elem: Box::new(Type::String)
            }
        );
        assert_eq!(rettypes, &vec![Type::Integer]);
    }

    #[test]
    fn print_com_argumento_incompativel_produz_erro() {
        let source = "function main(args: {string}): integer\n    print(42)\n    return 0\nend";
        let errs = check_source(source).unwrap_err();
        assert!(errs.iter().any(|e| e.message.contains("incompatível")));
    }

    #[test]
    fn chamada_a_funcao_nao_declarada_produz_erro() {
        let source =
            "function main(args: {string}): integer\n    funcao_inexistente()\n    return 0\nend";
        let errs = check_source(source).unwrap_err();
        assert!(errs.iter().any(|e| e.message.contains("não foi declarada")));
    }

    #[test]
    fn main_retornando_string_produz_erro_de_retorno_incompativel() {
        let source = "function main(args: {string}): integer\n    return \"oi\"\nend";
        let errs = check_source(source).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("retorno incompatível"))
        );
    }

    #[test]
    fn assinatura_de_main_invalida_produz_erro() {
        let source = "function main(): integer\n    return 0\nend";
        let errs = check_source(source).unwrap_err();
        assert!(errs.iter().any(|e| e.message.contains("main")));
    }

    #[test]
    fn atribuicao_multipla_montada_a_mao_produz_erro_defensivo() {
        // O parser (T11) nunca produz multi-assign — AST montada à mão para
        // exercitar a rejeição defensiva do checker (PRD.md, T12).
        let loc = Loc { line: 1, col: 1 };
        let program: Program = vec![TopLevel::TopLevelFunc {
            loc,
            islocal: false,
            name: "main".to_string(),
            params: vec![Decl {
                loc,
                name: "args".to_string(),
                r#type: Some(ast::Type::TypeArray {
                    loc,
                    subtype: Box::new(ast::Type::TypeString { loc }),
                }),
                option: false,
            }],
            rettypes: vec![ast::Type::TypeInteger { loc }],
            block: Stat::StatBlock {
                loc,
                stats: vec![Stat::StatAssign {
                    loc,
                    vars: vec![
                        Var::VarName {
                            loc,
                            name: "a".to_string(),
                        },
                        Var::VarName {
                            loc,
                            name: "b".to_string(),
                        },
                    ],
                    exps: vec![
                        Exp::ExpInteger { loc, value: 1 },
                        Exp::ExpInteger { loc, value: 2 },
                    ],
                }],
            },
        }];

        let errs = check(&program).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("atribuição múltipla"))
        );
    }

    #[test]
    fn foreign_import_produz_erro_de_construcao_nao_suportada() {
        // Mesmo espírito de um arquivo `.titan` do Titan original:
        // `foreign import` não faz parte do subconjunto desta fase. `record`
        // passou a ser aceito a partir da T29 — coberto por
        // `record_vazio_e_aceito_pelo_checker` mais abaixo.
        let loc = Loc { line: 1, col: 1 };
        let program: Program = vec![
            TopLevel::TopLevelForeignImport {
                loc,
                localname: "stdio".to_string(),
                headername: "stdio.h".to_string(),
            },
            TopLevel::TopLevelRecord {
                loc,
                name: "Ponto".to_string(),
                fields: vec![],
            },
        ];

        let errs = check(&program).unwrap_err();
        assert!(errs.iter().any(|e| e.message.contains("foreign import")));
    }

    // ---- T38: `import data` registra o módulo -----------------------------

    #[test]
    fn import_data_e_aceito_e_registra_o_simbolo() {
        let source =
            "import data\n\nfunction main(args: {string}): integer\n    return 0\nend";
        let typed = check_source(source).unwrap_or_else(|errs| {
            panic!(
                "esperava sucesso, obteve erros: {}",
                errs.iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        });
        assert_eq!(typed.len(), 1);
    }

    #[test]
    fn parametro_com_tipo_qualificado_do_modulo_importado_resolve() {
        // A T38 só cobre a resolução do tipo `data.DataFrame` em anotações —
        // construir um valor desse tipo (`data.read_csv(...)`) é a T39. Um
        // parâmetro tipado exercita `resolve_type`/`TypeQualName` sem
        // precisar de um valor atribuível ainda.
        let source = r#"import data

function usa(df: data.DataFrame): integer
    return 0
end

function main(args: {string}): integer
    return 0
end"#;
        let typed = check_source(source).unwrap_or_else(|errs| {
            panic!(
                "esperava sucesso, obteve erros: {}",
                errs.iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        });
        let TypedTopLevel::Func { params, .. } = &typed[0] else {
            panic!("esperava TypedTopLevel::Func, obteve {:?}", typed[0]);
        };
        assert_eq!(
            params[0].1,
            Type::Opaque {
                module: "data".to_string(),
                name: "DataFrame".to_string(),
                rust_path: "titan_data::DataFrame".to_string(),
            }
        );
    }

    #[test]
    fn import_de_capability_inexistente_produz_erro_com_lista_de_disponiveis() {
        let source =
            "import inexistente\n\nfunction main(args: {string}): integer\n    return 0\nend";
        let errs = check_source(source).unwrap_err();
        assert!(errs.iter().any(|e| {
            e.message.contains("capability 'inexistente' não existe")
                && e.message.contains("disponíveis")
                && e.message.contains("data")
        }));
    }

    #[test]
    fn import_duplicado_produz_erro_claro() {
        let source =
            "import data\nimport data\n\nfunction main(args: {string}): integer\n    return 0\nend";
        let errs = check_source(source).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("'data' já foi declarado antes"))
        );
    }

    #[test]
    fn tipo_qualificado_de_modulo_nao_importado_produz_erro_claro() {
        let source = r#"function main(args: {string}): integer
    local df: data.DataFrame = nil
    return 0
end"#;
        let errs = check_source(source).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("módulo 'data' não foi importado"))
        );
    }

    #[test]
    fn tipo_inexistente_no_modulo_importado_produz_erro_distinto() {
        let source = r#"import data

function main(args: {string}): integer
    local s: data.Series = nil
    return 0
end"#;
        let errs = check_source(source).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("o módulo 'data' não tem o tipo 'Series'"))
        );
    }

    #[test]
    fn atribuir_a_um_modulo_importado_produz_erro_claro() {
        let source = r#"import data

function main(args: {string}): integer
    data = 1
    return 0
end"#;
        let errs = check_source(source).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("não é possível atribuir ao módulo 'data'"))
        );
    }

    #[test]
    fn usar_modulo_importado_como_valor_produz_erro_claro() {
        let source = r#"import data

function main(args: {string}): integer
    local x = data
    return 0
end"#;
        let errs = check_source(source).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("'data' é um módulo, não um valor"))
        );
    }

    // ---- T39: chamada qualificada `data.f(...)` ---------------------------

    #[test]
    fn chamada_qualificada_de_funcao_de_modulo_e_aceita_e_tipa_o_opaco() {
        let source = r#"import data

function main(args: {string}): integer
    local df: data.DataFrame = data.read_csv("v.csv")
    return 0
end"#;
        let typed = check_source(source).unwrap_or_else(|errs| {
            panic!(
                "esperava sucesso, obteve erros: {}",
                errs.iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        });
        let TypedTopLevel::Func { body, .. } = &typed[0] else {
            panic!("esperava TypedTopLevel::Func, obteve {:?}", typed[0]);
        };
        let TypedStat::Block { stats, .. } = body.as_ref() else {
            panic!("esperava TypedStat::Block, obteve {body:?}");
        };
        let TypedStat::Decl { value, .. } = &stats[0] else {
            panic!("esperava TypedStat::Decl, obteve {:?}", stats[0]);
        };
        assert_eq!(
            value.ty,
            Type::Opaque {
                module: "data".to_string(),
                name: "DataFrame".to_string(),
                rust_path: "titan_data::DataFrame".to_string(),
            }
        );
        assert!(matches!(
            &value.kind,
            TypedExpKind::Call {
                callee: Callee::Module { module, name },
                ..
            } if module == "data" && name == "read_csv"
        ));
    }

    #[test]
    fn funcao_inexistente_no_modulo_produz_erro_claro() {
        let source = r#"import data

function main(args: {string}): integer
    local df: data.DataFrame = data.foo("v.csv")
    return 0
end"#;
        let errs = check_source(source).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("o módulo 'data' não tem função 'foo'"))
        );
    }

    #[test]
    fn chamada_qualificada_com_aridade_errada_produz_erro() {
        let source = r#"import data

function main(args: {string}): integer
    local df: data.DataFrame = data.read_csv("v.csv", "extra")
    return 0
end"#;
        let errs = check_source(source).unwrap_err();
        assert!(errs.iter().any(|e| e.message.contains("espera")));
    }

    #[test]
    fn chamada_qualificada_com_argumento_de_tipo_errado_produz_erro() {
        let source = r#"import data

function main(args: {string}): integer
    local df: data.DataFrame = data.read_csv(42)
    return 0
end"#;
        let errs = check_source(source).unwrap_err();
        assert!(errs.iter().any(|e| e.message.contains("incompatível")));
    }

    // ---- T40: método sobre tipo opaco `df.f(...)` -------------------------

    #[test]
    fn metodo_sobre_opaco_e_aceito_e_tipa_float() {
        let source = r#"import data

function main(args: {string}): integer
    local df: data.DataFrame = data.read_csv("v.csv")
    local total: float = df.soma("valor")
    return 0
end"#;
        let typed = check_source(source).unwrap_or_else(|errs| {
            panic!(
                "esperava sucesso, obteve erros: {}",
                errs.iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        });
        let TypedTopLevel::Func { body, .. } = &typed[0] else {
            panic!("esperava TypedTopLevel::Func, obteve {:?}", typed[0]);
        };
        let TypedStat::Block { stats, .. } = body.as_ref() else {
            panic!("esperava TypedStat::Block, obteve {body:?}");
        };
        let TypedStat::Decl { value, .. } = &stats[1] else {
            panic!("esperava TypedStat::Decl, obteve {:?}", stats[1]);
        };
        assert_eq!(value.ty, Type::Float);
        assert!(matches!(
            &value.kind,
            TypedExpKind::Call {
                callee: Callee::Method { module, name, .. },
                ..
            } if module == "data" && name == "soma"
        ));
    }

    #[test]
    fn metodo_inexistente_no_opaco_produz_erro_distinto_de_funcao_de_modulo() {
        let source = r#"import data

function main(args: {string}): integer
    local df: data.DataFrame = data.read_csv("v.csv")
    local total: float = df.foo("valor")
    return 0
end"#;
        let errs = check_source(source).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("'data.DataFrame' não tem método 'foo'"))
        );
        // Mensagem distinta da de função de módulo inexistente (T39).
        assert!(
            errs.iter()
                .all(|e| !e.message.contains("não tem função"))
        );
    }

    #[test]
    fn acessar_campo_de_opaco_e_rejeitado_com_mensagem_especifica() {
        let source = r#"import data

function main(args: {string}): integer
    local df: data.DataFrame = data.read_csv("v.csv")
    local x = df.campo
    return 0
end"#;
        let errs = check_source(source).unwrap_err();
        assert!(errs.iter().any(|e| {
            e.message.contains("não tem campos acessíveis") && e.message.contains("tipo opaco")
        }));
    }

    #[test]
    fn metodo_com_argumento_de_tipo_errado_produz_erro() {
        let source = r#"import data

function main(args: {string}): integer
    local df: data.DataFrame = data.read_csv("v.csv")
    local total: float = df.soma(42)
    return 0
end"#;
        let errs = check_source(source).unwrap_err();
        assert!(errs.iter().any(|e| e.message.contains("incompatível")));
    }

    #[test]
    fn chamada_antes_da_declaracao_e_permitida() {
        let source = r#"function chamador(): integer
    return chamado()
end

function chamado(): integer
    return 1
end"#;
        // `chamador` está definido antes de `chamado`, mas a passada 1 já
        // coletou todas as assinaturas — não deve haver erro de "main"
        // fora daqui, então filtramos essa mensagem específica.
        let result = check_source(source);
        match result {
            Ok(_) => panic!("esperava erro só por falta de 'main' válida"),
            Err(errs) => {
                assert!(errs.iter().all(|e| e.message.contains("main")));
            }
        }
    }

    // ---- Fase 1 (T12): if / while / for / atribuição --------------------

    /// Verifica `source` e devolve os statements tipados do corpo da
    /// primeira função.
    fn typed_body_stats(source: &str) -> Vec<TypedStat> {
        let typed = check_source(source).unwrap_or_else(|errs| {
            panic!(
                "esperava sucesso, obteve erros: {}",
                errs.iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        });
        let TypedTopLevel::Func { body, .. } = &typed[0] else {
            panic!("esperava TypedTopLevel::Func, obteve {:?}", typed[0]);
        };
        let TypedStat::Block { stats, .. } = body.as_ref() else {
            panic!("esperava TypedStat::Block como corpo");
        };
        stats.clone()
    }

    #[test]
    fn aceita_if_while_for_e_atribuicao() {
        let stats = typed_body_stats(
            "function main(args: {string}): integer\n\
             \x20   local x: integer = 0\n\
             \x20   if true then\n\
             \x20       x = 1\n\
             \x20   else\n\
             \x20       x = 2\n\
             \x20   end\n\
             \x20   while false do\n\
             \x20       x = 3\n\
             \x20   end\n\
             \x20   for i = 1, 10 do\n\
             \x20       x = 4\n\
             \x20   end\n\
             \x20   return x\n\
             end",
        );

        let TypedStat::If {
            thens, elsestat, ..
        } = &stats[1]
        else {
            panic!("esperava TypedStat::If, obteve {:?}", stats[1]);
        };
        assert_eq!(thens.len(), 1);
        assert_eq!(thens[0].condition.ty, Type::Boolean);
        assert!(elsestat.is_some());

        let TypedStat::While { condition, .. } = &stats[2] else {
            panic!("esperava TypedStat::While, obteve {:?}", stats[2]);
        };
        assert_eq!(condition.ty, Type::Boolean);

        let TypedStat::For { ty, inc, .. } = &stats[3] else {
            panic!("esperava TypedStat::For, obteve {:?}", stats[3]);
        };
        assert_eq!(*ty, Type::Integer);
        assert!(matches!(inc.kind, TypedExpKind::Integer(1)));
    }

    #[test]
    fn condicao_de_if_nao_boolean_produz_erro() {
        let source =
            "function main(args: {string}): integer\n    if 42 then\n    end\n    return 0\nend";
        let errs = check_source(source).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("condição") && e.message.contains("boolean"))
        );
    }

    #[test]
    fn condicao_de_while_nao_boolean_produz_erro() {
        let source = "function main(args: {string}): integer\n    while \"oi\" do\n    end\n    return 0\nend";
        let errs = check_source(source).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("condição") && e.message.contains("boolean"))
        );
    }

    #[test]
    fn for_com_variavel_nao_numerica_produz_erro() {
        let source = "function main(args: {string}): integer\n    for x = \"a\", 10 do\n    end\n    return 0\nend";
        let errs = check_source(source).unwrap_err();
        assert!(errs.iter().any(|e| e.message.contains("integer ou float")));
    }

    #[test]
    fn for_com_tipos_nao_identicos_produz_erro() {
        // Decisão 5 da Fase 1: sem coerção int→float no `for`.
        let source = "function main(args: {string}): integer\n    for x = 1, 10.0 do\n    end\n    return 0\nend";
        let errs = check_source(source).unwrap_err();
        assert!(errs.iter().any(|e| e.message.contains("mesmo tipo")));
    }

    #[test]
    fn for_float_ganha_inc_default_float() {
        let stats = typed_body_stats(
            "function main(args: {string}): integer\n\
             \x20   for f = 1.5, 2.5 do\n\
             \x20   end\n\
             \x20   return 0\n\
             end",
        );
        let TypedStat::For { ty, inc, .. } = &stats[0] else {
            panic!("esperava TypedStat::For");
        };
        assert_eq!(*ty, Type::Float);
        assert_eq!(inc.ty, Type::Float);
        assert!(matches!(inc.kind, TypedExpKind::Float(v) if v == 1.0));
    }

    #[test]
    fn variavel_do_for_nao_vaza_do_laco() {
        let source = "function main(args: {string}): integer\n\
             \x20   for i = 1, 10 do\n\
             \x20   end\n\
             \x20   i = 5\n\
             \x20   return 0\n\
             end";
        let errs = check_source(source).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("'i' não foi declarado"))
        );
    }

    #[test]
    fn atribuir_a_variavel_de_controle_do_for_e_permitido() {
        // O original também permite (a variável é uma declaração comum);
        // no template do T15 ela é sempre `mut`, sem rastreio.
        let source = "function main(args: {string}): integer\n\
             \x20   for i = 1, 10 do\n\
             \x20       i = 5\n\
             \x20   end\n\
             \x20   return 0\n\
             end";
        check_source(source).unwrap_or_else(|errs| {
            panic!(
                "esperava sucesso, obteve erros: {}",
                errs.iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        });
    }

    #[test]
    fn atribuicao_sem_declaracao_produz_erro() {
        let source = "function main(args: {string}): integer\n    x = 10\n    return 0\nend";
        let errs = check_source(source).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("'x' não foi declarado"))
        );
    }

    #[test]
    fn atribuir_a_funcao_produz_erro() {
        let source = "function main(args: {string}): integer\n    print = 1\n    return 0\nend";
        let errs = check_source(source).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("não é possível atribuir a uma função"))
        );
    }

    #[test]
    fn atribuir_a_parametro_produz_erro_nesta_fase() {
        // Divergência documentada do original (que permite): parâmetros não
        // têm rastreio de `mut`, e o Rust gerado não compilaria.
        let source = "function f(x: integer): integer\n\
             \x20   x = 1\n\
             \x20   return x\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   return 0\n\
             end";
        let errs = check_source(source).unwrap_err();
        assert!(errs.iter().any(|e| e.message.contains("parâmetro")));
    }

    #[test]
    fn atribuicao_com_tipo_incompativel_produz_erro() {
        let source = "function main(args: {string}): integer\n\
             \x20   local x: integer = 0\n\
             \x20   x = \"oi\"\n\
             \x20   return x\n\
             end";
        let errs = check_source(source).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("atribuição incompatível"))
        );
    }

    #[test]
    fn mutabilidade_marca_somente_locais_reatribuidos() {
        let stats = typed_body_stats(
            "function main(args: {string}): integer\n\
             \x20   local x: integer = 0\n\
             \x20   local y: integer = 1\n\
             \x20   while true do\n\
             \x20       x = 2\n\
             \x20   end\n\
             \x20   return y\n\
             end",
        );
        let TypedStat::Decl { name, mutable, .. } = &stats[0] else {
            panic!("esperava TypedStat::Decl");
        };
        assert_eq!(name, "x");
        assert!(*mutable, "x é reatribuída dentro do while → mutable");

        let TypedStat::Decl { name, mutable, .. } = &stats[1] else {
            panic!("esperava TypedStat::Decl");
        };
        assert_eq!(name, "y");
        assert!(!*mutable, "y nunca é reatribuída → imutável");
    }

    #[test]
    fn shadowing_marca_somente_a_declaracao_interna() {
        let stats = typed_body_stats(
            "function main(args: {string}): integer\n\
             \x20   local x: integer = 1\n\
             \x20   if true then\n\
             \x20       local x: integer = 2\n\
             \x20       x = 3\n\
             \x20   end\n\
             \x20   return x\n\
             end",
        );
        let TypedStat::Decl { mutable, .. } = &stats[0] else {
            panic!("esperava TypedStat::Decl externa");
        };
        assert!(
            !*mutable,
            "a externa nunca é atingida — o `x = 3` resolve para a interna"
        );

        let TypedStat::If { thens, .. } = &stats[1] else {
            panic!("esperava TypedStat::If");
        };
        let TypedStat::Block { stats: inner, .. } = &thens[0].block else {
            panic!("esperava TypedStat::Block no ramo then");
        };
        let TypedStat::Decl { mutable, .. } = &inner[0] else {
            panic!("esperava TypedStat::Decl interna");
        };
        assert!(*mutable, "a interna é a atingida pelo `x = 3`");
    }

    // ---- Fase 1 (T13): operadores binários/unários e coerção ------------

    /// Verifica `local r = <exp_src>` e devolve a expressão tipada.
    fn typed_value_of(exp_src: &str) -> TypedExp {
        let source = format!(
            "function main(args: {{string}}): integer\n    local r = {exp_src}\n    return 0\nend"
        );
        let stats = typed_body_stats(&source);
        let TypedStat::Decl { value, .. } = &stats[0] else {
            panic!("esperava TypedStat::Decl, obteve {:?}", stats[0]);
        };
        value.clone()
    }

    /// Verifica `local r = <exp_src>` esperando falha e devolve os erros.
    fn exp_errors_of(exp_src: &str) -> Vec<CheckError> {
        let source = format!(
            "function main(args: {{string}}): integer\n    local r = {exp_src}\n    return 0\nend"
        );
        check_source(&source).unwrap_err()
    }

    #[test]
    fn aritmetica_int_int_resulta_integer() {
        for exp in ["1 + 2", "5 - 1", "3 * 4", "7 % 3"] {
            let typed = typed_value_of(exp);
            assert_eq!(typed.ty, Type::Integer, "tipo de `{exp}`");
            assert!(
                matches!(typed.kind, TypedExpKind::Binop { .. }),
                "esperava Binop para `{exp}`"
            );
        }
        let typed = typed_value_of("1 + 2");
        assert!(matches!(
            typed.kind,
            TypedExpKind::Binop { op: BinOp::Add, .. }
        ));
    }

    #[test]
    fn aritmetica_com_um_lado_float_coage_para_float() {
        for exp in ["1 + 2.0", "2.0 * 3", "1.5 - 0.5", "7.0 % 3"] {
            assert_eq!(typed_value_of(exp).ty, Type::Float, "tipo de `{exp}`");
        }
    }

    #[test]
    fn divisao_e_potencia_resultam_sempre_float() {
        // `/` e `^` coagem ambos os lados mesmo quando int/int.
        for exp in ["10 / 3", "2 ^ 10", "1.5 / 0.5"] {
            assert_eq!(typed_value_of(exp).ty, Type::Float, "tipo de `{exp}`");
        }
        assert!(matches!(
            typed_value_of("2 ^ 10").kind,
            TypedExpKind::Binop { op: BinOp::Pow, .. }
        ));
    }

    #[test]
    fn aritmetica_com_operando_nao_numerico_produz_erro() {
        let errs = exp_errors_of("1 + \"a\"");
        assert!(errs.iter().any(|e| e.message.contains("numérico")));

        // Os dois lados errados → um erro por lado.
        let errs = exp_errors_of("true + false");
        assert_eq!(
            errs.iter()
                .filter(|e| e.message.contains("numérico"))
                .count(),
            2
        );
    }

    #[test]
    fn igualdade_de_tipos_comparaveis_resulta_boolean() {
        for exp in ["1 == 1.0", "\"a\" ~= \"b\"", "true == false"] {
            assert_eq!(typed_value_of(exp).ty, Type::Boolean, "tipo de `{exp}`");
        }
        assert!(matches!(
            typed_value_of("1 ~= 2").kind,
            TypedExpKind::Binop { op: BinOp::Ne, .. }
        ));
    }

    #[test]
    fn igualdade_entre_tipos_diferentes_produz_erro() {
        let errs = exp_errors_of("1 == \"a\"");
        assert!(errs.iter().any(|e| {
            e.message
                .contains("não é possível comparar integer com string")
        }));
    }

    #[test]
    fn ordem_aceita_numeros_com_coercao_e_strings() {
        for exp in ["1 < 2.0", "2 >= 2", "\"a\" < \"b\""] {
            assert_eq!(typed_value_of(exp).ty, Type::Boolean, "tipo de `{exp}`");
        }
    }

    #[test]
    fn ordem_com_boolean_ou_tipos_misturados_produz_erro() {
        for exp in ["true < false", "\"a\" < 1"] {
            let errs = exp_errors_of(exp);
            assert!(
                errs.iter()
                    .any(|e| e.message.contains("número com número ou string com string")),
                "esperava erro de ordem para `{exp}`"
            );
        }
    }

    #[test]
    fn and_or_boolean_estrito() {
        // Decisão 7: os dois lados boolean, resultado boolean.
        for exp in ["true and false", "true or false", "1 < 2 and 3 < 4"] {
            assert_eq!(typed_value_of(exp).ty, Type::Boolean, "tipo de `{exp}`");
        }

        let errs = exp_errors_of("1 and 2");
        assert_eq!(
            errs.iter()
                .filter(|e| e.message.contains("precisa ser boolean"))
                .count(),
            2,
            "um erro por lado não-boolean"
        );
        let errs = exp_errors_of("true or 1");
        assert!(
            errs.iter()
                .any(|e| e.message.contains("precisa ser boolean"))
        );
    }

    #[test]
    fn menos_unario_preserva_o_tipo_do_operando() {
        assert_eq!(typed_value_of("-1").ty, Type::Integer);
        assert_eq!(typed_value_of("-1.5").ty, Type::Float);
        // `- -1` aninhado segue integer.
        let typed = typed_value_of("- -1");
        assert_eq!(typed.ty, Type::Integer);
        assert!(matches!(
            typed.kind,
            TypedExpKind::Unop { op: UnOp::Neg, .. }
        ));
    }

    #[test]
    fn not_e_boolean_para_boolean() {
        assert_eq!(typed_value_of("not true").ty, Type::Boolean);
        assert_eq!(typed_value_of("not not false").ty, Type::Boolean);
    }

    #[test]
    fn unario_com_tipo_errado_produz_erro() {
        let errs = exp_errors_of("-\"a\"");
        assert!(errs.iter().any(|e| e.message.contains("numérico")));

        let errs = exp_errors_of("not 1");
        assert!(
            errs.iter()
                .any(|e| e.message.contains("precisa ser boolean"))
        );
    }

    #[test]
    fn concat_coage_numeros_para_string() {
        // Decisão 4: `"x: " .. 42` funciona — o número vira string no codegen.
        for exp in ["\"x: \" .. 42", "\"y: \" .. 1.5", "1 .. \"!\""] {
            let typed = typed_value_of(exp);
            assert_eq!(typed.ty, Type::String, "tipo de `{exp}`");
            assert!(matches!(typed.kind, TypedExpKind::Concat(_)));
        }
    }

    #[test]
    fn concat_com_boolean_ou_nil_produz_erro() {
        for exp in ["true .. \"x\"", "\"x\" .. nil"] {
            let errs = exp_errors_of(exp);
            assert!(
                errs.iter().any(|e| e.message.contains("operando de `..`")),
                "esperava erro de concat para `{exp}`"
            );
        }
    }

    #[test]
    fn binop_relacional_serve_de_condicao_de_if_e_while() {
        // Integração T12+T13: o resultado Boolean dos relacionais satisfaz
        // a checagem de condição.
        check_source(
            "function main(args: {string}): integer\n\
             \x20   if 1 < 2 then\n\
             \x20       return 1\n\
             \x20   end\n\
             \x20   while 1 > 2 do\n\
             \x20   end\n\
             \x20   return 0\n\
             end",
        )
        .unwrap_or_else(|errs| {
            panic!(
                "esperava sucesso, obteve erros: {}",
                errs.iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        });
    }

    #[test]
    fn operadores_fora_do_subconjunto_montados_a_mao_produzem_erro() {
        // O parser (T11) nunca produz `//` nem `~` — AST montada à mão para
        // exercitar o braço defensivo `_` da conversão String → BinOp/UnOp.
        // `#` deixou de ser exemplo aqui na T29 (passou a ser suportado);
        // ver a seção de testes da T29 mais abaixo.
        let loc = Loc { line: 1, col: 1 };
        let program: Program = vec![TopLevel::TopLevelFunc {
            loc,
            islocal: false,
            name: "main".to_string(),
            params: vec![Decl {
                loc,
                name: "args".to_string(),
                r#type: Some(ast::Type::TypeArray {
                    loc,
                    subtype: Box::new(ast::Type::TypeString { loc }),
                }),
                option: false,
            }],
            rettypes: vec![ast::Type::TypeInteger { loc }],
            block: Stat::StatBlock {
                loc,
                stats: vec![Stat::StatReturn {
                    loc,
                    exps: vec![
                        Exp::ExpBinop {
                            loc,
                            lhs: Box::new(Exp::ExpInteger { loc, value: 1 }),
                            op: "//".to_string(),
                            rhs: Box::new(Exp::ExpInteger { loc, value: 2 }),
                        },
                        Exp::ExpUnop {
                            loc,
                            op: "~".to_string(),
                            exp: Box::new(Exp::ExpInteger { loc, value: 1 }),
                        },
                    ],
                }],
            },
        }];

        let errs = check(&program).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("operador `//` não é suportado"))
        );
        assert!(
            errs.iter()
                .any(|e| e.message.contains("operador unário `~` não é suportado"))
        );
    }

    // ---- T29: records, arrays, maps, indexação, campos ------------------

    #[test]
    fn record_vazio_e_aceito_pelo_checker() {
        let loc = Loc { line: 1, col: 1 };
        let program: Program = vec![
            TopLevel::TopLevelRecord {
                loc,
                name: "Ponto".to_string(),
                fields: vec![],
            },
            TopLevel::TopLevelFunc {
                loc,
                islocal: false,
                name: "main".to_string(),
                params: vec![Decl {
                    loc,
                    name: "args".to_string(),
                    r#type: Some(ast::Type::TypeArray {
                        loc,
                        subtype: Box::new(ast::Type::TypeString { loc }),
                    }),
                    option: false,
                }],
                rettypes: vec![ast::Type::TypeInteger { loc }],
                block: Stat::StatBlock {
                    loc,
                    stats: vec![Stat::StatReturn {
                        loc,
                        exps: vec![Exp::ExpInteger { loc, value: 0 }],
                    }],
                },
            },
        ];

        let typed = check(&program).unwrap_or_else(|errs| {
            panic!(
                "esperava sucesso, obteve erros: {}",
                errs.iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        });
        assert!(
            typed
                .program
                .iter()
                .any(|t| matches!(t, TypedTopLevel::Record { name, .. } if name == "Ponto"))
        );
    }

    #[test]
    fn array_literal_com_contexto_e_aceito() {
        let stats = typed_body_stats(
            "function main(args: {string}): integer\n\
             \x20   local t: {integer} = {1, 2, 3}\n\
             \x20   return 0\n\
             end",
        );
        let TypedStat::Decl { value, .. } = &stats[0] else {
            panic!("esperava TypedStat::Decl");
        };
        assert_eq!(
            value.ty,
            Type::Array {
                elem: Box::new(Type::Integer)
            }
        );
        assert!(matches!(value.kind, TypedExpKind::ArrayLit(ref v) if v.len() == 3));
    }

    #[test]
    fn array_literal_sem_contexto_infere_do_primeiro_elemento() {
        let typed = typed_value_of("{1, 2, 3}");
        assert_eq!(
            typed.ty,
            Type::Array {
                elem: Box::new(Type::Integer)
            }
        );
    }

    #[test]
    fn array_aninhado_e_aceito() {
        let typed = typed_value_of("{{1, 2}, {3, 4}}");
        assert_eq!(
            typed.ty,
            Type::Array {
                elem: Box::new(Type::Array {
                    elem: Box::new(Type::Integer)
                })
            }
        );
    }

    #[test]
    fn init_list_vazio_sem_contexto_produz_erro() {
        let errs = exp_errors_of("{}");
        assert!(
            errs.iter()
                .any(|e| e.message.contains("inferir o tipo de `{}` vazio"))
        );
    }

    #[test]
    fn map_literal_e_aceito() {
        let typed = typed_value_of(r#"{["a"] = 1, ["b"] = 2}"#);
        assert_eq!(
            typed.ty,
            Type::Map {
                keys: Box::new(Type::String),
                values: Box::new(Type::Integer),
            }
        );
        assert!(matches!(typed.kind, TypedExpKind::MapLit(ref v) if v.len() == 2));
    }

    #[test]
    fn map_com_chave_float_produz_erro() {
        let source = "function main(args: {string}): integer\n\
             \x20   local m: {float: integer} = {}\n\
             \x20   return 0\n\
             end";
        let errs = check_source(source).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("chave de `map`") && e.message.contains("float"))
        );
    }

    #[test]
    fn record_completo_e_aceito_com_contexto() {
        let source = "record Ponto\n\
             \x20   x: integer\n\
             \x20   y: integer\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   local p: Ponto = {x = 1, y = 2}\n\
             \x20   return 0\n\
             end";
        check_source(source).unwrap_or_else(|errs| {
            panic!(
                "esperava sucesso, obteve erros: {}",
                errs.iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        });
    }

    #[test]
    fn record_incompleto_produz_erro() {
        let source = "record Ponto\n\
             \x20   x: integer\n\
             \x20   y: integer\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   local p: Ponto = {x = 1}\n\
             \x20   return 0\n\
             end";
        let errs = check_source(source).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("falta o campo 'y'"))
        );
    }

    #[test]
    fn record_com_campo_extra_produz_erro() {
        let source = "record Ponto\n\
             \x20   x: integer\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   local p: Ponto = {x = 1, z = 2}\n\
             \x20   return 0\n\
             end";
        let errs = check_source(source).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("campo 'z' não existe"))
        );
    }

    #[test]
    fn record_sem_contexto_produz_erro() {
        let source = "record Ponto\n\
             \x20   x: integer\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   local p = {x = 1}\n\
             \x20   return 0\n\
             end";
        let errs = check_source(source).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("inferir o tipo do record"))
        );
    }

    #[test]
    fn record_com_nome_reservado_do_rust_produz_erro() {
        let source = "record String\n\
             \x20   x: integer\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   return 0\n\
             end";
        let errs = check_source(source).unwrap_err();
        assert!(errs.iter().any(|e| e.message.contains("nome reservado")));
    }

    #[test]
    fn record_recursivo_direto_produz_erro() {
        let source = "record No\n\
             \x20   valor: integer\n\
             \x20   prox: No\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   return 0\n\
             end";
        let errs = check_source(source).unwrap_err();
        assert!(errs.iter().any(|e| e.message.contains("recursivo")));
    }

    #[test]
    fn record_recursivo_indireto_produz_erro() {
        let source = "record A\n\
             \x20   b: B\n\
             end\n\
             record B\n\
             \x20   a: A\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   return 0\n\
             end";
        let errs = check_source(source).unwrap_err();
        assert!(errs.iter().any(|e| e.message.contains("recursivo")));
    }

    #[test]
    fn record_com_array_do_proprio_tipo_e_aceito() {
        // Diferente de `prox: No` (campo direto, recursão real — rejeitada
        // acima), `filhos: {No}` é indireção via `Vec<No>`, que tem tamanho
        // finito: não é recursão infinita e precisa ser aceito.
        let source = "record No\n\
             \x20   valor: integer\n\
             \x20   filhos: {No}\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   return 0\n\
             end";
        check_source(source).unwrap_or_else(|errs| {
            panic!(
                "esperava sucesso, obteve erros: {}",
                errs.iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        });
    }

    #[test]
    fn record_contendo_campo_array_e_aceito() {
        let source = "record Lista\n\
             \x20   itens: {integer}\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   local l: Lista = {itens = {1, 2, 3}}\n\
             \x20   return 0\n\
             end";
        check_source(source).unwrap_or_else(|errs| {
            panic!(
                "esperava sucesso, obteve erros: {}",
                errs.iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        });
    }

    #[test]
    fn indexacao_de_array_e_aceita() {
        let typed = typed_value_of("({1, 2, 3})[1]");
        assert_eq!(typed.ty, Type::Integer);
        assert!(matches!(typed.kind, TypedExpKind::Index { .. }));
    }

    #[test]
    fn indice_de_array_nao_integer_produz_erro() {
        let errs = exp_errors_of(r#"({1, 2, 3})["a"]"#);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("índice incompatível"))
        );
    }

    #[test]
    fn acesso_a_campo_de_record_e_aceito() {
        let source = "record Ponto\n\
             \x20   x: integer\n\
             \x20   y: integer\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   local p: Ponto = {x = 1, y = 2}\n\
             \x20   return p.x\n\
             end";
        check_source(source).unwrap_or_else(|errs| {
            panic!(
                "esperava sucesso, obteve erros: {}",
                errs.iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        });
    }

    #[test]
    fn acesso_a_campo_inexistente_produz_erro() {
        let source = "record Ponto\n\
             \x20   x: integer\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   local p: Ponto = {x = 1}\n\
             \x20   return p.z\n\
             end";
        let errs = check_source(source).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("não tem campo 'z'"))
        );
    }

    #[test]
    fn hash_de_array_e_de_string_resulta_integer() {
        // O parser ainda não produz `#` como prefixo de expressão nesta
        // fase (PRD.md, T29) — AST montada à mão, como o restante da suíte
        // "montada à mão" já faz para construções que o parser não emite.
        let loc = Loc { line: 1, col: 1 };
        for operand in [
            Exp::ExpInitList {
                loc,
                fields: vec![
                    ast::Field {
                        loc,
                        name: ast::FieldName::None,
                        exp: Exp::ExpInteger { loc, value: 1 },
                    },
                    ast::Field {
                        loc,
                        name: ast::FieldName::None,
                        exp: Exp::ExpInteger { loc, value: 2 },
                    },
                ],
            },
            Exp::ExpString {
                loc,
                value: "abc".to_string(),
            },
        ] {
            let program: Program = vec![TopLevel::TopLevelFunc {
                loc,
                islocal: false,
                name: "main".to_string(),
                params: vec![Decl {
                    loc,
                    name: "args".to_string(),
                    r#type: Some(ast::Type::TypeArray {
                        loc,
                        subtype: Box::new(ast::Type::TypeString { loc }),
                    }),
                    option: false,
                }],
                rettypes: vec![ast::Type::TypeInteger { loc }],
                block: Stat::StatBlock {
                    loc,
                    stats: vec![Stat::StatReturn {
                        loc,
                        exps: vec![Exp::ExpUnop {
                            loc,
                            op: "#".to_string(),
                            exp: Box::new(operand),
                        }],
                    }],
                },
            }];
            check(&program).unwrap_or_else(|errs| {
                panic!(
                    "esperava sucesso, obteve erros: {}",
                    errs.iter()
                        .map(|e| e.to_string())
                        .collect::<Vec<_>>()
                        .join("; ")
                )
            });
        }
    }

    #[test]
    fn hash_de_map_produz_erro() {
        let loc = Loc { line: 1, col: 1 };
        let program: Program = vec![TopLevel::TopLevelFunc {
            loc,
            islocal: false,
            name: "main".to_string(),
            params: vec![Decl {
                loc,
                name: "args".to_string(),
                r#type: Some(ast::Type::TypeArray {
                    loc,
                    subtype: Box::new(ast::Type::TypeString { loc }),
                }),
                option: false,
            }],
            rettypes: vec![ast::Type::TypeInteger { loc }],
            block: Stat::StatBlock {
                loc,
                stats: vec![
                    Stat::StatDecl {
                        loc,
                        decls: vec![Decl {
                            loc,
                            name: "m".to_string(),
                            r#type: Some(ast::Type::TypeMap {
                                loc,
                                keystype: Box::new(ast::Type::TypeString { loc }),
                                valuestype: Box::new(ast::Type::TypeInteger { loc }),
                            }),
                            option: false,
                        }],
                        exps: vec![Exp::ExpInitList {
                            loc,
                            fields: vec![],
                        }],
                    },
                    Stat::StatReturn {
                        loc,
                        exps: vec![Exp::ExpUnop {
                            loc,
                            op: "#".to_string(),
                            exp: Box::new(Exp::ExpVar {
                                loc,
                                var: Box::new(Var::VarName {
                                    loc,
                                    name: "m".to_string(),
                                }),
                            }),
                        }],
                    },
                ],
            },
        }];
        let errs = check(&program).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("`#` espera um array ou string"))
        );
    }

    #[test]
    fn atribuicao_a_indice_de_array_e_aceita() {
        let source = "record Caixa\n\
             \x20   itens: {integer}\n\
             end\n\
             function altera(c: Caixa): integer\n\
             \x20   c.itens[1] = 9\n\
             \x20   return 0\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   return 0\n\
             end";
        check_source(source).unwrap_or_else(|errs| {
            panic!(
                "esperava sucesso, obteve erros: {}",
                errs.iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        });
    }

    #[test]
    fn atribuicao_a_campo_de_record_e_aceita() {
        let source = "record Ponto\n\
             \x20   x: integer\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   local p: Ponto = {x = 1}\n\
             \x20   p.x = 2\n\
             \x20   return 0\n\
             end";
        check_source(source).unwrap_or_else(|errs| {
            panic!(
                "esperava sucesso, obteve erros: {}",
                errs.iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        });
    }

    #[test]
    fn parametro_composto_aceita_atribuicao_indexada() {
        let source = "function altera(xs: {integer}): integer\n\
             \x20   xs[1] = 9\n\
             \x20   return 0\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   return 0\n\
             end";
        check_source(source).unwrap_or_else(|errs| {
            panic!(
                "esperava sucesso, obteve erros: {}",
                errs.iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        });
    }

    #[test]
    fn atribuicao_ao_parametro_composto_inteiro_produz_erro() {
        let source = "function troca(xs: {integer}): integer\n\
             \x20   xs = {1, 2}\n\
             \x20   return 0\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   return 0\n\
             end";
        let errs = check_source(source).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("parâmetro composto"))
        );
    }

    #[test]
    fn atribuicao_a_parametro_escalar_continua_rejeitada() {
        let source = "function f(x: integer): integer\n\
             \x20   x = 1\n\
             \x20   return x\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   return 0\n\
             end";
        let errs = check_source(source).unwrap_err();
        assert!(errs.iter().any(|e| e.message.contains("parâmetro")));
    }

    #[test]
    fn passar_mesma_variavel_composta_duas_vezes_produz_erro() {
        let source = "function f(xs: {integer}, ys: {integer}): integer\n\
             \x20   return 0\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   local xs: {integer} = {1, 2}\n\
             \x20   return f(xs, xs)\n\
             end";
        let errs = check_source(source).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("empréstimo mutável duplicado"))
        );
    }

    #[test]
    fn passar_array_composto_a_funcao_marca_variavel_como_mutavel() {
        let source = "function main(args: {string}): integer\n\
             \x20   local xs: {integer} = {1, 2}\n\
             \x20   local descartado: integer = usa(xs)\n\
             \x20   return 0\n\
             end\n\
             function usa(xs: {integer}): integer\n\
             \x20   return 0\n\
             end";
        let stats = typed_body_stats(source);
        let TypedStat::Decl { name, mutable, .. } = &stats[0] else {
            panic!("esperava TypedStat::Decl");
        };
        assert_eq!(name, "xs");
        assert!(
            *mutable,
            "xs é passada por valor composto a `usa` → marcada mutável (uso sob &mut)"
        );
    }
}



