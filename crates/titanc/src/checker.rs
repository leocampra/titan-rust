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
//! - Operadores (T13, completados na T61): regras de tipo espelhando
//!   `checker.lua:910-1122` (sem gradual typing), com a coerção int→float
//!   centralizada em `numeric_result`. O checker **não** emite nó de cast:
//!   o codegen decide o `as f64` comparando o tipo do operando com o tipo
//!   do resultado. Bitwise (`& | ~ << >>`) exige `integer` estrito, sem
//!   coerção de float — divergência deliberada do original, ADR 0021; `//`
//!   segue a regra aritmética de `+ - * %`.
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
//! - **Retornos múltiplos** (T65): a assinatura aceita lista de tipos
//!   (`: integer, integer`) e o `return` lista de valores, conferidos por
//!   aridade e por posição. Uma chamada com N>1 retornos usada em posição de
//!   expressão ajusta para o primeiro valor (`TypedExpKind::Adjust`); o
//!   enésimo valor é `TypedExpKind::Extra`.
//!
//! - **Tipos opcionais** (T68): `T?` entra no sistema de tipos, com a
//!   injeção `T → T?` nos pontos em que o destino está escrito
//!   ([`Checker::widen_to_option`]), a recusa de usar um `T?` sem testar
//!   ([`Checker::reject_option`]) e o **estreitamento de fluxo** de
//!   `if x ~= nil then` ([`Checker::check_if_condition`]), que vale só
//!   dentro do ramo. `T?` é invariante em `compatible` (ADR 0008) e
//!   continua sendo: o que a T68 acrescenta é injeção, não variância.
//!
//! A T73 acrescenta `foreign function`: a assinatura de uma função externa é
//! resolvida como a de qualquer função top-level, e o que é próprio dela é a
//! **fronteira** — só escalares e `string` atravessam
//! ([`Checker::check_foreign_boundary_type`]), e o nome vai para
//! `Checker::foreigns`, de onde `resolve_callee` o converte em
//! [`Callee::Foreign`] para o codegen emitir `unsafe extern "C"` (ADR 0025).
//!
//! Tudo fora do subconjunto (métodos de `record`, módulos de usuário)
//! produz um erro semântico claro — nunca panic.

use std::collections::{HashMap, HashSet};

use crate::ast::{self, Args, Decl, Exp, FieldName, Loc, Program, Stat, TopLevel, Var};
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

/// Pilha de escopos léxicos. `blocks` guarda, por bloco aberto, os nomes
/// declarados nele — só para `close_block` saber o que desempilhar de
/// `by_name`. `by_name` é o índice de verdade: cada nome mapeia à pilha dos
/// símbolos declarados com esse nome, do mais externo ao mais interno, então
/// o topo é sempre o que está em escopo agora (shadowing).
struct SymTab {
    blocks: Vec<Vec<String>>,
    by_name: HashMap<String, Vec<Symbol>>,
}

impl SymTab {
    fn new() -> Self {
        SymTab {
            blocks: vec![Vec::new()],
            by_name: HashMap::new(),
        }
    }

    fn open_block(&mut self) {
        self.blocks.push(Vec::new());
    }

    fn close_block(&mut self) {
        let declared = self.blocks.pop().expect("symtab sempre tem pelo menos um bloco");
        for name in declared {
            if let Some(stack) = self.by_name.get_mut(&name) {
                stack.pop();
                if stack.is_empty() {
                    self.by_name.remove(&name);
                }
            }
        }
    }

    fn add_symbol(&mut self, name: &str, ty: Type, kind: SymbolKind, def_loc: Loc) {
        self.blocks
            .last_mut()
            .expect("symtab sempre tem pelo menos um bloco")
            .push(name.to_string());
        self.by_name.entry(name.to_string()).or_default().push(Symbol {
            ty,
            kind,
            def_loc,
        });
    }

    fn find_symbol(&self, name: &str) -> Option<&Symbol> {
        self.by_name.get(name).and_then(|stack| stack.last())
    }

    /// Todos os nomes visíveis agora — o topo da pilha de cada nome em
    /// `by_name` já é, por construção, o símbolo do bloco mais interno que o
    /// declara (shadowing), então isto é uma coleta linear no número de
    /// nomes distintos em escopo, não uma soma sobre todos os blocos
    /// abertos. Usado para o snapshot de escopo do autocomplete (T50):
    /// `find_symbol` resolve *um* nome já sabido, isto aqui lista *todos*
    /// para uma posição do cursor ainda sem nome nenhum.
    fn visible_symbols(&self) -> HashMap<String, Symbol> {
        self.by_name
            .iter()
            .filter_map(|(name, stack)| stack.last().map(|symbol| (name.clone(), symbol.clone())))
            .collect()
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
    /// `foreign function abs(n: integer): integer` (T73) — vira um bloco
    /// `unsafe extern "C"` no Rust gerado. Não tem corpo, e por isso não
    /// carrega `body` nem `islocal`: o símbolo vem do linker.
    ForeignFunc {
        loc: Loc,
        name: String,
        params: Vec<(String, Type)>,
        rettypes: Vec<Type>,
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
    /// `local a, b = ...` (T67) — N>1 declarações de uma vez. Cada alvo é
    /// uma `Decl` completa (nome, tipo, `decl_id`, mutabilidade), então o
    /// fix-up de mutabilidade alcança **todos** eles, e `values` já vem no
    /// formato que o codegen precisa emitir (ver [`TypedMultiValues`]).
    DeclMulti {
        loc: Loc,
        targets: Vec<TypedDeclTarget>,
        values: TypedMultiValues,
    },
    /// `a, b = ...` (T67) — N>1 alvos de uma vez. A semântica do Lua exige
    /// que **todo** o lado direito seja avaliado antes de qualquer escrita
    /// (`a, b = b, a` troca de verdade), o que o codegen resolve com `let`
    /// temporários; ver [`TypedMultiValues`].
    AssignMulti {
        loc: Loc,
        targets: Vec<TypedLValue>,
        values: TypedMultiValues,
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
    /// `repeat` (Fase 5, T64) — o laço que testa no fim. `block` é sempre um
    /// [`TypedStat::Block`], mas o escopo que ele representa engloba também
    /// `condition`: o `until` enxerga os `local` do corpo (semântica do Lua),
    /// e é por isso que `check_repeat` só fecha o escopo depois de tipar a
    /// condição. O codegen (`loop { corpo; if cond { break; } }`) emite os
    /// dois dentro das mesmas chaves, preservando a visibilidade.
    Repeat {
        loc: Loc,
        block: Box<TypedStat>,
        condition: TypedExp,
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
    /// `for`-in (Fase 5, T71) sobre array ou map, já resolvido para uma das
    /// duas formas por [`TypedForInKind`]. O container vem como
    /// [`TypedExp`] e não como nome porque `for x in f() do` é tão válido
    /// quanto `for x in v do`; o que o checker garante é que **nenhuma**
    /// mutação do container acontece dentro do corpo, o que é o que permite
    /// ao codegen emitir um `for` nativo do Rust sobre `.iter()` sem esbarrar
    /// no borrow checker.
    ForIn {
        loc: Loc,
        kind: TypedForInKind,
        /// O container iterado, já tipado (`{T}` ou `{K: V}`).
        container: TypedExp,
        block: Box<TypedStat>,
    },
    Assign {
        loc: Loc,
        target: TypedLValue,
        value: TypedExp,
    },
    /// `break` (Fase 4, T55) — só produzido dentro de `while`/`for`;
    /// `check_stat` rejeita fora de laço antes de chegar aqui.
    Break {
        loc: Loc,
    },
    /// `continue` (Fase 5, T63) — mesma disciplina de `Break`: só produzido
    /// dentro de laço, com a **mesma** checagem de `loop_depth`.
    Continue {
        loc: Loc,
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

/// As duas formas de `for`-in (T71), separadas depois que o checker
/// conheceu o tipo do container — o parser só viu "um ou mais nomes".
///
/// Cada variante já carrega o tipo do que é ligado por volta, porque o
/// codegen precisa dele para decidir se o valor vindo do iterador entra
/// clonado (compostos e `String`, ADR 0006) ou copiado (escalares).
#[derive(Debug, Clone, PartialEq)]
pub enum TypedForInKind {
    /// `for x in v do` sobre `{T}` — `elem_ty` é o `T`.
    Array { name: String, elem_ty: Type },
    /// `for k, v in m do` sobre `{K: V}`.
    Map {
        key_name: String,
        key_ty: Type,
        value_name: String,
        value_ty: Type,
    },
}

/// Um alvo de `local a, b = ...` (T67), já verificado. É o mesmo conteúdo
/// que `TypedStat::Decl` carrega para uma declaração simples — menos o
/// valor, que na forma múltipla é comum a todos os alvos.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedDeclTarget {
    pub loc: Loc,
    pub name: String,
    pub ty: Type,
    /// Ver `TypedStat::Decl::decl_id`.
    pub decl_id: usize,
    /// Ver `TypedStat::Decl::mutable`; preenchido por `fixup_mutability`.
    pub mutable: bool,
}

/// O lado direito de uma declaração/atribuição múltipla (T67), nas duas
/// formas que a fonte permite — a distinção é do checker, não do codegen,
/// porque é ela que decide a aridade.
#[derive(Debug, Clone, PartialEq)]
pub enum TypedMultiValues {
    /// `local a, b = f()` / `a, b = f()`: **uma** chamada cuja assinatura
    /// declara exatamente tantos retornos quantos são os alvos. O codegen
    /// desestrutura a tupla da T66 num único `let`.
    Call(TypedExp),
    /// `a, b = b, a`: uma expressão por alvo, na ordem. O codegen avalia
    /// **todas** em `let` temporários antes de escrever em qualquer alvo —
    /// é o que faz o swap trocar de verdade (armadilha da T67).
    List(Vec<TypedExp>),
}

/// Lado esquerdo de uma atribuição já resolvido (T67): o que
/// `check_assign_target` apurou sobre um alvo antes de o valor ser tipado.
/// Existe para a atribuição múltipla poder validar **todos** os alvos com a
/// mesma máquina que a single-target sempre usou.
struct AssignTarget {
    lvalue: TypedLValue,
    /// Tipo que o alvo aceita — também o contexto passado a `check_exp`.
    ty: Type,
    /// `Some` quando o alvo (ou a raiz da cadeia) é uma local; é este id
    /// que `fixup_mutability` procura para emitir `let mut`.
    decl_id: Option<DeclId>,
    /// `Some` só para o alvo que é um nome simples — a mensagem de tipo
    /// incompatível o cita, e a de `v[i]`/`p.campo` não.
    name: Option<String>,
}

/// Ramo `then` já verificado de um `TypedStat::If`.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedThen {
    pub loc: Loc,
    pub condition: TypedExp,
    pub block: TypedStat,
    /// Nomes que a condição deste ramo estreitou de `T?` para `T` (T68), na
    /// ordem em que aparecem na condição. Dentro de `block` cada um deles
    /// **já** tem o tipo base — é o que faz `if x ~= nil then print(x) end`
    /// tipar sem o usuário escrever desembrulho nenhum.
    ///
    /// O codegen (T69) lê esta lista para emitir `if let Some(x) = x` em vez
    /// de comparar com `None` e desembrulhar dentro do corpo; enquanto ela
    /// estiver vazia — o caso de toda condição que não testa opcional —
    /// nada muda na emissão.
    ///
    /// **Armadilha para a T69:** um `if let Some(x) = x` liga um `x` novo,
    /// então uma atribuição a `x` dentro do ramo escreveria na ligação e
    /// não na variável de fora. O checker não proíbe essa atribuição (ela é
    /// válida e tipa contra o **tipo base**, o que aliás impede
    /// `x = nil` lá dentro), então quem emitir precisa escrever de volta —
    /// `if let Some(x) = x` sobre uma referência, ou o `match` equivalente.
    pub narrowed: Vec<String>,
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
    /// Chamada a uma função declarada por `foreign function` (T73). Só o
    /// codegen diferencia de [`Callee::Direct`] — a tipagem dos argumentos é
    /// a mesma —, e a diferença é toda de emissão: sem mangling (o nome é do
    /// símbolo do linker), envolta em `unsafe`, e com `string` convertida
    /// para `CString` na chamada.
    Foreign(String),
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
    /// Ajuste de uma chamada com N>1 retornos usada em posição escalar
    /// (T65 — o `ExpAdjust` de `ast.rs`): a chamada produz uma tupla e só o
    /// primeiro valor interessa. O `ty` do `TypedExp` que envolve este nó já
    /// é o do primeiro retorno; `exp` é sempre um `Call`.
    Adjust(Box<TypedExp>),
    /// `index`-ésimo (base 0) valor de retorno de uma chamada com N>1
    /// retornos (T65 — o `ExpExtra` de `ast.rs`). `exp` é sempre um `Call`.
    Extra {
        exp: Box<TypedExp>,
        index: usize,
    },
    /// Injeção `T → T?` (T68): um valor do tipo base entregue onde o destino
    /// declara `T?`. O `ty` do `TypedExp` que envolve este nó é o `Option`;
    /// `exp` é o valor original, com o tipo base.
    ///
    /// É um nó explícito, e não um `ty` reescrito em silêncio, porque é
    /// exatamente aqui que o codegen (T69) emite o `Some(...)` — decidir
    /// isso no checker, onde o tipo do destino é conhecido, evita o backend
    /// ter de reinferir contexto. O `nil` entregue a um `T?` **não** passa
    /// por aqui: continua `TypedExpKind::Nil`, só que com `ty` opcional, e
    /// vira `None`.
    SomeOf(Box<TypedExp>),
    /// Cast `as` (T70). O `ty` do `TypedExp` que envolve este nó já é o tipo
    /// **de destino**; `exp` carrega o de origem, que o codegen ainda precisa
    /// para saber o que empacotar ao subir para `value`.
    ///
    /// O cast de identidade não chega aqui: `check_cast` devolve o operando
    /// intacto, então o backend nunca emite conversão à toa.
    Cast {
        kind: CastKind,
        exp: Box<TypedExp>,
    },
}

/// Qual das conversões de [`TypedExpKind::Cast`] o backend deve emitir (T70).
///
/// Enum, e não um par de tipos que o codegen reinspecionaria: a decisão já foi
/// tomada em `check_cast`, com os tipos em mãos, e reduzi-la a quatro casos
/// deixa o `match` da emissão exaustivo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CastKind {
    /// `integer as float` — sempre exato para as magnitudes que um `i64`
    /// representa em `f64` sem perda, e arredondado para o `f64` mais próximo
    /// acima disso, que é o comportamento do `as` do Rust.
    IntToFloat,
    /// `float as integer` — **trunca** em direção a zero (`3.9` → `3`,
    /// `-3.9` → `-3`), não arredonda para baixo como o `//` da T61.
    FloatToInt,
    /// `x as value` — empacota no `titan_runtime::Value` correspondente.
    ToValue,
    /// `v as T` com `v: value` — desempacota, abortando em tempo de execução
    /// se o `value` guardar outro tipo.
    FromValue,
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
    /// `//` — divisão com piso (T61). Não é o `/` do Rust: para inteiros o
    /// Rust trunca em direção a zero e o Titan/Lua arredonda para baixo.
    IDiv,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    And,
    Or,
    /// `&` — bitwise AND (T61).
    BAnd,
    /// `|` — bitwise OR (T61).
    BOr,
    /// `~` **binário** — XOR no Titan (o `~` unário é NOT, e o `^` do Titan
    /// é potência, não XOR). Vira `^` na emissão (T61).
    BXor,
    /// `<<` — deslocamento à esquerda (T61).
    Shl,
    /// `>>` — deslocamento à direita (T61).
    Shr,
}

impl BinOp {
    /// Grafia do operador no fonte Titan — as mesmas strings que o parser
    /// coloca em `ExpBinop.op`. Desde a T61 cobre todos os operadores
    /// binários que o parser produz (bitwise e `//` inclusive); `None`
    /// sobra só para AST montada à mão, que vira erro claro no chamador.
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
            "//" => BinOp::IDiv,
            "&" => BinOp::BAnd,
            "|" => BinOp::BOr,
            // Titan `~` binário é XOR (o `^` é potência) — ver `BinOp::BXor`.
            "~" => BinOp::BXor,
            "<<" => BinOp::Shl,
            ">>" => BinOp::Shr,
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

/// Um símbolo em escopo — o que o autocomplete de posição de expressão
/// oferece (T50): nome, tipo formatado (para o `detail` do item) e, quando
/// é módulo (`import data`, sem membro `.` de valor — completado à parte),
/// o **nome real** do módulo.
///
/// `module` guarda esse nome real em vez de um simples `is_module: bool`
/// porque desde a T72 o nome do símbolo pode ser um alias (`d` em `import
/// data as d`) — e é o nome real que resolve contra
/// `capabilities::lookup_module`. `None` para tudo que não é módulo.
#[derive(Debug, Clone, PartialEq)]
pub struct ScopedSymbol {
    pub name: String,
    pub type_name: String,
    pub module: Option<String>,
}

/// Todos os símbolos visíveis num ponto do programa — snapshot tirado ao
/// fechar cada bloco léxico (T50). Como `ast::Stat` não guarda a `Loc` de
/// fechamento do bloco, o intervalo é aproximado pela `Loc` de abertura
/// (`start`) até a última `Loc` vista dentro dele (`end`); blocos aninhados
/// produzem snapshots aninhados, e o autocomplete escolhe o de intervalo mais
/// estreito que contém o cursor (o mais interno).
#[derive(Debug, Clone, PartialEq)]
pub struct ScopeSnapshot {
    pub start: Loc,
    pub end: Loc,
    pub symbols: Vec<ScopedSymbol>,
}

/// Saída completa de [`check`]: a AST tipada (o que `codegen` consome) mais
/// os índices que o LSP consome — usos (T49) e escopos (T50). Separados de
/// `TypedProgram` para não obrigar `codegen`, que não precisa deles, a lidar
/// com eles.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckedProgram {
    pub program: TypedProgram,
    pub uses: Vec<SymbolUse>,
    pub scopes: Vec<ScopeSnapshot>,
}

/// Operador unário já resolvido (T13; `Len` acrescentado estruturalmente na
/// T25 — `#v`/`#s`, sem produtor ainda: `check_unop` só mapeia `-`/`not` do
/// parser).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
    Len,
    /// `~` **unário** — bitwise NOT sobre inteiro (T61). Vira `!` no Rust,
    /// o mesmo operador de `Not`, mas sobre `i64` em vez de `bool`.
    BNot,
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
    /// **nome local** → entrada da tabela de capabilities
    /// (`capabilities.rs`, T37), consultada por `resolve_type`
    /// (`data.DataFrame`) e por `check_call`/`check_var` (T39/T40) para
    /// membros do módulo.
    ///
    /// A chave é o nome *local* porque é ele que o programa escreve à
    /// esquerda do `.` — com `import data as d` (T72) a chave é `d`. O nome
    /// real do módulo, que o codegen precisa para achar o caminho Rust, sai
    /// de `Capability::titan_name`, nunca desta chave.
    modules: HashMap<String, &'static crate::capabilities::Capability>,
    /// Índice colateral de usos resolvidos, para hover e go-to-definition
    /// (T49) — ver [`SymbolUse`].
    uses: Vec<SymbolUse>,
    /// Índice colateral de escopos, para autocomplete em posição de
    /// expressão (T50) — ver [`ScopeSnapshot`].
    scopes: Vec<ScopeSnapshot>,
    /// Última `Loc` vista durante a passada 2 — aproxima o fim de um bloco
    /// (que `ast::Stat` não guarda) para fechar o intervalo de um
    /// [`ScopeSnapshot`] quando o bloco fecha.
    last_loc: Loc,
    /// Local de declaração do *nome* de cada record (a chave do `record ...
    /// end`), separado de `records` porque `Type::Record` não carrega `Loc`.
    record_def_locs: HashMap<String, Loc>,
    /// Local de declaração de cada campo de record — `(nome do record, nome
    /// do campo) -> Loc`, pela mesma razão de `record_def_locs`.
    field_def_locs: HashMap<(String, String), Loc>,
    /// Profundidade de `while`/`for` aninhados (Fase 4, T55) — `break` só é
    /// válido quando `> 0`, e `continue` (T63) usa exatamente a mesma
    /// checagem. Incrementada/decrementada em `check_stat` ao entrar/sair do
    /// bloco do laço.
    loop_depth: usize,
    /// Nomes declarados por `foreign function` (T73), no molde de `modules`.
    ///
    /// Um símbolo externo é, para escopo e atribuição, idêntico a uma função
    /// top-level — daí continuar sendo [`SymbolKind::Global`] em vez de
    /// ganhar uma variante que forçaria um braço a mais em todo `match` de
    /// `SymbolKind` sem dizer nada de novo. O que **é** diferente é só a
    /// emissão (`unsafe`, `extern "C"`, `CString`), e é isso que este
    /// conjunto carrega até `resolve_callee`, que o converte em
    /// [`Callee::Foreign`].
    foreigns: HashSet<String>,
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
            foreigns: HashSet::new(),
            uses: Vec::new(),
            scopes: Vec::new(),
            last_loc: Loc { line: 0, col: 0 },
            record_def_locs: HashMap::new(),
            field_def_locs: HashMap::new(),
            loop_depth: 0,
        }
    }

    /// Atualiza a última `Loc` vista (T50) — chamado de todo ponto que já
    /// tinha uma `Loc` de statement/expressão em mãos, para o fim de um
    /// [`ScopeSnapshot`] acompanhar o quão longe no arquivo o bloco chegou.
    fn touch_loc(&mut self, loc: Loc) {
        if (loc.line, loc.col) > (self.last_loc.line, self.last_loc.col) {
            self.last_loc = loc;
        }
    }

    /// Fecha o bloco atual da symtab e grava o snapshot dos símbolos que
    /// estavam visíveis dentro dele (T50) — chamado em todo `close_block`
    /// exceto o do escopo global (sem `open_block` correspondente).
    fn close_scope(&mut self, start: Loc) {
        let symbols = self
            .st
            .visible_symbols()
            .into_iter()
            .map(|(name, symbol)| ScopedSymbol {
                module: match &symbol.kind {
                    SymbolKind::Module { name } => Some(name.clone()),
                    _ => None,
                },
                type_name: type_name(&symbol.ty),
                name,
            })
            .collect();
        let end = self.last_loc;
        self.st.close_block();
        self.scopes.push(ScopeSnapshot { start, end, symbols });
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
            // `enum` (T74): a AST e o `Type::Sum` já existem, mas a coleta
            // (`self.enums`), a desambiguação de construção de variante e a
            // exaustividade do `match` são da T76. Até lá nada chega aqui: o
            // parser ainda não produz este nó (T75). Braço explícito, e não
            // um `_`, para que a próxima variante de `TopLevel` volte a dar
            // erro de compilação em vez de passar em silêncio.
            TopLevel::TopLevelEnum { .. } => {}
            // `import data` e `import data as d` (T72). O nome que colide,
            // que vira símbolo e que chaveia `self.modules` é sempre o
            // **local** (`localname`); `modname` só serve para achar a
            // capability. Sem alias os dois são iguais, e o comportamento
            // da T38 fica idêntico.
            TopLevel::TopLevelImport {
                loc,
                localname,
                modname,
            } => {
                if self.st.find_symbol(localname).is_some()
                    || self.modules.contains_key(localname)
                {
                    self.error(*loc, format!("'{localname}' já foi declarado antes."));
                    return;
                }
                match crate::capabilities::lookup_module(modname) {
                    Some(capability) => {
                        self.modules.insert(localname.clone(), capability);
                        self.st.add_symbol(
                            localname,
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
            // `foreign function abs(n: integer): integer` (T73). A
            // assinatura é resolvida como a de qualquer função top-level —
            // a diferença toda está na **fronteira**: só escalares e
            // `string` atravessam (ADR 0025), e cada violação sai com erro
            // em português aqui, antes de o rustc ver o `extern "C"`.
            TopLevel::TopLevelForeignFunc {
                loc,
                name,
                params,
                rettypes,
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

                // Erros de fronteira são acumulados, não abortam no
                // primeiro: uma assinatura com dois tipos inválidos deve
                // apontar os dois, como o resto do checker faz.
                let mut ok = true;
                for (param, ty) in params.iter().zip(param_types.iter()) {
                    if !self.check_foreign_boundary_type(
                        param.loc,
                        ty,
                        &format!("o parâmetro '{}' de `foreign function {name}`", param.name),
                    ) {
                        ok = false;
                    }
                }
                // Retorno `nil` é a função externa sem valor de retorno
                // (`void` em C) — vale, e só nessa posição.
                if ret_types.len() > 1 {
                    self.error(
                        *loc,
                        format!(
                            "`foreign function {name}` não pode ter mais de um retorno: \
                             a fronteira C devolve um valor só."
                        ),
                    );
                    ok = false;
                } else if let Some(ret) = ret_types.first()
                    && *ret != Type::Nil
                    && !self.check_foreign_boundary_type(
                        *loc,
                        ret,
                        &format!("o retorno de `foreign function {name}`"),
                    )
                {
                    ok = false;
                }
                if !ok {
                    return;
                }

                let fn_ty = Type::Function {
                    params: param_types,
                    rettypes: ret_types,
                };
                self.st
                    .add_symbol(name, fn_ty.clone(), SymbolKind::Global, *loc);
                self.foreigns.insert(name.clone());
                self.record_use(*loc, *loc, name, &fn_ty);
            }
            TopLevel::TopLevelMethod { loc, .. } | TopLevel::TopLevelStatic { loc, .. } => {
                self.error(*loc, "métodos não são suportados nesta fase.");
            }
        }
    }

    /// Tipos que atravessam a fronteira de FFI (T73, ADR 0025): escalares
    /// (`integer`, `float`, `boolean`) e `string`. Devolve `true` quando o
    /// tipo passa; senão registra o erro e devolve `false`.
    ///
    /// A lista é curta de propósito. `{T}`, `{K:V}` e `record` têm layout
    /// escolhido pelo Rust (`Vec`, `HashMap`, `struct` sem `#[repr(C)]`),
    /// que nenhuma função C sabe ler; `value` é um enum boxado do runtime;
    /// `T?` é `Option<T>`, cujo layout só é garantido em casos que não vale
    /// a pena enumerar aqui; `nil` só faz sentido como "sem retorno". Passar
    /// qualquer um deles compilaria e leria memória errada — por isso a
    /// recusa é do checker, com mensagem em português, e não do rustc.
    fn check_foreign_boundary_type(&mut self, loc: Loc, ty: &Type, onde: &str) -> bool {
        if matches!(
            ty,
            Type::Integer | Type::Float | Type::Boolean | Type::String
        ) {
            return true;
        }
        self.error(
            loc,
            format!(
                "{onde} é {}, que não atravessa a fronteira de FFI; \
                 só integer, float, boolean e string atravessam.",
                type_name(ty)
            ),
        );
        false
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
            // `value`, o topo do gradual typing (T70). A T25 o rejeitava
            // aqui porque o codegen não sabia emiti-lo; desde a T70 ele tem
            // representação de verdade (`titan_runtime::Value`, um enum
            // boxado), então `rust_type_name` o emite como qualquer outro
            // tipo e a rejeição deixa de existir.
            ast::Type::TypeValue { .. } => Some(Type::Value),
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
            // `T?` (T68) — o tipo que o ADR 0008 adiou desde a Fase 2.
            //
            // Duas bases não fazem sentido e saem aqui, cada uma com sua
            // mensagem: `nil?` (o "ausente" já é o próprio `nil`) e `value?`
            // (`value` do gradual typing já aceita `nil`, então o `?` não
            // acrescentaria estado nenhum). `T??` nem chega — o parser
            // recusa o segundo `?`.
            ast::Type::TypeOption { loc, basetype } => {
                let base = self.resolve_type(basetype)?;
                match base {
                    Type::Nil => {
                        self.error(
                            *loc,
                            "`nil?` não faz sentido: `nil` já é a ausência de valor.",
                        );
                        None
                    }
                    Type::Value => {
                        self.error(
                            *loc,
                            "`value?` não faz sentido: `value` já aceita `nil`.",
                        );
                        None
                    }
                    base => Some(Type::Option {
                        base: Box::new(base),
                    }),
                }
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
            // `module` aqui é o nome **local** escrito no programa (`d` em
            // `import data as d`, T72); `Type::Opaque::module` guarda o nome
            // real do módulo, que é o que o codegen resolve contra
            // `capabilities::lookup_module`.
            ast::Type::TypeQualName { loc, module, name } => {
                let Some(capability) = self.modules.get(module) else {
                    self.error(*loc, format!("módulo '{module}' não foi importado."));
                    return None;
                };
                match capability.find_opaque(name) {
                    Some(opaque) => Some(Type::Opaque {
                        module: capability.titan_name.to_string(),
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

                self.close_scope(*loc);

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
            // `foreign function` (T73): a passada 1 já resolveu e validou
            // a assinatura inteira (inclusive a fronteira de FFI). Não há
            // corpo para checar, então aqui só reaproveitamos o símbolo —
            // se ele não está em `self.foreigns`, já foi rejeitado lá.
            TopLevel::TopLevelForeignFunc {
                loc, name, params, ..
            } => {
                if !self.foreigns.contains(name) {
                    return None;
                }
                let Some(Symbol {
                    ty:
                        Type::Function {
                            params: param_types,
                            rettypes: ret_types,
                        },
                    ..
                }) = self.st.find_symbol(name).cloned()
                else {
                    return None;
                };
                let named_params = params
                    .iter()
                    .zip(param_types)
                    .map(|(p, t)| (p.name.clone(), t))
                    .collect();
                Some(TypedTopLevel::ForeignFunc {
                    loc: *loc,
                    name: name.clone(),
                    params: named_params,
                    rettypes: ret_types,
                })
            }
            // Já reportado como erro na passada 1.
            _ => None,
        }
    }

    fn check_stat(&mut self, stat: &Stat, rettypes: &[Type]) -> Option<TypedStat> {
        self.touch_loc(stat_loc(stat));
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
                self.close_scope(*loc);
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
                // `local a, b = ...` (T67) tem caminho próprio: a aridade
                // entre alvos e valores é sua, e a declaração simples —
                // o caso comum de todo programa — segue exatamente como era.
                if decls.len() != 1 || exps.len() != 1 {
                    return self.check_decl_multi(*loc, decls, exps);
                }
                let decl = &decls[0];
                // Resolvido antes de tipar o valor (T29): `{...}` precisa do
                // tipo anotado como contexto para se desambiguar.
                let declared = match &decl.r#type {
                    Some(annotated) => Some(self.resolve_type(annotated)?),
                    None => None,
                };
                let value = self.check_exp(&exps[0], declared.as_ref())?;

                let (ty, value) = match declared {
                    Some(declared) => {
                        // `T → T?` e `nil → T?` (T68) antes da conferência:
                        // é a única injeção que `compatible` não faz, e
                        // fazê-la aqui é o que permite
                        // `local x: integer? = 10` e `... = nil`.
                        let value = Self::widen_to_option(&declared, value);
                        if !declared.compatible(&value.ty) {
                            // `T?` num destino `T` (T68): a mensagem que
                            // ensina o teste, não a genérica de tipos.
                            if self.reject_option_where_base_expected(&declared, &value) {
                                return None;
                            }
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
                        (declared, value)
                    }
                    // `local x? = exp` (T68, `Decl.option`): o tipo é o do
                    // valor **envolvido** no opcional, e não o do valor —
                    // é a forma de declarar `T?` sem repetir o `T`. Um
                    // `local x? = nil` não tem base para inferir: erro
                    // claro, com a saída escrita (anotar o tipo).
                    None if decl.option => {
                        if matches!(value.ty, Type::Nil) {
                            self.error(
                                decl.loc,
                                format!(
                                    "não dá para inferir o tipo de '{}?' a partir de `nil`: escreva o tipo (`local {}: T? = nil`).",
                                    decl.name, decl.name
                                ),
                            );
                            return None;
                        }
                        let optional = Type::Option {
                            base: Box::new(value.ty.clone()),
                        };
                        let value = Self::widen_to_option(&optional, value);
                        (optional, value)
                    }
                    // Sem anotação e sem `?`: o tipo é o do valor, e um
                    // valor opcional **não** é inferido como opcional por
                    // acidente — `local y = x` com `x: integer?` seria
                    // propagar a ausência sem o usuário ter pedido. O
                    // original faz o mesmo, forçando o valor
                    // (`tryforce`/"never infer option type",
                    // `checker.lua:322`); aqui, sem cast implícito, a
                    // forma honesta é recusar e apontar as duas saídas.
                    None => {
                        if matches!(value.ty, Type::Option { .. }) {
                            self.error(
                                decl.loc,
                                format!(
                                    "tipo opcional não é inferido: escreva `local {}? = ...` para declarar {} ou teste com `if ... ~= nil then` antes.",
                                    decl.name,
                                    type_name(&value.ty)
                                ),
                            );
                            return None;
                        }
                        (value.ty.clone(), value)
                    }
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
                // Chamada como statement descarta todos os retornos — não
                // passa por `adjust_to_one` (T65), que é para posição de
                // expressão.
                let call = match callexp {
                    Exp::ExpCall { loc, exp, args } => self.check_call(loc, exp, args)?.0,
                    other => self.check_exp(other, None)?,
                };
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

                // `return nil` / `return 10` numa função `: integer?`
                // (T68) — mesma injeção, com o destino escrito na
                // assinatura.
                let typed_exps: Vec<TypedExp> = typed_exps
                    .into_iter()
                    .zip(rettypes)
                    .map(|(exp, expected)| Self::widen_to_option(expected, exp))
                    .collect();

                for (found, expected) in typed_exps.iter().zip(rettypes) {
                    if !expected.compatible(&found.ty) {
                        if self.reject_option_where_base_expected(expected, found) {
                            return None;
                        }
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
                    // Estreitamento de fluxo (T68): a condição pode tornar
                    // um `T?` um `T` **dentro** do seu ramo. O bloco aberto
                    // aqui é o que hospeda os símbolos estreitados, e é
                    // fechá-lo logo depois do corpo que garante que o
                    // estreitamento não vaza — nem para o `elseif`/`else`
                    // seguintes, nem para depois do `if`.
                    self.st.open_block();
                    let (condition, narrowed) = self.check_if_condition(&then.condition);
                    let block = self.check_stat(&then.block, rettypes);
                    self.st.close_block();
                    match (condition, block) {
                        (Some(condition), Some(block)) => typed_thens.push(TypedThen {
                            loc: then.loc,
                            condition,
                            block,
                            narrowed,
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
                self.loop_depth += 1;
                let block = self.check_stat(block, rettypes);
                self.loop_depth -= 1;
                Some(TypedStat::While {
                    loc: *loc,
                    condition: condition?,
                    block: Box::new(block?),
                })
            }
            Stat::StatRepeat {
                loc,
                block,
                condition,
            } => self.check_repeat(*loc, block, condition, rettypes),
            Stat::StatFor {
                loc,
                decl,
                start,
                finish,
                inc,
                block,
            } => self.check_for(*loc, decl, start, finish, inc.as_deref(), block, rettypes),
            Stat::StatForIn {
                loc,
                decls,
                exp,
                block,
            } => self.check_for_in(*loc, decls, exp, block, rettypes),
            Stat::StatAssign { loc, vars, exps } => {
                // `a, b = ...` (T67), pelo mesmo motivo de `StatDecl`.
                if vars.len() != 1 || exps.len() != 1 {
                    return self.check_assign_multi(*loc, vars, exps);
                }
                self.check_assign(*loc, &vars[0], &exps[0])
            }
            Stat::StatBreak { loc } => {
                if self.loop_depth == 0 {
                    self.error(*loc, "`break` fora de um laço (`while`/`for`).");
                    return None;
                }
                Some(TypedStat::Break { loc: *loc })
            }
            // `continue` (T63): a **mesma** checagem de `break`, palavra por
            // palavra — `loop_depth` já é incrementado por `while` e por
            // `check_for`, então não há nada a acrescentar ao rastreamento.
            Stat::StatContinue { loc } => {
                if self.loop_depth == 0 {
                    self.error(*loc, "`continue` fora de um laço (`while`/`for`).");
                    return None;
                }
                Some(TypedStat::Continue { loc: *loc })
            }
        }
    }

    /// Condição de `if`/`elseif`/`while`: precisa ser `Boolean` (ou `Value`,
    /// via `compatible` — gradual typing), como o `checkexp(cond, ...,
    /// types.Boolean())` do original.
    fn check_condition(&mut self, exp: &Exp, contexto: &str) -> Option<TypedExp> {
        let typed = self.check_exp(exp, Some(&Type::Boolean))?;
        // `if x then` com `x: boolean?` (T68): Titan não tem truthy/falsy
        // (decisão 7 da Fase 1), então a condição precisa do valor presente
        // — e a mensagem que ensina o teste vale mais que "precisa ser
        // boolean, encontrado boolean?".
        if self.reject_option(&typed) {
            return None;
        }
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

    /// A condição de um ramo `if`/`elseif`, com o estreitamento de fluxo da
    /// T68 aplicado à medida que ela é lida.
    ///
    /// Só uma forma estreita, e é a que o Titan precisa ter: `x ~= nil`
    /// (nas duas ordens), com `x` sendo um nome de tipo `T?`. O símbolo
    /// estreitado é acrescentado ao bloco que o chamador **já abriu**, o
    /// que faz o estreitamento valer do ponto da condição em diante e
    /// morrer quando esse bloco fecha.
    ///
    /// Ler a condição em ordem importa por causa do `and`: em
    /// `x ~= nil and x > 0`, o lado direito precisa enxergar o `x` já
    /// estreitado, senão o próprio idioma canônico do teste não tiparia.
    /// Por isso este método desce pelo `and` em vez de tipar a condição
    /// inteira de uma vez — e desce **só** pelo `and`: num `or`, nenhum dos
    /// lados sabe o que o outro testou, e num `not` o estreitamento se
    /// inverteria.
    ///
    /// Divergência deliberada do Titan original, que não estreita nada: lá
    /// (`checker.lua:183`, `tryforce`) um `T?` usado como `T` ganha um cast
    /// implícito, sem que o fluxo tenha provado coisa alguma. Um cast que o
    /// usuário não escreveu e que pode falhar é exatamente o que a
    /// convenção deste projeto — erro claro em português, nunca panic —
    /// existe para evitar.
    ///
    /// `while`/`repeat` ficam de fora de propósito, e não por falta de
    /// oportunidade: o corpo do laço pode atribuir `nil` ao nome e a volta
    /// seguinte entraria com ele ausente. Provar que não atribui é análise
    /// de fluxo de verdade, que este checker não faz — e um estreitamento
    /// que às vezes mente é pior que nenhum. No `if` a pergunta não se
    /// coloca: o ramo executa uma vez, e uma atribuição lá dentro tipa
    /// contra o tipo **base** (o que, de quebra, recusa `x = nil` dentro do
    /// ramo que acabou de provar que `x` não é nil).
    ///
    /// Devolve a condição tipada (`None` quando ela não tipa) e os nomes
    /// estreitados, que o `TypedThen` carrega para o codegen (T69).
    fn check_if_condition(&mut self, exp: &Exp) -> (Option<TypedExp>, Vec<String>) {
        if let Exp::ExpBinop { loc, lhs, op, rhs } = exp
            && op == "and"
        {
            let (typed_lhs, mut narrowed) = self.check_if_condition(lhs);
            let (typed_rhs, narrowed_rhs) = self.check_if_condition(rhs);
            narrowed.extend(narrowed_rhs);
            let (Some(typed_lhs), Some(typed_rhs)) = (typed_lhs, typed_rhs) else {
                return (None, narrowed);
            };
            // Os dois lados de um `and` são boolean estrito (decisão 7 da
            // Fase 1) — a mesma exigência que `check_binop` faz, repetida
            // aqui porque a condição não passa mais por ele.
            let mut ok = true;
            for side in [&typed_lhs, &typed_rhs] {
                if !side.ty.equals(&Type::Boolean) {
                    self.error(
                        side.loc,
                        format!(
                            "operando de `and` precisa ser boolean, encontrado {}.",
                            type_name(&side.ty)
                        ),
                    );
                    ok = false;
                }
            }
            if !ok {
                return (None, narrowed);
            }
            return (
                Some(TypedExp {
                    loc: *loc,
                    ty: Type::Boolean,
                    kind: TypedExpKind::Binop {
                        op: BinOp::And,
                        lhs: Box::new(typed_lhs),
                        rhs: Box::new(typed_rhs),
                    },
                }),
                narrowed,
            );
        }

        let nome_testado = presence_test_name(exp);
        let condition = self.check_condition(exp, "if");
        let mut narrowed = Vec::new();
        if condition.is_some()
            && let Some(nome) = nome_testado
            && let Some(symbol) = self.st.find_symbol(&nome).cloned()
            && let Type::Option { base } = &symbol.ty
        {
            // O símbolo estreitado guarda o **mesmo** `kind` e `def_loc` do
            // original: atribuir ao nome dentro do ramo continua alcançando
            // a mesma declaração (e o mesmo `let mut` no fix-up), e
            // go-to-definition continua saltando para onde ele foi
            // declarado. Só o tipo muda.
            self.st.add_symbol(
                &nome,
                base.as_ref().clone(),
                symbol.kind.clone(),
                symbol.def_loc,
            );
            narrowed.push(nome);
        }
        (condition, narrowed)
    }

    /// `repeat block until cond` (T64).
    ///
    /// A armadilha da tarefa é de **escopo**, não de tipos: em Lua a
    /// condição do `until` enxerga os `local` declarados no corpo
    /// (`repeat local x = f() until x > 10` é válido), o que inverte a ordem
    /// natural de abrir/fechar bloco. Por isso este método **não** delega o
    /// corpo a `check_stat` — que abriria e fecharia o escopo antes de a
    /// condição ser vista — e sim repete aqui o que o braço `StatBlock` faz,
    /// intercalando a condição entre o último statement e o `close_scope`.
    ///
    /// Fora isso é um laço como os outros: `loop_depth` sobe pelo corpo, e
    /// `break`/`continue` (T63) entram sem caso especial (ADR 0023).
    fn check_repeat(
        &mut self,
        loc: Loc,
        block: &Stat,
        condition: &Exp,
        rettypes: &[Type],
    ) -> Option<TypedStat> {
        let Stat::StatBlock {
            loc: block_loc,
            stats,
        } = block
        else {
            // Defensivo: `parse_stat_repeat` só produz `StatBlock` como corpo.
            self.error(loc, "corpo de `repeat` precisa ser um bloco.");
            return None;
        };

        self.touch_loc(*block_loc);
        self.st.open_block();
        let mut typed_stats = Vec::with_capacity(stats.len());
        let mut ok = true;
        self.loop_depth += 1;
        for stat in stats {
            match self.check_stat(stat, rettypes) {
                Some(typed) => typed_stats.push(typed),
                None => ok = false,
            }
        }
        self.loop_depth -= 1;
        // Aqui está o ponto da tarefa: a condição é tipada com o escopo do
        // corpo ainda aberto.
        let typed_condition = self.check_condition(condition, "until");
        self.close_scope(*block_loc);

        if !ok {
            return None;
        }
        Some(TypedStat::Repeat {
            loc,
            block: Box::new(TypedStat::Block {
                loc: *block_loc,
                stats: typed_stats,
            }),
            condition: typed_condition?,
        })
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

        // `for i? = 1, 10` (T68): a variável de controle recebe um valor a
        // cada volta, nunca a ausência de um — o `?` do lado do nome, que o
        // `local` usa para inferir `T?`, não tem leitura aqui.
        if decl.option {
            self.error(
                decl.loc,
                "a variável de controle do `for` não pode ser opcional (`?`).",
            );
            return None;
        }

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
        self.loop_depth += 1;
        let typed_block = self.check_stat(block, rettypes);
        self.loop_depth -= 1;
        self.close_scope(decl.loc);

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

    /// `for`-in (T71) sobre `{T}` e `{K: V}`.
    ///
    /// A ordem aqui importa: o container é tipado **antes** de qualquer nome
    /// entrar em escopo, porque `for v in v do` deve ver o `v` de fora, não a
    /// variável que o próprio laço está declarando.
    ///
    /// Três coisas são decididas neste ponto e em nenhum outro:
    ///
    /// 1. **Qual forma é** — `{T}` liga um nome, `{K: V}` liga dois. Um
    ///    número errado de nomes é erro aqui e não no parser, que não conhece
    ///    o tipo (ver [`Parser::parse_stat_for_in`]).
    /// 2. **Que tipos as variáveis têm** — inferidos do container, ou
    ///    conferidos contra a anotação quando o programa escreveu uma.
    /// 3. **Que o corpo não muta o container** — a checagem que o PRD pede em
    ///    português e que, de quebra, é o que torna seguro o `for` nativo do
    ///    Rust sobre `.iter()` que o codegen emite. Sem ela o `rustc`
    ///    recusaria o programa em inglês, quebrando a convenção do projeto.
    fn check_for_in(
        &mut self,
        loc: Loc,
        decls: &[ast::Decl],
        exp: &Exp,
        block: &Stat,
        rettypes: &[Type],
    ) -> Option<TypedStat> {
        let container = self.check_exp(exp, None)?;

        // `{T}?` / `{K: V}?` (T68): iterar exige o container presente, e a
        // mensagem que ensina o teste vale mais que "esperava array ou map".
        if self.reject_option(&container) {
            return None;
        }

        // Cada `Decl` do `for`-in é ligada a um valor por volta, nunca à
        // ausência de um — mesmo motivo do `for` numérico.
        for decl in decls {
            if decl.option {
                self.error(
                    decl.loc,
                    "a variável do `for`-in não pode ser opcional (`?`).",
                );
                return None;
            }
        }

        // `for k, k in m do`: as duas variáveis são ligadas pelo **mesmo**
        // padrão do `for` do Rust, e repetir um nome ali é
        // `identifier bound more than once` — erro do `rustc`, em inglês.
        // Recusado aqui, e não só porque o Rust recusaria: sombrear a chave
        // com o valor na mesma linha não tem leitura útil nenhuma.
        if let [primeira, segunda] = decls
            && primeira.name == segunda.name
        {
            self.error(
                segunda.loc,
                format!(
                    "'{}' aparece duas vezes nas variáveis do `for`-in; chave e \
valor precisam de nomes diferentes.",
                    segunda.name
                ),
            );
            return None;
        }

        let kind = match &container.ty {
            Type::Array { elem } => {
                if decls.len() != 1 {
                    self.error(
                        loc,
                        format!(
                            "iterar sobre {} liga um nome (o elemento); \
                             encontrei {}. Escreva `for x in ... do`.",
                            type_name(&container.ty),
                            decls.len()
                        ),
                    );
                    return None;
                }
                let elem_ty = (**elem).clone();
                let ty = self.for_in_var_type(&decls[0], &elem_ty, "o elemento")?;
                TypedForInKind::Array {
                    name: decls[0].name.clone(),
                    elem_ty: ty,
                }
            }
            Type::Map { keys, values } => {
                if decls.len() != 2 {
                    self.error(
                        loc,
                        format!(
                            "iterar sobre {} liga dois nomes (chave e valor); \
                             encontrei {}. Escreva `for k, v in ... do`.",
                            type_name(&container.ty),
                            decls.len()
                        ),
                    );
                    return None;
                }
                let key_ty_esperado = (**keys).clone();
                let value_ty_esperado = (**values).clone();
                let key_ty = self.for_in_var_type(&decls[0], &key_ty_esperado, "a chave")?;
                let value_ty = self.for_in_var_type(&decls[1], &value_ty_esperado, "o valor")?;
                TypedForInKind::Map {
                    key_name: decls[0].name.clone(),
                    key_ty,
                    value_name: decls[1].name.clone(),
                    value_ty,
                }
            }
            outro => {
                self.error(
                    container.loc,
                    format!(
                        "o `for`-in itera sobre array (`{{T}}`) ou map (`{{K: V}}`), \
                         encontrado {}.",
                        type_name(outro)
                    ),
                );
                return None;
            }
        };

        // Mutar o container durante a iteração: erro claro do checker, em
        // português, e não `cannot borrow as mutable` do `rustc` (PRD T71).
        // Só é detectável quando o container é uma variável — `for x in f()
        // do` itera um temporário que o corpo não tem como alcançar.
        if let Some(nome) = root_exp_var_name(exp) {
            self.reject_mutacao_durante_iteracao(block, &nome);
        }

        // As variáveis do laço vivem num bloco próprio que não vaza para fora
        // (o corpo `StatBlock` abre o seu por cima), como no `for` numérico.
        self.st.open_block();
        match &kind {
            TypedForInKind::Array { name, elem_ty } => {
                self.st
                    .add_symbol(name, elem_ty.clone(), SymbolKind::ForVar, decls[0].loc);
                self.record_use(decls[0].loc, decls[0].loc, name, elem_ty);
            }
            TypedForInKind::Map {
                key_name,
                key_ty,
                value_name,
                value_ty,
            } => {
                self.st
                    .add_symbol(key_name, key_ty.clone(), SymbolKind::ForVar, decls[0].loc);
                self.record_use(decls[0].loc, decls[0].loc, key_name, key_ty);
                self.st.add_symbol(
                    value_name,
                    value_ty.clone(),
                    SymbolKind::ForVar,
                    decls[1].loc,
                );
                self.record_use(decls[1].loc, decls[1].loc, value_name, value_ty);
            }
        }
        self.loop_depth += 1;
        let typed_block = self.check_stat(block, rettypes);
        self.loop_depth -= 1;
        self.close_scope(loc);

        Some(TypedStat::ForIn {
            loc,
            kind,
            container,
            block: Box::new(typed_block?),
        })
    }

    /// Tipo de uma variável de `for`-in: o que o container oferece, ou a
    /// anotação do programa **se** ela disser a mesma coisa.
    ///
    /// Não há coerção aqui — nem a de `integer`→`float` que a atribuição
    /// permite. `for x: float in {1, 2} do` é um engano sobre o que o array
    /// contém, e dizer isso é mais útil que converter em silêncio.
    fn for_in_var_type(&mut self, decl: &ast::Decl, oferecido: &Type, papel: &str) -> Option<Type> {
        let Some(anotado) = &decl.r#type else {
            return Some(oferecido.clone());
        };
        let anotado = self.resolve_type(anotado)?;
        if !anotado.equals(oferecido) {
            self.error(
                decl.loc,
                format!(
                    "{papel} iterado tem tipo {}, mas '{}' foi declarado como {}.",
                    type_name(oferecido),
                    decl.name,
                    type_name(&anotado)
                ),
            );
            return None;
        }
        Some(anotado)
    }

    /// Recusa mutação do container **durante** a iteração (PRD T71).
    ///
    /// Duas formas contam como mutação, e são exatamente as duas que o ADR
    /// 0007 já identifica como uso mutável de um composto:
    ///
    /// - escrever no container ou dentro dele — `v = ...`, `v[i] = ...`,
    ///   `v.campo = ...`, inclusive como um dos alvos de uma atribuição
    ///   múltipla (T67);
    /// - passá-lo como argumento de função, porque o codegen emite `&mut`
    ///   no call site independentemente do corpo do callee.
    ///
    /// A varredura é sintática e roda sobre a AST **antes** de o corpo ser
    /// tipado, de propósito: o nome do container ainda designa, em todo o
    /// corpo, o mesmo símbolo de fora do laço — as variáveis do laço só
    /// entram em escopo depois. O preço é que a varredura não tem escopo: um
    /// `local v = ...` que **sombreie** o container é aceito (declarar não é
    /// mutar), mas escrever no `v` novo depois disso é reportado como se
    /// fosse o container. Conservador na direção segura — recusa um programa
    /// válido em vez de aceitar um que o `rustc` recusaria —, e trocar o nome
    /// resolve.
    fn reject_mutacao_durante_iteracao(&mut self, block: &Stat, container: &str) {
        let mut ofensas = Vec::new();
        coleta_mutacoes(block, container, &mut ofensas);
        for loc in ofensas {
            self.error(
                loc,
                format!(
                    "não é possível modificar '{container}' dentro do `for`-in que \
                     itera sobre ele; itere sobre uma cópia ou colete as mudanças \
                     e aplique-as depois do laço."
                ),
            );
        }
    }

    /// Atribuição single-target `nome = exp` | `v[i] = exp` | `p.campo = exp`
    /// (`checker.lua:378-410`, estendido na T29 para `Index`/`Field`).
    ///
    /// A resolução do **alvo** — quem pode receber e com que tipo — mora em
    /// [`Self::check_assign_target`] desde a T67, porque a atribuição
    /// múltipla precisa exatamente dela, alvo por alvo.
    fn check_assign(&mut self, loc: Loc, var: &Var, exp: &Exp) -> Option<TypedStat> {
        let alvo = self.check_assign_target(loc, var)?;
        let value = self.check_exp(exp, Some(&alvo.ty))?;
        let value = self.coerce_assign_value(&alvo, value)?;
        self.mark_assigned(&alvo);
        Some(TypedStat::Assign {
            loc,
            target: alvo.lvalue,
            value,
        })
    }

    /// Resolve o lado esquerdo de uma atribuição: valida que o alvo pode
    /// receber (função/módulo/parâmetro são rejeitados, cada um com sua
    /// mensagem), devolve o `TypedLValue`, o tipo esperado do valor e o
    /// `DeclId` da local a marcar como mutável.
    fn check_assign_target(&mut self, loc: Loc, var: &Var) -> Option<AssignTarget> {
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

                let decl_id = match symbol.kind {
                    SymbolKind::Local { decl_id } => Some(decl_id),
                    _ => None,
                };
                Some(AssignTarget {
                    lvalue: TypedLValue::Name(name.clone()),
                    ty: symbol.ty,
                    decl_id,
                    name: Some(name.clone()),
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
                let lvalue = match target.kind {
                    TypedExpKind::Index { base, index } => TypedLValue::Index { base, index },
                    TypedExpKind::Field { base, name } => TypedLValue::Field { base, name },
                    // Inatingível: `check_var` só produz `Index`/`Field` para
                    // `VarBracket`/`VarDot`, os únicos braços deste `match`.
                    _ => unreachable!("check_var produziu um TypedExpKind inesperado"),
                };
                let decl_id = match root_symbol.kind {
                    SymbolKind::Local { decl_id } => Some(decl_id),
                    _ => None,
                };
                Some(AssignTarget {
                    lvalue,
                    ty: target_ty,
                    decl_id,
                    name: None,
                })
            }
        }
    }

    /// Confere a compatibilidade do valor com o alvo já resolvido. A
    /// mensagem nomeia a variável quando o alvo é um nome — as duas grafias
    /// que existiam antes da T67, preservadas palavra por palavra.
    fn coerce_assign_value(&mut self, alvo: &AssignTarget, value: TypedExp) -> Option<TypedExp> {
        // `x = 10` e `x = nil` com `x: integer?` (T68): mesma injeção da
        // declaração, no outro ponto em que o destino está escrito.
        let value = Self::widen_to_option(&alvo.ty, value);
        if alvo.ty.compatible(&value.ty) {
            return Some(value);
        }
        if self.reject_option_where_base_expected(&alvo.ty, &value) {
            return None;
        }
        let mensagem = match &alvo.name {
            Some(name) => format!(
                "atribuição incompatível para '{name}': esperado {}, encontrado {}.",
                type_name(&alvo.ty),
                type_name(&value.ty)
            ),
            None => format!(
                "atribuição incompatível: esperado {}, encontrado {}.",
                type_name(&alvo.ty),
                type_name(&value.ty)
            ),
        };
        self.error(value.loc, mensagem);
        None
    }

    // ---- `Option`/`?` (T68) ---------------------------------------------

    /// Ajusta um valor ao destino quando o destino é `T?` — a injeção
    /// `T → T?` e `nil → T?` (T68).
    ///
    /// `compatible` **não** faz esse trabalho de propósito: `Option` é
    /// invariante lá (ADR 0008), e tem de continuar sendo, senão
    /// `{integer}?` aceitaria `{value}?`. O que falta não é variância, é a
    /// **injeção** do tipo base no opcional, que só é válida num sentido e
    /// só num ponto conhecido: onde o destino está escrito. É o mesmo lugar
    /// em que o Titan original insere seu `trycoerce`
    /// (`checker.lua:926-930`), e é por isso que a marcação vira um nó
    /// (`SomeOf`) em vez de uma reescrita silenciosa de `ty`: o codegen
    /// (T69) emite o `Some(...)` exatamente aqui.
    ///
    /// Valores que já são do tipo do destino passam intactos, e isso inclui
    /// um `T?` entregue a um `T?` — a injeção não se aplica duas vezes.
    fn widen_to_option(expected: &Type, value: TypedExp) -> TypedExp {
        let Type::Option { base } = expected else {
            return value;
        };
        match &value.ty {
            // `nil` não vira `Some(nil)`: vira `None`, e o nó continua
            // sendo o literal — só o tipo passa a ser o opcional.
            Type::Nil => TypedExp {
                ty: expected.clone(),
                ..value
            },
            // `T?` já é o destino (ou é incompatível com ele, e quem chama
            // reporta): nada a injetar.
            Type::Option { .. } => value,
            found if base.compatible(found) => TypedExp {
                loc: value.loc,
                ty: expected.clone(),
                kind: TypedExpKind::SomeOf(Box::new(value)),
            },
            _ => value,
        }
    }

    /// Recusa um valor opcional usado onde o tipo base é exigido — a regra
    /// "usar um `T?` sem testar é erro claro" da T68.
    ///
    /// A mensagem nomeia a variável quando dá (o caso que importa, porque é
    /// o nome que o usuário vai escrever no teste) e sempre aponta a saída:
    /// o `if ... ~= nil`, que é o único desembrulho que a linguagem tem.
    ///
    /// Devolve `true` quando recusou, para o chamador encadear com os `ok`
    /// que já acumula.
    fn reject_option(&mut self, exp: &TypedExp) -> bool {
        let Type::Option { base } = &exp.ty else {
            return false;
        };
        let base = type_name(base);
        let mensagem = match &exp.kind {
            TypedExpKind::Var(name) => format!(
                "'{name}' é {base}? e pode ser nil: teste com `if {name} ~= nil then` antes de usar como {base}."
            ),
            _ => format!(
                "este valor é {base}? e pode ser nil: guarde-o num `local` e teste com `if nome ~= nil then` antes de usar como {base}."
            ),
        };
        self.error(exp.loc, mensagem);
        true
    }

    /// Recusa opcional em cada operando de uma vez, sem curto-circuito: os
    /// dois lados de `a + b` com ambos opcionais rendem os dois erros, como
    /// em toda checagem de operando deste arquivo.
    fn reject_option_operands(&mut self, sides: [&TypedExp; 2]) -> bool {
        // `|` e não `||`: os dois lados precisam ser avaliados.
        self.reject_option(sides[0]) | self.reject_option(sides[1])
    }

    /// O mesmo erro, mas na checagem de compatibilidade com um destino
    /// escrito (declaração, atribuição, argumento, retorno): entregar um
    /// `T?` onde se pede `T` é o caso principal de "usar sem testar", e
    /// "esperado integer, encontrado integer?" não diz o que fazer.
    ///
    /// Só fala quando o destino **não** é opcional: entre dois opcionais de
    /// bases diferentes o problema é de tipo, não de ausência, e aí a
    /// mensagem genérica do chamador é a certa.
    ///
    /// Devolve `true` quando recusou.
    fn reject_option_where_base_expected(
        &mut self,
        expected: &Type,
        found: &TypedExp,
    ) -> bool {
        if matches!(expected, Type::Option { .. } | Type::Value) {
            return false;
        }
        self.reject_option(found)
    }

    /// Registra o `DeclId` do alvo em `self.assigned` — é o que faz
    /// `fixup_mutability` emitir `let mut` na declaração correspondente.
    fn mark_assigned(&mut self, alvo: &AssignTarget) {
        if let Some(decl_id) = alvo.decl_id {
            self.assigned.insert(decl_id);
        }
    }

    /// `local a, b = ...` (T67). Duas formas, e só duas: **uma** chamada que
    /// devolve tantos valores quantos são os alvos (`local q, r =
    /// divmod(7, 2)` — a desestruturação da tupla da T66), ou **uma
    /// expressão por alvo** (`local a, b = 1, 2`). Qualquer outra combinação
    /// de aridade é erro claro — Titan não tem o preenchimento silencioso
    /// com `nil` do Lua.
    fn check_decl_multi(&mut self, loc: Loc, decls: &[Decl], exps: &[Exp]) -> Option<TypedStat> {
        // Os tipos anotados valem de contexto para os valores (mesma razão
        // da T29 na declaração simples), então são resolvidos antes.
        let mut anotados = Vec::with_capacity(decls.len());
        for decl in decls {
            let anotado = match &decl.r#type {
                Some(annotated) => Some(self.resolve_type(annotated)?),
                None => None,
            };
            anotados.push(anotado);
        }

        // `local a?, b = ...` (T68): o `?` do lado do nome infere o tipo do
        // valor, e a forma múltipla não tem como fazer isso — o tipo do
        // alvo entra em `check_multi_values` **antes** de o valor ser
        // tipado. Em vez de inferir errado, erro claro com a saída escrita.
        for decl in decls {
            if decl.option {
                self.error(
                    decl.loc,
                    format!(
                        "`{}?` não vale em declaração múltipla: escreva o tipo (`local {}: T?, ...`).",
                        decl.name, decl.name
                    ),
                );
                return None;
            }
        }

        let (values, tipos): (TypedMultiValues, Vec<Type>) =
            self.check_multi_values(loc, &anotados, exps, "declaração")?;

        // Só depois de tudo tipado os nomes entram no escopo: em Titan,
        // como em Lua, o lado direito de um `local` enxerga o escopo de
        // **fora** da declaração (`local x = x` lê o `x` externo).
        let mut targets = Vec::with_capacity(decls.len());
        for (i, decl) in decls.iter().enumerate() {
            let ty = tipos[i].clone();
            let decl_id = self.next_decl_id;
            self.next_decl_id += 1;
            self.st.add_symbol(
                &decl.name,
                ty.clone(),
                SymbolKind::Local { decl_id },
                decl.loc,
            );
            // Hover sobre o próprio nome declarado (T49), como na simples.
            self.record_use(decl.loc, decl.loc, &decl.name, &ty);
            targets.push(TypedDeclTarget {
                loc: decl.loc,
                name: decl.name.clone(),
                ty,
                decl_id,
                mutable: false,
            });
        }

        Some(TypedStat::DeclMulti {
            loc,
            targets,
            values,
        })
    }

    /// `a, b = ...` (T67). Mesmas duas formas de aridade da declaração
    /// múltipla; a diferença é que os alvos já existem, então cada um passa
    /// por [`Self::check_assign_target`] — o que mantém intactas as
    /// rejeições de atribuir a função, módulo ou parâmetro — e **todos**
    /// entram em `self.assigned`, para o fix-up marcar cada declaração
    /// correspondente como `mut` (exigência explícita da tarefa).
    fn check_assign_multi(&mut self, loc: Loc, vars: &[Var], exps: &[Exp]) -> Option<TypedStat> {
        let mut alvos = Vec::with_capacity(vars.len());
        let mut ok = true;
        for var in vars {
            match self.check_assign_target(loc, var) {
                Some(alvo) => alvos.push(alvo),
                None => ok = false,
            }
        }
        if !ok {
            return None;
        }

        let esperados: Vec<Option<Type>> = alvos.iter().map(|a| Some(a.ty.clone())).collect();
        let (values, _) = self.check_multi_values(loc, &esperados, exps, "atribuição")?;

        // A compatibilidade de cada valor com o seu alvo já foi conferida
        // dentro de `check_multi_values` (que recebeu os tipos esperados);
        // aqui só resta registrar a mutabilidade — de todos.
        for alvo in &alvos {
            self.mark_assigned(alvo);
        }

        Some(TypedStat::AssignMulti {
            loc,
            targets: alvos.into_iter().map(|a| a.lvalue).collect(),
            values,
        })
    }

    /// O lado direito comum a `local a, b = ...` e a `a, b = ...` (T67).
    ///
    /// `esperados` traz, por posição, o tipo que aquele alvo exige (o
    /// anotado da declaração ou o da variável já existente) ou `None`
    /// quando o tipo vem do próprio valor. Devolve os valores tipados e o
    /// tipo final de cada posição.
    ///
    /// `contexto` só nomeia a construção nas mensagens de erro
    /// ("declaração"/"atribuição").
    fn check_multi_values(
        &mut self,
        loc: Loc,
        esperados: &[Option<Type>],
        exps: &[Exp],
        contexto: &str,
    ) -> Option<(TypedMultiValues, Vec<Type>)> {
        let alvos = esperados.len();

        // Forma 1: uma chamada só, desestruturada. Reconhecida pela forma
        // do fonte (`ExpCall` sozinho), e é a aridade **declarada** da
        // função que precisa bater com a dos alvos.
        if exps.len() == 1
            && alvos > 1
            && let Exp::ExpCall {
                loc: call_loc,
                exp,
                args,
            } = &exps[0]
        {
            // Chamada crua, sem `adjust_to_one`: é a aridade completa
            // que interessa, como em `check_extra` (T65).
            let (call, rettypes) = self.check_call(call_loc, exp, args)?;
            if rettypes.len() != alvos {
                self.error(
                    loc,
                    format!(
                        "{contexto} múltipla com {alvos} alvo(s), mas a chamada produz {} valor(es) de retorno.",
                        rettypes.len()
                    ),
                );
                return None;
            }
            // Cada componente da tupla precisa caber no seu alvo.
            //
            // Aqui **não** cabe a injeção `T → T?` da T68: a tupla é
            // desestruturada num `let` só, e não há expressão por alvo onde
            // pendurar o `Some(...)`. Quem quiser um `T?` a partir de uma
            // função que devolve `T` declara o retorno como `T?`; o caso
            // contrário cai na mensagem de tipos incompatíveis logo abaixo,
            // que já nomeia os dois tipos.
            let mut tipos = Vec::with_capacity(alvos);
            for (i, rettype) in rettypes.iter().enumerate() {
                match &esperados[i] {
                    Some(esperado) => {
                        if !esperado.compatible(rettype) {
                            self.error(
                                loc,
                                format!(
                                    "tipos incompatíveis no {}º alvo da {contexto} múltipla: esperado {}, encontrado {}.",
                                    i + 1,
                                    type_name(esperado),
                                    type_name(rettype)
                                ),
                            );
                            return None;
                        }
                        tipos.push(esperado.clone());
                    }
                    None => tipos.push(rettype.clone()),
                }
            }
            return Some((TypedMultiValues::Call(call), tipos));
        }

        // Forma 2: uma expressão por alvo.
        if exps.len() != alvos {
            self.error(
                loc,
                format!(
                    "{contexto} múltipla com {alvos} alvo(s), mas {} valor(es) à direita.",
                    exps.len()
                ),
            );
            return None;
        }

        let mut typed = Vec::with_capacity(alvos);
        let mut tipos = Vec::with_capacity(alvos);
        for (i, exp) in exps.iter().enumerate() {
            let esperado = esperados[i].as_ref();
            let value = self.check_exp(exp, esperado)?;
            // `T → T?` / `nil → T?` (T68), igual à forma single-target.
            let value = match esperado {
                Some(esperado) => Self::widen_to_option(esperado, value),
                None => value,
            };
            match esperado {
                Some(esperado) => {
                    if !esperado.compatible(&value.ty) {
                        if self.reject_option_where_base_expected(esperado, &value) {
                            return None;
                        }
                        self.error(
                            value.loc,
                            format!(
                                "tipos incompatíveis no {}º alvo da {contexto} múltipla: esperado {}, encontrado {}.",
                                i + 1,
                                type_name(esperado),
                                type_name(&value.ty)
                            ),
                        );
                        return None;
                    }
                    tipos.push(esperado.clone());
                }
                None => tipos.push(value.ty.clone()),
            }
            typed.push(value);
        }
        Some((TypedMultiValues::List(typed), tipos))
    }

    /// Tipa uma expressão. `context`, acrescentado na T29 (PRD.md), é o tipo
    /// esperado nesta posição quando conhecido de antemão (anotação de
    /// `local`, tipo de parâmetro/retorno, elemento de array/map, campo de
    /// record) — só `ExpInitList` o consome (a desambiguação de `{...}`
    /// depende dele), mas ele precisa atravessar todo `check_exp` para
    /// chegar até um `{...}` aninhado em qualquer posição.
    fn check_exp(&mut self, exp: &Exp, context: Option<&Type>) -> Option<TypedExp> {
        self.touch_loc(exp_loc(exp));
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
                            // `T?` concatenado tem a mensagem da T68, não a
                            // genérica de operando de `..`.
                            if self.reject_option(&typed) {
                                ok = false;
                                typed_exps.push(typed);
                                continue;
                            }
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
            // Chamada em posição de expressão: com N>1 retornos, ajusta
            // para o primeiro valor (T65).
            Exp::ExpCall { loc, exp, args } => {
                let (call, rettypes) = self.check_call(loc, exp, args)?;
                Some(self.adjust_to_one(call, &rettypes))
            }
            Exp::ExpInitList { loc, fields } => self.check_init_list(*loc, fields, context),
            Exp::ExpUnop { loc, op, exp } => self.check_unop(*loc, op, exp),
            Exp::ExpBinop { loc, lhs, op, rhs } => self.check_binop(*loc, op, lhs, rhs),
            Exp::ExpCast { loc, exp, target } => self.check_cast(*loc, exp, target),
            // `ExpAdjust`/`ExpExtra` (T65): nós de ajuste de retorno
            // múltiplo. O parser não os produz a partir do fonte — quem
            // monta o `Adjust` é o próprio `check_exp` no braço de
            // `ExpCall` —, mas a AST os expõe e o checker os tipa de
            // verdade em vez de rejeitá-los.
            Exp::ExpAdjust { exp, .. } => match exp.as_ref() {
                Exp::ExpCall {
                    loc: call_loc,
                    exp,
                    args,
                } => {
                    let (call, rettypes) = self.check_call(call_loc, exp, args)?;
                    Some(self.adjust_to_one(call, &rettypes))
                }
                // Ajustar o que já é escalar não muda nada.
                other => self.check_exp(other, context),
            },
            Exp::ExpExtra {
                loc, exp, index, ..
            } => self.check_extra(*loc, exp, *index),
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
            // `local xs: {integer}? = {1, 2}` (T68): num destino opcional,
            // quem desambigua o `{...}` é o tipo **base** — o `?` diz o que
            // o destino aceita além do valor, não que forma o valor tem. A
            // injeção `T → T?` acontece depois, em `widen_to_option`, no
            // ponto onde o destino está escrito.
            Some(Type::Option { base }) => self.check_init_list(loc, fields, Some(base.as_ref())),
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

    /// Regras de tipo dos operadores binários (T13/T61), espelhando
    /// `checker.lua:910-1122` sem gradual typing.
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

        // Um `T?` só é operando legítimo de `==`/`~=` (o teste de presença);
        // em qualquer outro operador é o erro "pode ser nil" da T68, e é
        // melhor dá-lo aqui, uma vez, do que deixar cada regra abaixo
        // reclamar que o operando "não é numérico" — o problema não é o
        // tipo base, é a ausência possível.
        if !matches!(op, BinOp::Eq | BinOp::Ne) && self.reject_option_operands([&lhs, &rhs]) {
            return None;
        }

        let ty = match op {
            // Ambos numéricos; int/int → int, qualquer float promove a float.
            // `//` entra aqui: no original é o mesmo braço de `+ - * %`
            // (`checker.lua:988`) — int/int → int, qualquer float promove.
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Mod | BinOp::IDiv => {
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
            // string/string ou boolean/boolean — mais o teste de presença
            // `T? == nil` / `T? ~= nil` (T68), o **único** desembrulho que
            // a linguagem oferece e a porta de entrada do estreitamento.
            //
            // `T == nil` com `T` não-opcional segue sendo erro: a resposta
            // seria constante, e a pergunta quase sempre denuncia um tipo
            // escrito errado.
            BinOp::Eq | BinOp::Ne => {
                let testa_presenca = (matches!(lhs.ty, Type::Option { .. })
                    && matches!(rhs.ty, Type::Nil))
                    || (matches!(lhs.ty, Type::Nil) && matches!(rhs.ty, Type::Option { .. }));
                let both_numeric = is_numeric(&lhs.ty) && is_numeric(&rhs.ty);
                let same_primitive =
                    lhs.ty.equals(&rhs.ty) && matches!(lhs.ty, Type::String | Type::Boolean);
                if !(testa_presenca || both_numeric || same_primitive) {
                    // Um opcional comparado com algo que não é `nil`
                    // (`x == 10`) é o erro de "usar sem testar", não o de
                    // tipos incomparáveis: a mensagem que ensina a saída
                    // vale mais aqui.
                    if self.reject_option_operands([&lhs, &rhs]) {
                        return None;
                    }
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
            // Bitwise exige `Integer` dos dois lados, resultado `Integer`.
            // Divergência deliberada de `checker.lua:1097-1109`, que coage
            // float para integer (ADR 0021): aqui `1.5 & 2` é erro claro em
            // português, e não uma truncagem silenciosa — quem quiser
            // truncar escreve o cast (T71). Nenhuma promoção int→float
            // acontece, então o resultado é sempre `Integer`.
            BinOp::BAnd | BinOp::BOr | BinOp::BXor | BinOp::Shl | BinOp::Shr => {
                if !self.check_integer_operands(op_str, &lhs, &rhs) {
                    return None;
                }
                Type::Integer
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

    /// Reporta um erro por lado não-inteiro de um operador bitwise,
    /// apontando o `loc` do operando culpado. Separado de
    /// [`Checker::check_numeric_operands`] porque bitwise **não** aceita
    /// float: a mensagem precisa dizer `integer`, não "numérico".
    fn check_integer_operands(&mut self, op_str: &str, lhs: &TypedExp, rhs: &TypedExp) -> bool {
        let mut ok = true;
        for side in [lhs, rhs] {
            if !side.ty.equals(&Type::Integer) {
                self.error(
                    side.loc,
                    format!(
                        "operando de `{op_str}` precisa ser integer, encontrado {}.",
                        type_name(&side.ty)
                    ),
                );
                ok = false;
            }
        }
        ok
    }

    /// Regras de tipo do cast `as` (T70).
    ///
    /// **Cast não é parsing.** Só três famílias de conversão passam:
    ///
    /// - `integer ↔ float` — numérica, a única que muda a representação de um
    ///   primitivo. `3.9 as integer` **trunca** para `3` (e `-3.9` para `-3`),
    ///   porque é o `as` do Rust por baixo; isso difere do `//` da T61, que
    ///   arredonda para baixo e daria `-4`. A divergência é deliberada e está
    ///   documentada no README.
    /// - **qualquer tipo → `value`** — a subida ao topo do gradual typing,
    ///   sempre permitida.
    /// - **`value` → qualquer tipo** — a descida, checada em tempo de
    ///   execução: se o `value` não guardar aquele tipo, o programa aborta com
    ///   mensagem em português, como toda falha de runtime do Titan.
    ///
    /// Tudo o mais é erro de compilação com mensagem que diz o que o `as`
    /// **não** faz: `"a" as integer` não parseia a string, e quem quer isso
    /// está pedindo outra operação, não um cast.
    ///
    /// O cast para o próprio tipo (`x as integer` com `x: integer`) é aceito e
    /// vira identidade — recusá-lo só criaria atrito em código genérico sem
    /// proteger nada.
    fn check_cast(&mut self, loc: Loc, exp: &Exp, target: &ast::Type) -> Option<TypedExp> {
        // O alvo é resolvido **antes** do operando para que `x as {inexistente}`
        // reclame do tipo, que é o erro mais próximo do que o programador
        // escreveu.
        let target_ty = self.resolve_type(target)?;
        let typed = self.check_exp(exp, Some(&target_ty))?;
        let origem = typed.ty.clone();

        let kind = match (&origem, &target_ty) {
            // Identidade: o operando já é do tipo pedido.
            (o, t) if o.equals(t) => return Some(typed),
            // Numérica, nos dois sentidos.
            (Type::Integer, Type::Float) => CastKind::IntToFloat,
            (Type::Float, Type::Integer) => CastKind::FloatToInt,
            // Subida ao topo do gradual typing.
            (_, Type::Value) => CastKind::ToValue,
            // Descida do topo, checada em tempo de execução. Só primitiva:
            // `v as {integer}` teria de converter elemento a elemento e
            // poderia falhar no meio, com metade do array já construído —
            // um ponto de falha por cast é o contrato mais simples de
            // explicar, e quem precisa do composto desce campo a campo.
            (Type::Value, Type::Boolean | Type::Integer | Type::Float | Type::String) => {
                CastKind::FromValue
            }
            (Type::Value, alvo) => {
                self.error(
                    loc,
                    format!(
                        "`value` só desce para tipo primitivo (`boolean`, `integer`, \
                         `float`, `string`), não para {}.",
                        type_name(alvo)
                    ),
                );
                return None;
            }
            _ => {
                self.error(
                    loc,
                    format!(
                        "não existe cast de {} para {}: `as` converte entre números \
                         (`integer`/`float`) e de/para `value`, não interpreta texto \
                         nem reinterpreta compostos.",
                        type_name(&origem),
                        type_name(&target_ty)
                    ),
                );
                return None;
            }
        };

        Some(TypedExp {
            loc,
            ty: target_ty,
            kind: TypedExpKind::Cast {
                kind,
                exp: Box::new(typed),
            },
        })
    }

    /// Regras de tipo dos operadores unários (T13/T29): `-` numérico preserva
    /// o tipo do operando; `not` é boolean → boolean (`checker.lua:1100-1122`);
    /// `#` (`checker.lua:852-859`) sobre `Array`/`String` resulta `Integer` —
    /// `parser::parse_unary_exp` produz `#` como prefixo de expressão desde a
    /// T30 (lacuna do parser fechada ali; `check_unop` já sabia mapear `"#"`
    /// desde a T29). `~` (bitwise NOT) entrou na T61: `Integer` → `Integer`,
    /// sem coerção de float.
    fn check_unop(&mut self, loc: Loc, op_str: &str, exp: &Exp) -> Option<TypedExp> {
        let op = match op_str {
            "-" => UnOp::Neg,
            "not" => UnOp::Not,
            "#" => UnOp::Len,
            // `~` unário é bitwise NOT (T61) — o XOR é o `~` binário.
            "~" => UnOp::BNot,
            // Defensivo para AST montada à mão: o parser (T60) só produz
            // `-`, `not`, `#` e `~` como prefixo.
            _ => {
                self.error(
                    loc,
                    format!("operador unário `{op_str}` não é suportado nesta fase."),
                );
                return None;
            }
        };

        let exp = self.check_exp(exp, None)?;
        // Mesma regra do binário: `-x`, `not x`, `#x` e `~x` exigem o valor
        // presente (T68).
        if self.reject_option(&exp) {
            return None;
        }
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
            // Mesma exigência do bitwise binário: `Integer` estrito, sem
            // coerção de float (ADR 0021 — divergência deliberada de
            // `checker.lua:870-881`).
            UnOp::BNot => {
                if !exp.ty.equals(&Type::Integer) {
                    self.error(
                        exp.loc,
                        format!(
                            "operando de `~` precisa ser integer, encontrado {}.",
                            type_name(&exp.ty)
                        ),
                    );
                    return None;
                }
                Type::Integer
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
                // `xs[i]` com `xs: {integer}?` (T68): indexar um possível
                // `nil` é o erro de "usar sem testar", não "não é possível
                // indexar".
                if self.reject_option(&base) {
                    return None;
                }
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
                if self.reject_option(&index) {
                    return None;
                }
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
                // `p.x` com `p: Ponto?` (T68), pelo mesmo motivo da
                // indexação.
                if self.reject_option(&base) {
                    return None;
                }
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
                let callee = if self.foreigns.contains(name) {
                    Callee::Foreign(name.clone())
                } else {
                    Callee::Direct(name.clone())
                };
                Some((callee, name.clone(), params, rettypes))
            }
            // `data.read_csv(...)` (T39): base é o símbolo de um módulo
            // importado — resolve contra a tabela de capabilities em vez da
            // pilha de escopos.
            //
            // `local_name` é o que o programa escreveu (`d` em `import data
            // as d`, T72) e é o que aparece nas mensagens de erro; o
            // `Callee::Module` carrega o nome real do módulo, que é o que o
            // codegen resolve contra `capabilities::lookup_module`.
            Var::VarDot { exp, name, .. } if self.dot_base_module(exp).is_some() => {
                let local_name = self.dot_base_module(exp).expect("checado acima");
                let capability = *self
                    .modules
                    .get(&local_name)
                    .expect("dot_base_module só devolve módulo importado");
                let Some(function) = capability.find_function(name) else {
                    self.error(
                        *loc,
                        format!("o módulo '{local_name}' não tem função '{name}'."),
                    );
                    return None;
                };
                let module = capability.titan_name.to_string();
                Some((
                    Callee::Module {
                        module: module.clone(),
                        name: name.clone(),
                    },
                    format!("{local_name}.{name}"),
                    function.params.to_vec(),
                    vec![requalify_rettype(&function.rettype, &module)],
                ))
            }
            // `df.soma(...)` (T40) — delegado a `resolve_method_callee`,
            // que a forma com dois-pontos (`df:soma(...)`, T72) também usa.
            Var::VarDot { exp, name, .. } => self.resolve_method_callee(loc, exp, name),
            _ => {
                self.error(
                    *loc,
                    "só é possível chamar um nome de função diretamente nesta fase.",
                );
                None
            }
        }
    }

    /// Resolve a chamada de método sobre um receptor de tipo `Opaque` — o
    /// ponto em que `df.soma(...)` (T40) e `df:soma(...)` (T72) se
    /// encontram. As duas formas diferem só em **onde o parser guarda o
    /// nome do método** (`Var::VarDot` versus `Args::ArgsMethod`); daqui
    /// para baixo são a mesma coisa, e produzem o mesmo `Callee::Method`.
    ///
    /// O método é resolvido contra a capability do módulo que originou o
    /// opaco (`Type::Opaque::module`, preenchido por `requalify_rettype` em
    /// T39). O receptor conta como uso mutável pela mesma regra de
    /// `check_assign` (`is_composite` inclui `Opaque`).
    fn resolve_method_callee(
        &mut self,
        loc: &Loc,
        recv_exp: &Exp,
        name: &str,
    ) -> Option<(Callee, String, Vec<Type>, Vec<Type>)> {
        let receiver = self.check_exp(recv_exp, None)?;
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
        if let Exp::ExpVar { var, .. } = recv_exp
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
                name: name.to_string(),
            },
            format!("{recv_name}.{name}"),
            method.params.to_vec(),
            vec![requalify_rettype(&method.rettype, &module)],
        ))
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

    /// Tipa uma chamada **crua**: devolve o `TypedExp` da chamada (cujo `ty`
    /// é o do primeiro retorno) junto da lista completa de tipos de retorno
    /// da assinatura. Quem chama decide o que fazer com os valores além do
    /// primeiro — [`Self::adjust_to_one`] em posição de expressão, descarte
    /// em posição de comando (T65).
    fn check_call(
        &mut self,
        loc: &Loc,
        callee: &Exp,
        args: &Args,
    ) -> Option<(TypedExp, Vec<Type>)> {
        // As duas formas de chamada (T72). `ArgsFunc` é `f(x)`,
        // `data.f(x)` e `df.f(x)` — quem é chamado está todo em `callee`.
        // `ArgsMethod` é `df:f(x)`: o receptor é o `callee` e o nome do
        // método vem nos argumentos, então a resolução vai direto ao braço
        // de método, sem passar por `resolve_callee`.
        let (callee, name, params, rettypes, arg_exps) = match args {
            Args::ArgsFunc { args: arg_exps, .. } => {
                let (callee, name, params, rettypes) = self.resolve_callee(loc, callee)?;
                (callee, name, params, rettypes, arg_exps)
            }
            Args::ArgsMethod {
                method,
                args: arg_exps,
                ..
            } => {
                let (callee, name, params, rettypes) =
                    self.resolve_method_callee(loc, callee, method)?;
                (callee, name, params, rettypes, arg_exps)
            }
        };

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

        // `f(10)` com `f(x: integer?)` (T68): a injeção `T → T?` vale no
        // argumento como vale na declaração — o tipo do destino está
        // escrito, é o do parâmetro.
        let typed_args: Vec<TypedExp> = typed_args
            .into_iter()
            .zip(&params)
            .map(|(arg, expected)| Self::widen_to_option(expected, arg))
            .collect();

        for (arg, expected) in typed_args.iter().zip(&params) {
            if !expected.compatible(&arg.ty) {
                if self.reject_option_where_base_expected(expected, arg) {
                    return None;
                }
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

        // `rettypes[0]` sempre existe porque toda assinatura coletada tem ao
        // menos um tipo de retorno (`TypeNil` quando omitido). Com N>1
        // retornos (T65) o tipo da chamada crua é o do primeiro valor.
        let ty = rettypes.first().cloned().unwrap_or(Type::Nil);

        Some((
            TypedExp {
                loc: *loc,
                ty,
                kind: TypedExpKind::Call {
                    callee,
                    args: typed_args,
                },
            },
            rettypes,
        ))
    }

    /// `ExpExtra` (T65): o `index`-ésimo (base 0) valor de retorno de uma
    /// chamada. Só faz sentido sobre uma chamada, e o índice precisa existir
    /// na assinatura.
    fn check_extra(&mut self, loc: Loc, exp: &Exp, index: usize) -> Option<TypedExp> {
        let Exp::ExpCall {
            loc: call_loc,
            exp,
            args,
        } = exp
        else {
            self.error(
                loc,
                "só uma chamada de função produz valores de retorno extras.",
            );
            return None;
        };
        // Chamada crua, sem passar por `adjust_to_one` — é justamente a
        // aridade completa que interessa aqui.
        let (inner, rettypes) = self.check_call(call_loc, exp, args)?;
        let Some(ty) = rettypes.get(index).cloned() else {
            self.error(
                loc,
                format!(
                    "a chamada produz {} valor(es) de retorno, mas foi pedido o {}º.",
                    rettypes.len(),
                    index + 1
                ),
            );
            return None;
        };
        Some(TypedExp {
            loc,
            ty,
            kind: TypedExpKind::Extra {
                exp: Box::new(inner),
                index,
            },
        })
    }

    /// Envolve uma chamada com N>1 retornos num `Adjust` — posição escalar
    /// fica com o primeiro valor (T65). Uma chamada de retorno único passa
    /// intacta, e o caso comum do programa continua exatamente como era.
    fn adjust_to_one(&self, call: TypedExp, rettypes: &[Type]) -> TypedExp {
        if rettypes.len() <= 1 {
            return call;
        }
        TypedExp {
            loc: call.loc,
            ty: call.ty.clone(),
            kind: TypedExpKind::Adjust(Box::new(call)),
        }
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
        // T67: **todos** os alvos da declaração múltipla são marcados, não
        // só o primeiro — `local a, b = f()` seguido de `a = 1` precisa
        // sair com `a` mutável e `b` não.
        TypedStat::DeclMulti { targets, .. } => {
            for target in targets {
                target.mutable = assigned.contains(&target.decl_id);
            }
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
        TypedStat::While { block, .. }
        | TypedStat::Repeat { block, .. }
        | TypedStat::For { block, .. }
        | TypedStat::ForIn { block, .. } => {
            fixup_mutability(block, assigned);
        }
        TypedStat::Call { .. }
        | TypedStat::Return { .. }
        | TypedStat::Assign { .. }
        | TypedStat::AssignMulti { .. }
        | TypedStat::Break { .. }
        | TypedStat::Continue { .. } => {}
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
/// O nome testado por `nome ~= nil` (ou `nil ~= nome`) — o gatilho do
/// estreitamento de fluxo da T68.
///
/// Reconhece **só** a variável nua, nas duas ordens. `p.campo ~= nil` e
/// `xs[i] ~= nil` ficam de fora de propósito: estreitar um lugar exigiria
/// provar que nada entre o teste e o uso o reescreveu, e não há nada no
/// checker que prove isso hoje. `==` também fica de fora: ele estreitaria o
/// ramo `else`, e este método só responde pelo ramo `then`.
fn presence_test_name(exp: &Exp) -> Option<String> {
    let Exp::ExpBinop { lhs, op, rhs, .. } = exp else {
        return None;
    };
    if op != "~=" {
        return None;
    }
    match (lhs.as_ref(), rhs.as_ref()) {
        (Exp::ExpVar { var, .. }, Exp::ExpNil { .. })
        | (Exp::ExpNil { .. }, Exp::ExpVar { var, .. }) => match var.as_ref() {
            Var::VarName { name, .. } => Some(name.clone()),
            _ => None,
        },
        _ => None,
    }
}

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

/// Acumula em `ofensas` cada `Loc` do bloco em que `container` é mutado
/// (T71) — ver [`Checker::reject_mutacao_durante_iteracao`], onde a política
/// está explicada; aqui está só a travessia.
///
/// A travessia é a da AST inteira, laços aninhados inclusive: `for x in v do
/// for y in w do v[1] = 0 end end` muta `v` durante a iteração de `v`
/// exatamente como se estivesse um nível acima.
fn coleta_mutacoes(stat: &Stat, container: &str, ofensas: &mut Vec<Loc>) {
    match stat {
        Stat::StatBlock { stats, .. } => {
            for stat in stats {
                coleta_mutacoes(stat, container, ofensas);
            }
        }
        Stat::StatWhile {
            condition, block, ..
        } => {
            coleta_mutacoes_exp(condition, container, ofensas);
            coleta_mutacoes(block, container, ofensas);
        }
        Stat::StatRepeat {
            block, condition, ..
        } => {
            coleta_mutacoes(block, container, ofensas);
            coleta_mutacoes_exp(condition, container, ofensas);
        }
        Stat::StatIf {
            thens, elsestat, ..
        } => {
            for then in thens {
                coleta_mutacoes_exp(&then.condition, container, ofensas);
                coleta_mutacoes(&then.block, container, ofensas);
            }
            if let Some(elsestat) = elsestat {
                coleta_mutacoes(elsestat, container, ofensas);
            }
        }
        Stat::StatFor {
            start,
            finish,
            inc,
            block,
            ..
        } => {
            coleta_mutacoes_exp(start, container, ofensas);
            coleta_mutacoes_exp(finish, container, ofensas);
            if let Some(inc) = inc {
                coleta_mutacoes_exp(inc, container, ofensas);
            }
            coleta_mutacoes(block, container, ofensas);
        }
        Stat::StatForIn { exp, block, .. } => {
            coleta_mutacoes_exp(exp, container, ofensas);
            coleta_mutacoes(block, container, ofensas);
        }
        Stat::StatAssign { vars, exps, .. } => {
            for var in vars {
                if root_var_name(var).as_deref() == Some(container) {
                    ofensas.push(var_loc(var));
                }
            }
            for exp in exps {
                coleta_mutacoes_exp(exp, container, ofensas);
            }
        }
        // `local v = ...` dentro do corpo: o nome passa a designar outra
        // coisa, mas a varredura é anterior à tipagem e não tem escopo — o
        // conservadorismo é declarado em `reject_mutacao_durante_iteracao`.
        // O que interessa aqui são os **valores**, que podem conter chamadas.
        Stat::StatDecl { exps, .. } => {
            for exp in exps {
                coleta_mutacoes_exp(exp, container, ofensas);
            }
        }
        Stat::StatCall { callexp, .. } => coleta_mutacoes_exp(callexp, container, ofensas),
        Stat::StatReturn { exps, .. } => {
            for exp in exps {
                coleta_mutacoes_exp(exp, container, ofensas);
            }
        }
        Stat::StatBreak { .. } | Stat::StatContinue { .. } => {}
    }
}

/// A metade de [`coleta_mutacoes`] que percorre expressões. Só uma forma de
/// expressão muta um composto: passá-lo como argumento, porque o codegen
/// emite `&mut` no call site (ADR 0007). Ler `v[i]` não muta nada.
fn coleta_mutacoes_exp(exp: &Exp, container: &str, ofensas: &mut Vec<Loc>) {
    match exp {
        Exp::ExpCall { exp, args, .. } => {
            coleta_mutacoes_exp(exp, container, ofensas);
            let args = match args {
                Args::ArgsFunc { args, .. } => args,
                Args::ArgsMethod { args, .. } => args,
            };
            for arg in args {
                if root_exp_var_name(arg).as_deref() == Some(container) {
                    ofensas.push(exp_loc(arg));
                }
                coleta_mutacoes_exp(arg, container, ofensas);
            }
        }
        Exp::ExpVar { var, .. } => coleta_mutacoes_var(var, container, ofensas),
        Exp::ExpUnop { exp, .. }
        | Exp::ExpCast { exp, .. }
        | Exp::ExpAdjust { exp, .. }
        | Exp::ExpExtra { exp, .. } => coleta_mutacoes_exp(exp, container, ofensas),
        Exp::ExpBinop { lhs, rhs, .. } => {
            coleta_mutacoes_exp(lhs, container, ofensas);
            coleta_mutacoes_exp(rhs, container, ofensas);
        }
        Exp::ExpConcat { exps, .. } => {
            for exp in exps {
                coleta_mutacoes_exp(exp, container, ofensas);
            }
        }
        Exp::ExpInitList { fields, .. } => {
            for field in fields {
                if let FieldName::Key(key) = &field.name {
                    coleta_mutacoes_exp(key, container, ofensas);
                }
                coleta_mutacoes_exp(&field.exp, container, ofensas);
            }
        }
        Exp::ExpNil { .. }
        | Exp::ExpBool { .. }
        | Exp::ExpInteger { .. }
        | Exp::ExpFloat { .. }
        | Exp::ExpString { .. } => {}
    }
}

/// `v[f(w)]` e `p.campo` também carregam expressões dentro.
fn coleta_mutacoes_var(var: &Var, container: &str, ofensas: &mut Vec<Loc>) {
    match var {
        Var::VarName { .. } => {}
        Var::VarBracket { exp1, exp2, .. } => {
            coleta_mutacoes_exp(exp1, container, ofensas);
            coleta_mutacoes_exp(exp2, container, ofensas);
        }
        Var::VarDot { exp, .. } => coleta_mutacoes_exp(exp, container, ofensas),
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

/// `Loc` de um statement — todo braço de `ast::Stat` tem `loc` como primeiro
/// campo; usado por `touch_loc` (T50) para aproximar o fim de um bloco.
fn stat_loc(stat: &Stat) -> Loc {
    match stat {
        Stat::StatBlock { loc, .. }
        | Stat::StatWhile { loc, .. }
        | Stat::StatRepeat { loc, .. }
        | Stat::StatIf { loc, .. }
        | Stat::StatFor { loc, .. }
        | Stat::StatForIn { loc, .. }
        | Stat::StatAssign { loc, .. }
        | Stat::StatDecl { loc, .. }
        | Stat::StatCall { loc, .. }
        | Stat::StatReturn { loc, .. }
        | Stat::StatBreak { loc, .. }
        | Stat::StatContinue { loc, .. } => *loc,
    }
}

/// `Loc` de um alvo de atribuição (T71) — o que a mensagem de "mutou o
/// container durante a iteração" aponta.
fn var_loc(var: &Var) -> Loc {
    match var {
        Var::VarName { loc, .. } | Var::VarBracket { loc, .. } | Var::VarDot { loc, .. } => *loc,
    }
}

/// `Loc` de uma expressão — mesma ideia de [`stat_loc`], para `ast::Exp`.
fn exp_loc(exp: &Exp) -> Loc {
    match exp {
        Exp::ExpNil { loc }
        | Exp::ExpBool { loc, .. }
        | Exp::ExpInteger { loc, .. }
        | Exp::ExpFloat { loc, .. }
        | Exp::ExpString { loc, .. }
        | Exp::ExpInitList { loc, .. }
        | Exp::ExpCall { loc, .. }
        | Exp::ExpVar { loc, .. }
        | Exp::ExpUnop { loc, .. }
        | Exp::ExpConcat { loc, .. }
        | Exp::ExpBinop { loc, .. }
        | Exp::ExpCast { loc, .. }
        | Exp::ExpAdjust { loc, .. }
        | Exp::ExpExtra { loc, .. } => *loc,
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
        // Nome nu, como `Record` (T74): a mensagem de erro fala do tipo
        // `Exp`, não da lista de variantes que o usuário já escreveu.
        Type::Sum { name, .. } => name.clone(),
        Type::Option { base } => format!("{}?", type_name(base)),
        Type::Opaque { module, name, .. } => format!("{}.{}", module, name),
    }
}

/// Roda as duas passadas sobre `program`, devolvendo o `Checker` já
/// preenchido (erros, `uses`, `scopes` e a AST tipada parcial) — [`check`] e
/// [`check_partial`] são as duas formas de consumir esse resultado.
fn run(program: &Program) -> (Checker, TypedProgram) {
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

    // Snapshot do escopo global (T50) — funções top-level, módulos
    // importados e `BUILTINS`, sem `close_block` correspondente porque este
    // bloco nunca fecha de verdade (vive por todo o arquivo).
    let global_scope = ScopeSnapshot {
        start: Loc { line: 1, col: 1 },
        end: checker.last_loc,
        symbols: checker
            .st
            .visible_symbols()
            .into_iter()
            .map(|(name, symbol)| ScopedSymbol {
                module: match &symbol.kind {
                    SymbolKind::Module { name } => Some(name.clone()),
                    _ => None,
                },
                type_name: type_name(&symbol.ty),
                name,
            })
            .collect(),
    };
    checker.scopes.push(global_scope);

    (checker, typed_program)
}

/// Verifica o programa por completo, produzindo a AST tipada (mais o índice
/// de usos da T49, em [`CheckedProgram::uses`]) em caso de sucesso.
///
/// Nunca panic: qualquer construção fora do subconjunto suportado, ou erro de
/// tipo, vira uma entrada em `Err`.
pub fn check(program: &Program) -> Result<CheckedProgram, Vec<CheckError>> {
    let (checker, typed_program) = run(program);

    if checker.errors.is_empty() {
        Ok(CheckedProgram {
            program: typed_program,
            uses: checker.uses,
            scopes: checker.scopes,
        })
    } else {
        Err(checker.errors)
    }
}

/// Mesma análise de [`check`], mas devolve os índices colaterais (`uses`,
/// `scopes`) **mesmo quando o programa tem erro de tipo** — o autocomplete
/// de membro (T50) precisa resolver o tipo de um receptor (`df` em `df.`)
/// justamente quando o buffer remendado ainda não tipa por completo (ex.:
/// `data.titan_lsp_cursor()` não existe no módulo, um erro esperado). Não
/// tem uso para `TypedProgram` nem para os erros — quem chama já sabe que o
/// buffer é sintético e não vai reportá-los.
pub fn check_partial(program: &Program) -> CheckedProgram {
    let (checker, typed_program) = run(program);
    CheckedProgram {
        program: typed_program,
        uses: checker.uses,
        scopes: checker.scopes,
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
    fn type_name_de_enum_e_o_nome_nu() {
        // T74: a mensagem de erro fala de `Exp`, não da lista de variantes —
        // e não desce na recursão, que aqui já está fechada em `ExpBinop`.
        let exp = Type::Sum {
            name: "Exp".to_string(),
            variants: vec![
                ("ExpNil".to_string(), vec![]),
                ("ExpInteger".to_string(), vec![Type::Integer]),
            ],
        };
        assert_eq!(type_name(&exp), "Exp");

        // E compostos sobre um `enum` se descrevem normalmente.
        assert_eq!(
            type_name(&Type::Array {
                elem: Box::new(exp.clone())
            }),
            "{Exp}"
        );
        assert_eq!(
            type_name(&Type::Option {
                base: Box::new(exp)
            }),
            "Exp?"
        );
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
    fn atribuicao_multipla_montada_a_mao_tipa_desde_a_t67() {
        // Este teste nasceu (T12) como prova da rejeição defensiva de
        // multi-assign, montando a AST à mão porque o parser de então nunca
        // a produzia. Na T67 o parser passou a produzi-la e o checker a
        // aceitá-la — a AST montada à mão continua útil como prova de que a
        // aceitação vale para o **nó**, não só para a grafia que o parser
        // gera. `a` e `b` são declarados antes, como o fonte exigiria.
        let loc = Loc { line: 1, col: 1 };
        let decl = |nome: &str, valor: i64| Stat::StatDecl {
            loc,
            decls: vec![Decl {
                loc,
                name: nome.to_string(),
                r#type: Some(ast::Type::TypeInteger { loc }),
                option: false,
            }],
            exps: vec![Exp::ExpInteger { loc, value: valor }],
        };
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
                    decl("a", 0),
                    decl("b", 0),
                    Stat::StatAssign {
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
                    },
                    Stat::StatReturn {
                        loc,
                        exps: vec![Exp::ExpInteger { loc, value: 0 }],
                    },
                ],
            },
        }];

        let typed = check(&program).expect("multi-assign tipa desde a T67");
        let TypedTopLevel::Func { body, .. } = &typed.program[0] else {
            panic!("esperava a função `main`");
        };
        let TypedStat::Block { stats, .. } = body.as_ref() else {
            panic!("esperava um bloco");
        };
        assert!(matches!(stats[2], TypedStat::AssignMulti { .. }));
        // As duas declarações atingidas pela atribuição saem mutáveis.
        for (i, stat) in stats.iter().take(2).enumerate() {
            let TypedStat::Decl { mutable, .. } = stat else {
                panic!("esperava uma declaração");
            };
            assert!(mutable, "a declaração {i} devia sair mutável");
        }
    }

    // ---- T73: `foreign function` ------------------------------------------

    /// Junta as mensagens de erro num texto só, para os `assert!(contains)`
    /// abaixo não dependerem de qual erro saiu primeiro.
    fn mensagens(errs: &[CheckError]) -> String {
        errs.iter()
            .map(|e| e.message.clone())
            .collect::<Vec<_>>()
            .join(" | ")
    }

    #[test]
    fn foreign_function_registra_o_simbolo_e_a_chamada_tipa() {
        let source = "foreign function abs(n: integer): integer\n\n\
                      function main(args: {string}): integer\n\
                      \x20   return abs(-7)\n\
                      end";
        let typed = check_source(source)
            .unwrap_or_else(|errs| panic!("esperava sucesso, obteve: {}", mensagens(&errs)));

        // A declaração externa sobrevive até a AST tipada — é dela que o
        // codegen tira o bloco `extern "C"`.
        let foreign = typed
            .iter()
            .find_map(|t| match t {
                TypedTopLevel::ForeignFunc {
                    name,
                    params,
                    rettypes,
                    ..
                } => Some((name, params, rettypes)),
                _ => None,
            })
            .expect("esperava um TypedTopLevel::ForeignFunc");
        assert_eq!(foreign.0, "abs");
        assert_eq!(foreign.1, &vec![("n".to_string(), Type::Integer)]);
        assert_eq!(foreign.2, &vec![Type::Integer]);
    }

    #[test]
    fn chamada_a_foreign_function_produz_callee_foreign() {
        // O que separa `Callee::Foreign` de `Callee::Direct` é só a emissão
        // (`unsafe`, sem mangling) — mas a distinção tem de chegar ao
        // codegen, e é isso que este teste fixa.
        let source = "foreign function abs(n: integer): integer\n\n\
                      function main(args: {string}): integer\n\
                      \x20   return abs(-7)\n\
                      end";
        let typed = check_source(source)
            .unwrap_or_else(|errs| panic!("esperava sucesso, obteve: {}", mensagens(&errs)));

        let mut achou = false;
        for top in &typed {
            let TypedTopLevel::Func { body, .. } = top else {
                continue;
            };
            let TypedStat::Block { stats, .. } = body.as_ref() else {
                continue;
            };
            for stat in stats {
                let TypedStat::Return { exps, .. } = stat else {
                    continue;
                };
                let TypedExpKind::Call { callee, .. } = &exps[0].kind else {
                    continue;
                };
                assert_eq!(*callee, Callee::Foreign("abs".to_string()));
                achou = true;
            }
        }
        assert!(achou, "esperava encontrar a chamada a `abs` no corpo de main");
    }

    #[test]
    fn foreign_function_sem_retorno_e_aceita() {
        // Retorno omitido é `nil`, o `void` do C — a única posição em que
        // `nil` atravessa a fronteira.
        let source = "foreign function sync()\n\n\
                      function main(args: {string}): integer\n\
                      \x20   sync()\n\
                      \x20   return 0\n\
                      end";
        check_source(source)
            .unwrap_or_else(|errs| panic!("esperava sucesso, obteve: {}", mensagens(&errs)));
    }

    #[test]
    fn foreign_function_com_string_na_fronteira_e_aceita() {
        let source = "foreign function strlen(s: string): integer\n\n\
                      function main(args: {string}): integer\n\
                      \x20   return strlen(\"abc\")\n\
                      end";
        check_source(source)
            .unwrap_or_else(|errs| panic!("esperava sucesso, obteve: {}", mensagens(&errs)));
    }

    #[test]
    fn foreign_function_com_array_na_fronteira_da_erro_claro() {
        let source = "foreign function soma(xs: {integer}): integer\n\n\
                      function main(args: {string}): integer\n\
                      \x20   return 0\n\
                      end";
        let errs = check_source(source).unwrap_err();
        let msg = mensagens(&errs);
        assert!(
            msg.contains("fronteira de FFI") && msg.contains("{integer}"),
            "mensagem devia citar a fronteira e o tipo recusado: {msg}"
        );
    }

    #[test]
    fn foreign_function_com_record_na_fronteira_da_erro_claro() {
        // O caso do critério de aceite: "tipo composto na fronteira dá erro
        // claro". `record` é o composto mais tentador, porque em C existe
        // `struct` — mas o layout do Rust não é o do C sem `#[repr(C)]`.
        let source = "record Ponto\n\x20   x: integer\n\x20   y: integer\nend\n\n\
                      foreign function dist(p: Ponto): float\n\n\
                      function main(args: {string}): integer\n\
                      \x20   return 0\n\
                      end";
        let errs = check_source(source).unwrap_err();
        let msg = mensagens(&errs);
        assert!(
            msg.contains("fronteira de FFI") && msg.contains("Ponto"),
            "mensagem devia citar a fronteira e o record recusado: {msg}"
        );
    }

    #[test]
    fn foreign_function_com_retorno_composto_da_erro_claro() {
        let source = "foreign function nomes(): {string}\n\n\
                      function main(args: {string}): integer\n\
                      \x20   return 0\n\
                      end";
        let errs = check_source(source).unwrap_err();
        let msg = mensagens(&errs);
        assert!(
            msg.contains("fronteira de FFI") && msg.contains("o retorno"),
            "mensagem devia citar o retorno: {msg}"
        );
    }

    #[test]
    fn foreign_function_com_value_na_fronteira_da_erro_claro() {
        // `value` compila e tem representação (T70), mas é um enum boxado do
        // runtime — nenhuma função C sabe lê-lo.
        let source = "foreign function f(v: value): integer\n\n\
                      function main(args: {string}): integer\n\
                      \x20   return 0\n\
                      end";
        let errs = check_source(source).unwrap_err();
        assert!(mensagens(&errs).contains("fronteira de FFI"));
    }

    #[test]
    fn foreign_function_com_opcional_na_fronteira_da_erro_claro() {
        let source = "foreign function f(n: integer?): integer\n\n\
                      function main(args: {string}): integer\n\
                      \x20   return 0\n\
                      end";
        let errs = check_source(source).unwrap_err();
        assert!(mensagens(&errs).contains("fronteira de FFI"));
    }

    #[test]
    fn foreign_function_com_dois_retornos_da_erro_claro() {
        // A ABI C devolve um valor só; retorno múltiplo (T66) para no
        // checker, não no rustc.
        let source = "foreign function divmod(a: integer, b: integer): integer, integer\n\n\
                      function main(args: {string}): integer\n\
                      \x20   return 0\n\
                      end";
        let errs = check_source(source).unwrap_err();
        assert!(
            mensagens(&errs).contains("não pode ter mais de um retorno"),
            "obteve: {}",
            mensagens(&errs)
        );
    }

    #[test]
    fn foreign_function_acumula_os_erros_de_fronteira() {
        // Dois parâmetros inválidos devem render dois erros, não parar no
        // primeiro — é o que o resto do checker faz.
        let source = "foreign function f(xs: {integer}, ys: {string}): integer\n\n\
                      function main(args: {string}): integer\n\
                      \x20   return 0\n\
                      end";
        let errs = check_source(source).unwrap_err();
        let fronteira = errs
            .iter()
            .filter(|e| e.message.contains("fronteira de FFI"))
            .count();
        assert_eq!(fronteira, 2, "obteve: {}", mensagens(&errs));
    }

    #[test]
    fn foreign_function_colidindo_com_funcao_titan_da_erro_claro() {
        let source = "foreign function abs(n: integer): integer\n\n\
                      function abs(n: integer): integer\n\
                      \x20   return n\n\
                      end\n\n\
                      function main(args: {string}): integer\n\
                      \x20   return 0\n\
                      end";
        let errs = check_source(source).unwrap_err();
        assert!(mensagens(&errs).contains("já foi declarado antes"));
    }

    #[test]
    fn chamada_a_foreign_function_com_argumento_de_tipo_errado_da_erro_claro() {
        // A fronteira não relaxa a tipagem: os argumentos são checados
        // contra a assinatura como os de qualquer função Titan.
        let source = "foreign function abs(n: integer): integer\n\n\
                      function main(args: {string}): integer\n\
                      \x20   return abs(\"x\")\n\
                      end";
        let errs = check_source(source).unwrap_err();
        assert!(!errs.is_empty(), "esperava erro de tipo no argumento");
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

    // ---- T72: `import` com alias e `df:metodo()` --------------------------

    /// O alias entra na symtab e resolve tipo qualificado e chamada de
    /// módulo pelo nome local — mas o `Type::Opaque` e o `Callee::Module`
    /// carregam o nome **real** do módulo, que é o que o codegen resolve.
    #[test]
    fn import_com_alias_resolve_tipo_e_chamada_pelo_nome_local() {
        let source = r#"import data as d

function main(args: {string}): integer
    local df: d.DataFrame = d.read_csv("v.csv")
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

    /// Com alias, o nome do módulo **real** deixa de estar em escopo — quem
    /// escreveu `as d` escolheu `d`.
    #[test]
    fn import_com_alias_nao_deixa_o_nome_do_modulo_em_escopo() {
        let source = r#"import data as d

function main(args: {string}): integer
    local df: data.DataFrame = data.read_csv("v.csv")
    return 0
end"#;
        let errs = check_source(source).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("módulo 'data' não foi importado"))
        );
    }

    /// Mensagem de erro de membro inexistente cita o nome **local**, que é
    /// o que o programa escreveu.
    #[test]
    fn funcao_inexistente_sob_alias_cita_o_nome_local() {
        let source = r#"import data as d

function main(args: {string}): integer
    local df: d.DataFrame = d.foo("v.csv")
    return 0
end"#;
        let errs = check_source(source).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("o módulo 'd' não tem função 'foo'")),
            "obteve: {:?}",
            errs.iter().map(|e| e.to_string()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn alias_colidindo_com_nome_ja_declarado_produz_erro_claro() {
        let source = r#"import data as soma

function soma(): integer
    return 0
end

function main(args: {string}): integer
    return 0
end"#;
        let errs = check_source(source).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("'soma' já foi declarado antes")),
            "obteve: {:?}",
            errs.iter().map(|e| e.to_string()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn dois_aliases_para_o_mesmo_modulo_convivem() {
        let source = r#"import data as a
import data as b

function main(args: {string}): integer
    local df: a.DataFrame = b.read_csv("v.csv")
    return 0
end"#;
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

    /// O critério central da T72: `df:soma(...)` produz **o mesmo**
    /// `TypedExp` que `df.soma(...)` — mesmo `Callee::Method`, mesmo tipo.
    #[test]
    fn metodo_com_dois_pontos_produz_o_mesmo_typedexp_que_com_ponto() {
        let com_ponto = r#"import data

function main(args: {string}): integer
    local df: data.DataFrame = data.read_csv("v.csv")
    local total: float = df.soma("valor")
    return 0
end"#;
        let com_dois_pontos = com_ponto.replace("df.soma", "df:soma");

        let typed_ponto = check_source(com_ponto).expect("forma com ponto deve tipar");
        let typed_dois = check_source(&com_dois_pontos).expect("forma com dois-pontos deve tipar");

        let valor = |typed: &[TypedTopLevel]| {
            let TypedTopLevel::Func { body, .. } = &typed[0] else {
                panic!("esperava TypedTopLevel::Func");
            };
            let TypedStat::Block { stats, .. } = body.as_ref() else {
                panic!("esperava TypedStat::Block");
            };
            let TypedStat::Decl { value, .. } = &stats[1] else {
                panic!("esperava TypedStat::Decl");
            };
            value.clone()
        };

        let (ponto, dois) = (valor(&typed_ponto), valor(&typed_dois));
        // `loc` difere de propósito — as duas formas escrevem a chamada em
        // colunas diferentes (`(` versus `:`), e é isso que o LSP deve
        // apontar em cada uma. O que a T72 exige idêntico é o resto: mesmo
        // tipo e mesmo `Callee::Method` com o mesmo receptor e argumentos.
        assert_eq!(ponto.ty, dois.ty);
        assert_eq!(ponto.kind, dois.kind);
    }

    #[test]
    fn metodo_com_dois_pontos_sobre_nao_opaco_produz_erro_claro() {
        let source = r#"function main(args: {string}): integer
    local x: integer = 1
    local y: integer = x:soma(2)
    return 0
end"#;
        let errs = check_source(source).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("só é possível chamar um nome de função")),
            "obteve: {:?}",
            errs.iter().map(|e| e.to_string()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn metodo_inexistente_com_dois_pontos_produz_o_mesmo_erro_que_com_ponto() {
        let source = r#"import data

function main(args: {string}): integer
    local df: data.DataFrame = data.read_csv("v.csv")
    local total: float = df:foo("valor")
    return 0
end"#;
        let errs = check_source(source).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("'data.DataFrame' não tem método 'foo'"))
        );
    }

    /// As duas formas convivem inclusive sob alias.
    #[test]
    fn dois_pontos_funciona_sob_alias() {
        let source = r#"import data as d

function main(args: {string}): integer
    local df: d.DataFrame = d.read_csv("v.csv")
    local total: float = df:soma("valor")
    return 0
end"#;
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
        // AST montada à mão para exercitar o braço defensivo `_` da
        // conversão String → BinOp/UnOp, que só é alcançável assim: o
        // parser nunca produz estas grafias. `#` deixou de ser exemplo aqui
        // na T29 e `//`/`~` na T61 — os três passaram a ser suportados —,
        // então o que resta são grafias inventadas.
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
                            op: "<=>".to_string(),
                            rhs: Box::new(Exp::ExpInteger { loc, value: 2 }),
                        },
                        Exp::ExpUnop {
                            loc,
                            op: "++".to_string(),
                            exp: Box::new(Exp::ExpInteger { loc, value: 1 }),
                        },
                    ],
                }],
            },
        }];

        let errs = check(&program).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("operador `<=>` não é suportado"))
        );
        assert!(
            errs.iter()
                .any(|e| e.message.contains("operador unário `++` não é suportado"))
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

    // ---- T71: `for`-in sobre array e map --------------------------------

    /// Monta um `main` com `corpo` no meio — o formato de quase todo caso
    /// desta seção, onde só o corpo do laço muda.
    fn fonte_main(corpo: &str) -> String {
        format!("function main(args: {{string}}): integer\n{corpo}\n    return 0\nend")
    }

    fn erros_de(corpo: &str) -> Vec<CheckError> {
        check_source(&fonte_main(corpo)).unwrap_err()
    }

    fn contem_erro(corpo: &str, trecho: &str) {
        let errs = erros_de(corpo);
        assert!(
            errs.iter().any(|e| e.message.contains(trecho)),
            "esperava um erro contendo {trecho:?}, obtive: {}",
            errs.iter()
                .map(|e| e.message.clone())
                .collect::<Vec<_>>()
                .join("; ")
        );
    }

    /// O tipo do elemento vem do container, sem anotação nenhuma no fonte.
    #[test]
    fn for_in_infere_o_tipo_do_elemento_do_array() {
        let source = fonte_main(
            "    local v: {integer} = {1, 2}\n\
             \x20   local s: integer = 0\n\
             \x20   for x in v do\n\
             \x20       s = s + x\n\
             \x20   end",
        );
        check_source(&source).unwrap_or_else(|errs| {
            panic!(
                "esperava sucesso, obteve: {}",
                errs.iter()
                    .map(|e| e.message.clone())
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        });
    }

    /// Sobre um map, os dois nomes recebem tipos **diferentes** — chave e
    /// valor —, e é a ordem das declarações que decide qual é qual.
    #[test]
    fn for_in_liga_chave_e_valor_com_os_tipos_do_map() {
        let source = fonte_main(
            "    local m: {string: integer} = {[\"a\"] = 1}\n\
             \x20   local s: integer = 0\n\
             \x20   local t: string = \"\"\n\
             \x20   for k, n in m do\n\
             \x20       s = s + n\n\
             \x20       t = k\n\
             \x20   end",
        );
        check_source(&source).unwrap_or_else(|errs| {
            panic!(
                "esperava sucesso, obteve: {}",
                errs.iter()
                    .map(|e| e.message.clone())
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        });
    }

    /// As variáveis do laço não vazam para fora dele — mesmo escopo do `for`
    /// numérico.
    #[test]
    fn variavel_do_for_in_nao_vaza_para_fora_do_laco() {
        contem_erro(
            "    local v: {integer} = {1}\n\
             \x20   for x in v do\n\
             \x20   end\n\
             \x20   local y: integer = x",
            "'x' não foi declarado",
        );
    }

    /// `for v in v do` enxerga o `v` **de fora**: o container é tipado antes
    /// de qualquer nome do laço entrar em escopo.
    #[test]
    fn container_e_tipado_antes_de_a_variavel_entrar_em_escopo() {
        let source = fonte_main(
            "    local v: {integer} = {1}\n\
             \x20   local s: integer = 0\n\
             \x20   for v in v do\n\
             \x20       s = s + v\n\
             \x20   end",
        );
        check_source(&source).unwrap_or_else(|errs| {
            panic!(
                "esperava sucesso, obteve: {}",
                errs.iter()
                    .map(|e| e.message.clone())
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        });
    }

    #[test]
    fn for_in_sobre_array_com_dois_nomes_produz_erro() {
        contem_erro(
            "    local v: {integer} = {1}\n\
             \x20   for k, x in v do\n\
             \x20   end",
            "liga um nome (o elemento)",
        );
    }

    #[test]
    fn for_in_sobre_map_com_um_nome_produz_erro() {
        contem_erro(
            "    local m: {string: integer} = {[\"a\"] = 1}\n\
             \x20   for x in m do\n\
             \x20   end",
            "liga dois nomes (chave e valor)",
        );
    }

    #[test]
    fn for_in_sobre_escalar_produz_erro() {
        contem_erro(
            "    local n: integer = 3\n\
             \x20   for x in n do\n\
             \x20   end",
            "itera sobre array (`{T}`) ou map (`{K: V}`), encontrado integer",
        );
    }

    /// Anotar a variável com outro tipo é um engano sobre o que o container
    /// contém — e não há coerção aqui, nem a de `integer`→`float`.
    #[test]
    fn anotacao_divergente_na_variavel_do_for_in_produz_erro() {
        contem_erro(
            "    local v: {integer} = {1}\n\
             \x20   for x: float in v do\n\
             \x20   end",
            "o elemento iterado tem tipo integer, mas 'x' foi declarado como float",
        );
    }

    /// `for k, k in m do` liga os dois nomes pelo **mesmo** padrão do `for`
    /// do Rust, onde repetir um nome é `identifier bound more than once` —
    /// erro do `rustc`, em inglês. Recusado aqui antes disso.
    #[test]
    fn nomes_repetidos_no_for_in_produzem_erro() {
        contem_erro(
            "    local m: {string: integer} = {[\"a\"] = 1}\n\
             \x20   for k, k in m do\n\
             \x20   end",
            "'k' aparece duas vezes nas variáveis do `for`-in",
        );
    }

    #[test]
    fn variavel_opcional_no_for_in_produz_erro() {
        contem_erro(
            "    local v: {integer} = {1}\n\
             \x20   for x? in v do\n\
             \x20   end",
            "não pode ser opcional",
        );
    }

    /// Iterar um `{T}?` exige o container presente (T68).
    #[test]
    fn for_in_sobre_opcional_produz_erro_que_ensina_o_teste() {
        let errs = erros_de(
            "    local v: {integer}? = nil\n\
             \x20   for x in v do\n\
             \x20   end",
        );
        assert!(!errs.is_empty(), "esperava erro ao iterar sobre opcional");
    }

    /// `break`/`continue` (T63) contam o `for`-in como laço: `loop_depth`
    /// sobe pelo corpo como em qualquer outro.
    #[test]
    fn break_e_continue_valem_dentro_do_for_in() {
        let source = fonte_main(
            "    local v: {integer} = {1, 2}\n\
             \x20   for x in v do\n\
             \x20       if x == 1 then\n\
             \x20           continue\n\
             \x20       end\n\
             \x20       break\n\
             \x20   end",
        );
        check_source(&source).unwrap_or_else(|errs| {
            panic!(
                "esperava sucesso, obteve: {}",
                errs.iter()
                    .map(|e| e.message.clone())
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        });
    }

    /// Iterar sobre uma **chamada** não passa pela checagem de mutação: o
    /// temporário não tem nome que o corpo possa alcançar.
    #[test]
    fn iterar_sobre_chamada_permite_mutar_outros_containers() {
        let source = "function nums(): {integer}\n\
             \x20   return {1, 2}\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   local v: {integer} = {9}\n\
             \x20   for x in nums() do\n\
             \x20       v[1] = x\n\
             \x20   end\n\
             \x20   return 0\n\
             end";
        check_source(source).unwrap_or_else(|errs| {
            panic!(
                "esperava sucesso, obteve: {}",
                errs.iter()
                    .map(|e| e.message.clone())
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        });
    }

    /// Sombrear o nome do container é aceito — declarar não é mutar. Fixa o
    /// lado permissivo da varredura sem escopo (ADR 0024).
    #[test]
    fn sombrear_o_nome_do_container_e_permitido() {
        let source = fonte_main(
            "    local v: {integer} = {1, 2}\n\
             \x20   for x in v do\n\
             \x20       local v: integer = x\n\
             \x20       print(\"v: \" .. v)\n\
             \x20   end",
        );
        check_source(&source).unwrap_or_else(|errs| {
            panic!(
                "esperava sucesso, obteve: {}",
                errs.iter()
                    .map(|e| e.message.clone())
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        });
    }

    /// E o lado conservador do mesmo desenho: escrever no `v` **sombreado**
    /// é recusado como se fosse o container, porque a varredura roda antes da
    /// tipagem e não tem escopo. Documentado no ADR 0024 — o teste existe
    /// para que a troca por uma varredura com escopo seja uma mudança
    /// **visível**, e não um efeito colateral silencioso.
    #[test]
    fn escrever_no_container_sombreado_e_recusado_conservadoramente() {
        contem_erro(
            "    local v: {integer} = {1, 2}\n\
             \x20   for x in v do\n\
             \x20       local v: {integer} = {9}\n\
             \x20       v[1] = x\n\
             \x20   end",
            "não é possível modificar 'v' dentro do `for`-in",
        );
    }

    /// Mutar **outro** container dentro do laço é legítimo — a checagem é
    /// sobre o container iterado, não sobre escrita em geral.
    #[test]
    fn mutar_outro_container_dentro_do_for_in_e_permitido() {
        let source = fonte_main(
            "    local v: {integer} = {1, 2}\n\
             \x20   local w: {integer} = {0, 0}\n\
             \x20   for x in v do\n\
             \x20       w[1] = x\n\
             \x20   end",
        );
        check_source(&source).unwrap_or_else(|errs| {
            panic!(
                "esperava sucesso, obteve: {}",
                errs.iter()
                    .map(|e| e.message.clone())
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

    // ---- T61: tipos de bitwise e `//` -----------------------------------

    /// Tipo da expressão do primeiro `local` do corpo de `main` — atalho
    /// para afirmar sobre o resultado de um operador sem desmontar a AST
    /// tipada inteira.
    fn tipo_do_primeiro_local(source: &str) -> Type {
        let stats = typed_body_stats(source);
        let TypedStat::Decl { value, .. } = &stats[0] else {
            panic!("esperava TypedStat::Decl, obteve {:?}", stats[0]);
        };
        value.ty.clone()
    }

    fn fonte_com_local(exp: &str) -> String {
        format!("function main(args: {{string}}): integer\n    local a = {exp}\n    return 0\nend")
    }

    #[test]
    fn bitwise_entre_inteiros_resulta_integer() {
        for exp in ["1 & 2", "1 | 2", "1 ~ 2", "1 << 2", "1 >> 2", "~1"] {
            assert_eq!(
                tipo_do_primeiro_local(&fonte_com_local(exp)),
                Type::Integer,
                "`{exp}` deveria resultar integer"
            );
        }
    }

    /// Divergência deliberada de `checker.lua:1097-1109`, que coage float
    /// para integer: aqui o erro chega em português, em vez de truncar em
    /// silêncio (PRD.md, T61).
    #[test]
    fn bitwise_com_float_produz_erro_em_portugues() {
        for exp in ["1.5 & 2", "2 | 1.5", "1.5 ~ 2", "1.5 << 2", "1 >> 1.5"] {
            let errs = check_source(&fonte_com_local(exp)).unwrap_err();
            assert!(
                errs.iter()
                    .any(|e| e.message.contains("precisa ser integer")),
                "`{exp}` deveria acusar operando não-integer, obteve {errs:?}"
            );
        }
    }

    #[test]
    fn bitwise_not_unario_com_float_produz_erro() {
        let errs = check_source(&fonte_com_local("~1.5")).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("`~` precisa ser integer"))
        );
    }

    #[test]
    fn bitwise_com_string_produz_erro() {
        let errs = check_source(&fonte_com_local("\"a\" & 1")).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("precisa ser integer"))
        );
    }

    #[test]
    fn divisao_inteira_segue_a_regra_aritmetica_dos_demais() {
        assert_eq!(
            tipo_do_primeiro_local(&fonte_com_local("7 // 2")),
            Type::Integer
        );
        for exp in ["7.0 // 2", "7 // 2.0", "7.0 // 2.0"] {
            assert_eq!(
                tipo_do_primeiro_local(&fonte_com_local(exp)),
                Type::Float,
                "`{exp}` deveria promover a float"
            );
        }
    }

    #[test]
    fn divisao_inteira_com_string_produz_erro_de_operando_numerico() {
        let errs = check_source(&fonte_com_local("\"a\" // 2")).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("precisa ser numérico"))
        );
    }

    /// `~` desambigua pelo número de operandos: binário é XOR, prefixo é NOT
    /// — os dois no mesmo programa, para provar que o checker não confunde.
    #[test]
    fn til_binario_e_unario_convivem_no_mesmo_programa() {
        let stats = typed_body_stats(
            "function main(args: {string}): integer\n\
             \x20   local x = 5 ~ 3\n\
             \x20   local y = ~0\n\
             \x20   return 0\n\
             end",
        );
        let TypedStat::Decl { value: x, .. } = &stats[0] else {
            panic!("esperava Decl");
        };
        assert!(matches!(
            x.kind,
            TypedExpKind::Binop {
                op: BinOp::BXor,
                ..
            }
        ));
        let TypedStat::Decl { value: y, .. } = &stats[1] else {
            panic!("esperava Decl");
        };
        assert!(matches!(
            y.kind,
            TypedExpKind::Unop { op: UnOp::BNot, .. }
        ));
    }

    /// T63: dentro de laço, `continue` vira `TypedStat::Continue` — em
    /// `while` e em `for`, que é onde `loop_depth` é incrementado.
    #[test]
    fn continue_dentro_de_laco_e_aceito() {
        let stats = typed_body_stats(
            "function main(args: {string}): integer\n\
             \x20   while true do\n\
             \x20       continue\n\
             \x20   end\n\
             \x20   for i = 1, 3 do\n\
             \x20       continue\n\
             \x20   end\n\
             \x20   return 0\n\
             end",
        );
        let TypedStat::While { block, .. } = &stats[0] else {
            panic!("esperava While");
        };
        let TypedStat::Block { stats: corpo, .. } = block.as_ref() else {
            panic!("esperava Block");
        };
        assert!(matches!(corpo[0], TypedStat::Continue { .. }));
        let TypedStat::For { block, .. } = &stats[1] else {
            panic!("esperava For");
        };
        let TypedStat::Block { stats: corpo, .. } = block.as_ref() else {
            panic!("esperava Block");
        };
        assert!(matches!(corpo[0], TypedStat::Continue { .. }));
    }

    /// Fora de laço é erro claro em português, com a mesma forma da mensagem
    /// de `break` — a checagem é literalmente a mesma.
    #[test]
    fn continue_fora_de_laco_e_erro_claro() {
        let erros = check_source(
            "function main(args: {string}): integer\n\
             \x20   continue\n\
             \x20   return 0\n\
             end",
        )
        .expect_err("esperava erro");
        assert!(
            erros
                .iter()
                .any(|e| e.to_string().contains("`continue` fora de um laço")),
            "{erros:?}"
        );
    }

    /// T64: `repeat` deixou de ser rejeitado ("não é suportado nesta fase")
    /// e passou a produzir `TypedStat::Repeat`, com a condição tipada como
    /// `Boolean` — a mesma exigência do `while` (ADR 0005: sem truthy).
    #[test]
    fn repeat_produz_typed_repeat_com_condicao_boolean() {
        let stats = typed_body_stats(
            "function main(args: {string}): integer\n\
             \x20   local n: integer = 0\n\
             \x20   repeat\n\
             \x20       n = n + 1\n\
             \x20   until n > 3\n\
             \x20   return 0\n\
             end",
        );
        let TypedStat::Repeat {
            block, condition, ..
        } = &stats[1]
        else {
            panic!("esperava TypedStat::Repeat, obteve {:?}", stats[1]);
        };
        assert_eq!(condition.ty, Type::Boolean);
        let TypedStat::Block { stats: corpo, .. } = block.as_ref() else {
            panic!("esperava Block");
        };
        assert_eq!(corpo.len(), 1);
    }

    /// A armadilha da T64: em Lua a condição do `until` enxerga os `local`
    /// declarados no corpo, o que obriga o escopo do bloco a fechar
    /// **depois** de a condição ser tipada. Se `check_repeat` delegasse o
    /// corpo a `check_stat` (que fecha o escopo ao sair do `StatBlock`),
    /// este caso falharia com "nome não declarado".
    #[test]
    fn until_enxerga_local_declarado_no_corpo() {
        let stats = typed_body_stats(
            "function main(args: {string}): integer\n\
             \x20   local n: integer = 0\n\
             \x20   repeat\n\
             \x20       local x: integer = n * 2\n\
             \x20       n = n + 1\n\
             \x20   until x > 10\n\
             \x20   return 0\n\
             end",
        );
        assert!(matches!(stats[1], TypedStat::Repeat { .. }), "{stats:?}");
    }

    /// O outro lado da mesma moeda: o escopo **fecha**. O `local` do corpo
    /// não vaza para depois do laço, senão o `repeat` estaria declarando no
    /// escopo de fora.
    #[test]
    fn local_do_corpo_nao_vaza_para_depois_do_repeat() {
        let erros = check_source(
            "function main(args: {string}): integer\n\
             \x20   repeat\n\
             \x20       local x: integer = 1\n\
             \x20   until true\n\
             \x20   return x\n\
             end",
        )
        .expect_err("esperava erro");
        assert!(
            erros
                .iter()
                .any(|e| e.to_string().contains("'x' não foi declarado")),
            "{erros:?}"
        );
    }

    /// Condição não-boolean é erro claro em português, nomeando `until` —
    /// não `repeat` — porque é a palavra que o usuário escreveu antes dela.
    #[test]
    fn until_com_condicao_nao_boolean_e_erro_claro() {
        let erros = check_source(
            "function main(args: {string}): integer\n\
             \x20   repeat\n\
             \x20   until 1\n\
             \x20   return 0\n\
             end",
        )
        .expect_err("esperava erro");
        assert!(
            erros
                .iter()
                .any(|e| e.to_string().contains("condição do `until`")),
            "{erros:?}"
        );
    }

    /// O corpo do `repeat` é laço para efeito de `loop_depth`: `break` e
    /// `continue` dentro dele são aceitos sem caso especial (ADR 0023).
    #[test]
    fn break_e_continue_dentro_de_repeat_sao_aceitos() {
        let stats = typed_body_stats(
            "function main(args: {string}): integer\n\
             \x20   local n: integer = 0\n\
             \x20   repeat\n\
             \x20       n = n + 1\n\
             \x20       if n == 1 then\n\
             \x20           continue\n\
             \x20       end\n\
             \x20       break\n\
             \x20   until true\n\
             \x20   return 0\n\
             end",
        );
        let TypedStat::Repeat { block, .. } = &stats[1] else {
            panic!("esperava Repeat");
        };
        let TypedStat::Block { stats: corpo, .. } = block.as_ref() else {
            panic!("esperava Block");
        };
        assert!(matches!(corpo[2], TypedStat::Break { .. }), "{corpo:?}");
    }

    /// Depois do laço fechado `loop_depth` voltou a zero: um `continue` ali
    /// é erro, mesmo havendo um laço antes no mesmo corpo.
    #[test]
    fn continue_depois_do_laco_e_erro_claro() {
        let erros = check_source(
            "function main(args: {string}): integer\n\
             \x20   while false do\n\
             \x20   end\n\
             \x20   continue\n\
             \x20   return 0\n\
             end",
        )
        .expect_err("esperava erro");
        assert!(
            erros
                .iter()
                .any(|e| e.to_string().contains("`continue` fora de um laço")),
            "{erros:?}"
        );
    }

    // ---- T65: retornos múltiplos ----------------------------------------

    /// Os comandos do corpo da função de nome `name` — `typed_body_stats`
    /// assume a primeira função do programa, e os testes de retorno múltiplo
    /// precisam olhar a `main` de um programa que declara `divmod` antes.
    fn typed_stats_da_funcao(source: &str, name: &str) -> Vec<TypedStat> {
        let typed = check_source(source).unwrap_or_else(|errs| {
            panic!(
                "esperava sucesso, obteve erros: {}",
                errs.iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        });
        let body = typed
            .iter()
            .find_map(|top| match top {
                TypedTopLevel::Func { name: n, body, .. } if n == name => Some(body.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("esperava a função '{name}' no programa tipado"));
        let TypedStat::Block { stats, .. } = body.as_ref() else {
            panic!("esperava TypedStat::Block como corpo");
        };
        stats.clone()
    }

    const DIVMOD: &str = "function divmod(a: integer, b: integer): integer, integer\n\
                          \x20   return a // b, a % b\n\
                          end\n";

    /// O caso central da T65: uma assinatura com dois retornos tipa, e os
    /// dois tipos chegam ao programa tipado.
    #[test]
    fn funcao_com_dois_retornos_tipa() {
        let source = format!(
            "{DIVMOD}\
             function main(args: {{string}}): integer\n\
             \x20   return 0\n\
             end"
        );
        let typed = check_source(&source).unwrap_or_else(|errs| {
            panic!(
                "esperava sucesso, obteve erros: {}",
                errs.iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        });
        let TypedTopLevel::Func { name, rettypes, .. } = &typed[0] else {
            panic!("esperava TypedTopLevel::Func");
        };
        assert_eq!(name, "divmod");
        assert_eq!(rettypes, &vec![Type::Integer, Type::Integer]);
    }

    /// `return` com menos valores que a assinatura reusa, sem mudança, a
    /// checagem de aridade que já existia.
    #[test]
    fn return_com_aridade_menor_produz_erro_claro() {
        let source = "function divmod(a: integer, b: integer): integer, integer\n\
                      \x20   return a // b\n\
                      end\n\
                      function main(args: {string}): integer\n\
                      \x20   return 0\n\
                      end";
        let erros = check_source(source).expect_err("esperava erro");
        assert!(
            erros.iter().any(|e| e
                .to_string()
                .contains("retornou 1 valor(es), mas a função espera 2")),
            "{erros:?}"
        );
    }

    /// E com valores demais, idem.
    #[test]
    fn return_com_aridade_maior_produz_erro_claro() {
        let source = "function f(): integer\n\
                      \x20   return 1, 2\n\
                      end\n\
                      function main(args: {string}): integer\n\
                      \x20   return 0\n\
                      end";
        let erros = check_source(source).expect_err("esperava erro");
        assert!(
            erros.iter().any(|e| e
                .to_string()
                .contains("retornou 2 valor(es), mas a função espera 1")),
            "{erros:?}"
        );
    }

    /// Cada valor é conferido contra o tipo na sua posição, não só o
    /// primeiro.
    #[test]
    fn tipo_incompativel_no_segundo_retorno_produz_erro_claro() {
        let source = "function f(): integer, string\n\
                      \x20   return 1, 2\n\
                      end\n\
                      function main(args: {string}): integer\n\
                      \x20   return 0\n\
                      end";
        let erros = check_source(source).expect_err("esperava erro");
        assert!(
            erros.iter().any(|e| e
                .to_string()
                .contains("retorno incompatível: esperado string, encontrado integer")),
            "{erros:?}"
        );
    }

    /// Chamada de dois retornos em posição escalar ajusta para o primeiro
    /// valor: o tipo é o do primeiro retorno e o nó vira um `Adjust`.
    #[test]
    fn chamada_de_dois_retornos_em_posicao_escalar_ajusta_para_o_primeiro() {
        let source = format!(
            "{DIVMOD}\
             function main(args: {{string}}): integer\n\
             \x20   local q: integer = divmod(7, 2)\n\
             \x20   return q\n\
             end"
        );
        let stats = typed_stats_da_funcao(&source, "main");
        let TypedStat::Decl { ty, value, .. } = &stats[0] else {
            panic!("esperava Decl, obteve {:?}", stats[0]);
        };
        assert_eq!(ty, &Type::Integer);
        assert_eq!(value.ty, Type::Integer);
        let TypedExpKind::Adjust(inner) = &value.kind else {
            panic!("esperava Adjust, obteve {:?}", value.kind);
        };
        assert!(
            matches!(&inner.kind, TypedExpKind::Call { .. }),
            "{:?}",
            inner.kind
        );
    }

    /// Chamada de retorno único não ganha envelope nenhum — o caso comum
    /// continua exatamente como era.
    #[test]
    fn chamada_de_retorno_unico_nao_ganha_ajuste() {
        let source = "function f(): integer\n\
                      \x20   return 1\n\
                      end\n\
                      function main(args: {string}): integer\n\
                      \x20   local x: integer = f()\n\
                      \x20   return x\n\
                      end";
        let stats = typed_stats_da_funcao(source, "main");
        let TypedStat::Decl { value, .. } = &stats[0] else {
            panic!("esperava Decl");
        };
        assert!(
            matches!(&value.kind, TypedExpKind::Call { .. }),
            "{:?}",
            value.kind
        );
    }

    /// Chamada como comando descarta os dois retornos e não passa pelo
    /// ajuste — quem ajusta é a posição de expressão.
    #[test]
    fn chamada_de_dois_retornos_como_comando_nao_ganha_ajuste() {
        let source = format!(
            "{DIVMOD}\
             function main(args: {{string}}): integer\n\
             \x20   divmod(7, 2)\n\
             \x20   return 0\n\
             end"
        );
        let stats = typed_stats_da_funcao(&source, "main");
        let TypedStat::Call { call, .. } = &stats[0] else {
            panic!("esperava Call, obteve {:?}", stats[0]);
        };
        assert!(
            matches!(&call.kind, TypedExpKind::Call { .. }),
            "{:?}",
            call.kind
        );
    }

    /// O ajuste vale em qualquer posição de expressão, não só no `local`:
    /// dentro de um operador o valor usado é o primeiro retorno.
    #[test]
    fn ajuste_vale_dentro_de_expressao() {
        let source = format!(
            "{DIVMOD}\
             function main(args: {{string}}): integer\n\
             \x20   return divmod(7, 2) + 1\n\
             end"
        );
        let stats = typed_stats_da_funcao(&source, "main");
        let TypedStat::Return { exps, .. } = &stats[0] else {
            panic!("esperava Return");
        };
        let TypedExpKind::Binop { lhs, .. } = &exps[0].kind else {
            panic!("esperava Binop");
        };
        assert!(
            matches!(&lhs.kind, TypedExpKind::Adjust(_)),
            "{:?}",
            lhs.kind
        );
    }

    /// `ExpExtra` — o nó que a AST expõe desde a Fase 0 — deixa de ser
    /// rejeitado e passa a ser tipado pelo tipo do enésimo retorno.
    #[test]
    fn exp_extra_tipa_pelo_enesimo_retorno() {
        let source = "function f(): integer, string\n\
                      \x20   return 1, \"a\"\n\
                      end\n\
                      function main(args: {string}): integer\n\
                      \x20   return 0\n\
                      end";
        let typed = check_source(source).expect("esperava sucesso");
        assert_eq!(typed.len(), 2);

        // O parser não produz `ExpExtra` a partir do fonte; o nó é montado
        // aqui para exercitar o braço do checker.
        let loc = Loc { line: 1, col: 1 };
        let chamada = ast::Exp::ExpCall {
            loc,
            exp: Box::new(ast::Exp::ExpVar {
                loc,
                var: Box::new(ast::Var::VarName {
                    loc,
                    name: "f".to_string(),
                }),
            }),
            args: ast::Args::ArgsFunc { loc, args: vec![] },
        };
        let fonte = ast::Exp::ExpExtra {
            loc,
            exp: Box::new(chamada),
            index: 1,
            r#type: None,
        };

        let mut checker = Checker::new();
        checker.st.add_symbol(
            "f",
            Type::Function {
                params: vec![],
                rettypes: vec![Type::Integer, Type::String],
            },
            SymbolKind::Global,
            loc,
        );
        let typed_exp = checker.check_exp(&fonte, None).expect("esperava sucesso");
        assert_eq!(typed_exp.ty, Type::String);
        assert!(
            matches!(&typed_exp.kind, TypedExpKind::Extra { index: 1, .. }),
            "{:?}",
            typed_exp.kind
        );
    }

    /// Repassar uma chamada de dois retornos num `return` de dois retornos
    /// **não** expande a lista: a chamada em posição de expressão ajusta
    /// para o primeiro valor (a regra desta tarefa), e a aridade acusa a
    /// diferença com erro claro. A expansão do Lua fica fora do escopo da
    /// T65 — o que importa aqui é que o caso não passa em silêncio nem
    /// panica.
    #[test]
    fn repassar_chamada_de_dois_retornos_no_return_produz_erro_claro() {
        let source = format!(
            "{DIVMOD}\
             function repassa(a: integer, b: integer): integer, integer\n\
             \x20   return divmod(a, b)\n\
             end\n\
             function main(args: {{string}}): integer\n\
             \x20   return 0\n\
             end"
        );
        let erros = check_source(&source).expect_err("esperava erro");
        assert!(
            erros.iter().any(|e| e
                .to_string()
                .contains("retornou 1 valor(es), mas a função espera 2")),
            "{erros:?}"
        );
    }

    /// Índice além da assinatura é erro claro, não `panic` nem silêncio.
    #[test]
    fn exp_extra_com_indice_alem_da_assinatura_produz_erro_claro() {
        let loc = Loc { line: 1, col: 1 };
        let chamada = ast::Exp::ExpCall {
            loc,
            exp: Box::new(ast::Exp::ExpVar {
                loc,
                var: Box::new(ast::Var::VarName {
                    loc,
                    name: "f".to_string(),
                }),
            }),
            args: ast::Args::ArgsFunc { loc, args: vec![] },
        };
        let fonte = ast::Exp::ExpExtra {
            loc,
            exp: Box::new(chamada),
            index: 5,
            r#type: None,
        };

        let mut checker = Checker::new();
        checker.st.add_symbol(
            "f",
            Type::Function {
                params: vec![],
                rettypes: vec![Type::Integer, Type::String],
            },
            SymbolKind::Global,
            loc,
        );
        assert!(checker.check_exp(&fonte, None).is_none());
        assert!(
            checker
                .errors
                .iter()
                .any(|e| e.to_string().contains("valor(es) de retorno")),
            "{:?}",
            checker.errors
        );
    }
    // ---- T67: multi-assign e declaração múltipla -----------------------

    /// O caso central da tarefa: `local q, r = divmod(7, 2)` desestrutura a
    /// tupla da T66 — dois alvos, tipados pelos dois retornos da assinatura.
    #[test]
    fn declaracao_multipla_desestrutura_chamada_de_dois_retornos() {
        let source = format!(
            "{DIVMOD}\
             function main(args: {{string}}): integer\n\
             \x20   local q, r = divmod(7, 2)\n\
             \x20   return q + r\n\
             end"
        );
        let stats = typed_stats_da_funcao(&source, "main");
        let TypedStat::DeclMulti {
            targets, values, ..
        } = &stats[0]
        else {
            panic!("esperava DeclMulti, obteve {:?}", stats[0]);
        };
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].name, "q");
        assert_eq!(targets[0].ty, Type::Integer);
        assert_eq!(targets[1].name, "r");
        assert_eq!(targets[1].ty, Type::Integer);
        // Os dois alvos ganham `decl_id` distintos — é o que permite o
        // fix-up marcar um como `mut` sem marcar o outro.
        assert_ne!(targets[0].decl_id, targets[1].decl_id);
        // A chamada entra crua, sem `Adjust`: é a tupla inteira que é
        // desestruturada, não o primeiro valor.
        let TypedMultiValues::Call(call) = values else {
            panic!("esperava desestruturação de chamada, obteve {values:?}");
        };
        assert!(matches!(call.kind, TypedExpKind::Call { .. }));
    }

    /// A outra forma: uma expressão por alvo.
    #[test]
    fn declaracao_multipla_por_lista_de_valores() {
        let stats = typed_body_stats(
            "function main(args: {string}): integer\n\
             \x20   local a: integer, b: string = 1, \"x\"\n\
             \x20   return 0\n\
             end",
        );
        let TypedStat::DeclMulti {
            targets, values, ..
        } = &stats[0]
        else {
            panic!("esperava DeclMulti, obteve {:?}", stats[0]);
        };
        assert_eq!(targets[0].ty, Type::Integer);
        assert_eq!(targets[1].ty, Type::String);
        let TypedMultiValues::List(exps) = values else {
            panic!("esperava lista de valores, obteve {values:?}");
        };
        assert_eq!(exps.len(), 2);
    }

    /// Sem anotação, o tipo de cada alvo vem do valor correspondente — não
    /// do primeiro nem de uma unificação entre eles.
    #[test]
    fn declaracao_multipla_sem_anotacao_infere_cada_alvo_do_seu_valor() {
        let stats = typed_body_stats(
            "function main(args: {string}): integer\n\
             \x20   local a, b = 1, \"x\"\n\
             \x20   return 0\n\
             end",
        );
        let TypedStat::DeclMulti { targets, .. } = &stats[0] else {
            panic!("esperava DeclMulti, obteve {:?}", stats[0]);
        };
        assert_eq!(targets[0].ty, Type::Integer);
        assert_eq!(targets[1].ty, Type::String);
    }

    /// `a, b = b, a` tipa e produz dois alvos. A ordem de avaliação é
    /// responsabilidade do codegen; aqui o que importa é o nó.
    #[test]
    fn atribuicao_multipla_tipa_o_swap() {
        let stats = typed_body_stats(
            "function main(args: {string}): integer\n\
             \x20   local a: integer = 1\n\
             \x20   local b: integer = 2\n\
             \x20   a, b = b, a\n\
             \x20   return 0\n\
             end",
        );
        let TypedStat::AssignMulti { targets, .. } = &stats[2] else {
            panic!("esperava AssignMulti, obteve {:?}", stats[2]);
        };
        assert_eq!(
            targets,
            &vec![
                TypedLValue::Name("a".to_string()),
                TypedLValue::Name("b".to_string())
            ]
        );
    }

    /// A exigência explícita da tarefa: o fix-up marca **todos** os alvos
    /// da atribuição múltipla, não só o primeiro.
    #[test]
    fn atribuicao_multipla_marca_todos_os_alvos_como_mutaveis() {
        let stats = typed_body_stats(
            "function main(args: {string}): integer\n\
             \x20   local a: integer = 1\n\
             \x20   local b: integer = 2\n\
             \x20   local c: integer = 3\n\
             \x20   a, b = b, a\n\
             \x20   return 0\n\
             end",
        );
        for (i, esperado) in [true, true, false].iter().enumerate() {
            let TypedStat::Decl { mutable, name, .. } = &stats[i] else {
                panic!("esperava Decl, obteve {:?}", stats[i]);
            };
            assert_eq!(mutable, esperado, "mutabilidade errada em '{name}'");
        }
    }

    /// O mesmo para os alvos de uma **declaração** múltipla: quem é
    /// reatribuído depois sai mutável, quem não é não sai.
    #[test]
    fn declaracao_multipla_marca_so_o_alvo_reatribuido() {
        let stats = typed_body_stats(
            "function main(args: {string}): integer\n\
             \x20   local a: integer, b: integer = 1, 2\n\
             \x20   a = 3\n\
             \x20   return b\n\
             end",
        );
        let TypedStat::DeclMulti { targets, .. } = &stats[0] else {
            panic!("esperava DeclMulti, obteve {:?}", stats[0]);
        };
        assert!(targets[0].mutable, "'a' é reatribuído e devia sair mutável");
        assert!(!targets[1].mutable, "'b' nunca é reatribuído");
    }

    /// Aridade incompatível entre alvos e a chamada dá erro claro — Titan
    /// não preenche o que falta com `nil` como o Lua faz.
    #[test]
    fn aridade_incompativel_com_a_chamada_produz_erro_claro() {
        let source = format!(
            "{DIVMOD}\
             function main(args: {{string}}): integer\n\
             \x20   local a, b, c = divmod(7, 2)\n\
             \x20   return 0\n\
             end"
        );
        let errs = check_source(&source).unwrap_err();
        assert!(
            errs.iter().any(|e| e
                .message
                .contains("3 alvo(s), mas a chamada produz 2 valor(es)")),
            "{errs:?}"
        );
    }

    /// Aridade incompatível na forma de lista, nos dois sentidos.
    #[test]
    fn aridade_incompativel_na_lista_produz_erro_claro() {
        for (fonte, trecho) in [
            (
                "function main(args: {string}): integer\n\
                 \x20   local a: integer = 1\n\
                 \x20   local b: integer = 2\n\
                 \x20   a, b = 1\n\
                 \x20   return 0\n\
                 end",
                "2 alvo(s), mas 1 valor(es)",
            ),
            (
                "function main(args: {string}): integer\n\
                 \x20   local a, b = 1, 2, 3\n\
                 \x20   return 0\n\
                 end",
                "2 alvo(s), mas 3 valor(es)",
            ),
        ] {
            let errs = check_source(fonte).unwrap_err();
            assert!(
                errs.iter().any(|e| e.message.contains(trecho)),
                "esperava '{trecho}', obteve {errs:?}"
            );
        }
    }

    /// Tipo incompatível num alvo específico nomeia **qual** alvo é.
    #[test]
    fn tipo_incompativel_em_alvo_da_declaracao_multipla_nomeia_a_posicao() {
        let errs = check_source(
            "function main(args: {string}): integer\n\
             \x20   local a: integer, b: string = 1, 2\n\
             \x20   return 0\n\
             end",
        )
        .unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("2º alvo") && e.message.contains("esperado string")),
            "{errs:?}"
        );
    }

    /// As rejeições de alvo do single-target continuam valendo para cada
    /// alvo da lista — atribuir a uma função não passa a ser permitido por
    /// estar acompanhado.
    #[test]
    fn alvo_invalido_na_atribuicao_multipla_continua_rejeitado() {
        let errs = check_source(
            "function f(): integer\n\
             \x20   return 1\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   local a: integer = 1\n\
             \x20   a, f = 1, 2\n\
             \x20   return 0\n\
             end",
        )
        .unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("atribuir a uma função")),
            "{errs:?}"
        );
    }

    /// O lado direito de um `local` múltiplo enxerga o escopo de **fora**
    /// da declaração, como em Lua: `local x, y = y, x` lê os homônimos
    /// externos, não os que estão sendo declarados.
    #[test]
    fn lado_direito_do_local_multiplo_enxerga_o_escopo_externo() {
        let stats = typed_body_stats(
            "function main(args: {string}): integer\n\
             \x20   local x: integer = 1\n\
             \x20   local y: string = \"s\"\n\
             \x20   local x: string, y: integer = y, x\n\
             \x20   return 0\n\
             end",
        );
        let TypedStat::DeclMulti { targets, .. } = &stats[2] else {
            panic!("esperava DeclMulti, obteve {:?}", stats[2]);
        };
        assert_eq!(targets[0].ty, Type::String);
        assert_eq!(targets[1].ty, Type::Integer);
    }

    /// Uma chamada de retorno **único** não vira desestruturação: com dois
    /// alvos, a aridade não bate e o erro é claro.
    #[test]
    fn chamada_de_retorno_unico_nao_preenche_dois_alvos() {
        let errs = check_source(
            "function f(): integer\n\
             \x20   return 1\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   local a, b = f()\n\
             \x20   return 0\n\
             end",
        )
        .unwrap_err();
        assert!(
            errs.iter().any(|e| e
                .message
                .contains("2 alvo(s), mas a chamada produz 1 valor(es)")),
            "{errs:?}"
        );
    }

    // ---- T68: `Option`/`?` e narrowing de fluxo -------------------------

    /// Envolve `corpo` num `main` válido — o critério de aceite da T68 é
    /// todo sobre statements dentro de uma função.
    fn em_main(corpo: &str) -> String {
        format!("function main(args: {{string}}): integer\n{corpo}\n    return 0\nend")
    }

    #[test]
    fn t68_local_com_tipo_opcional_e_nil_tipa() {
        let stats = typed_body_stats(&em_main("    local x: integer? = nil"));
        let TypedStat::Decl { ty, value, .. } = &stats[0] else {
            panic!("esperava Decl, obteve {:?}", stats[0]);
        };
        assert_eq!(
            *ty,
            Type::Option {
                base: Box::new(Type::Integer)
            }
        );
        // `nil` no destino opcional continua sendo o literal (vira `None` na
        // T69), com o tipo do destino — não um `SomeOf`.
        assert_eq!(value.kind, TypedExpKind::Nil);
        assert_eq!(*ty, value.ty);
    }

    #[test]
    fn t68_valor_do_tipo_base_e_injetado_no_opcional() {
        let stats = typed_body_stats(&em_main("    local x: integer? = 10"));
        let TypedStat::Decl { ty, value, .. } = &stats[0] else {
            panic!("esperava Decl, obteve {:?}", stats[0]);
        };
        assert_eq!(
            *ty,
            Type::Option {
                base: Box::new(Type::Integer)
            }
        );
        let TypedExpKind::SomeOf(inner) = &value.kind else {
            panic!("esperava SomeOf, obteve {:?}", value.kind);
        };
        assert_eq!(inner.ty, Type::Integer);
        assert_eq!(inner.kind, TypedExpKind::Integer(10));
    }

    #[test]
    fn t68_usar_opcional_direto_da_erro_claro_sobre_nil() {
        let errs = check_source(&em_main(
            "    local x: integer? = 10\n    local y: integer = x + 1",
        ))
        .unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("'x' é integer? e pode ser nil")
                    && e.message.contains("if x ~= nil then")),
            "{errs:?}"
        );
    }

    #[test]
    fn t68_opcional_como_condicao_da_erro_claro() {
        let errs =
            check_source(&em_main("    local b: boolean? = true\n    if b then\n    end"))
                .unwrap_err();
        assert!(
            errs.iter().any(|e| e.message.contains("pode ser nil")),
            "{errs:?}"
        );
    }

    #[test]
    fn t68_narrowing_dentro_do_if_da_o_tipo_base() {
        let stats = typed_body_stats(&em_main(
            "    local x: integer? = 10\n\
             \x20   if x ~= nil then\n\
             \x20       local y: integer = x + 1\n\
             \x20   end",
        ));
        let TypedStat::If { thens, .. } = &stats[1] else {
            panic!("esperava If, obteve {:?}", stats[1]);
        };
        assert_eq!(thens[0].narrowed, vec!["x".to_string()]);
    }

    /// `nil ~= x` estreita igual a `x ~= nil` — a ordem dos operandos não é
    /// parte da regra.
    #[test]
    fn t68_narrowing_funciona_com_nil_do_lado_esquerdo() {
        let stats = typed_body_stats(&em_main(
            "    local x: integer? = 10\n\
             \x20   if nil ~= x then\n\
             \x20       local y: integer = x\n\
             \x20   end",
        ));
        let TypedStat::If { thens, .. } = &stats[1] else {
            panic!("esperava If, obteve {:?}", stats[1]);
        };
        assert_eq!(thens[0].narrowed, vec!["x".to_string()]);
    }

    /// O critério de aceite em uma linha: o estreitamento **não** vaza para
    /// depois do `if`.
    #[test]
    fn t68_narrowing_nao_vaza_para_depois_do_if() {
        let errs = check_source(&em_main(
            "    local x: integer? = 10\n\
             \x20   if x ~= nil then\n\
             \x20       local dentro: integer = x\n\
             \x20   end\n\
             \x20   local fora: integer = x",
        ))
        .unwrap_err();
        assert!(
            errs.iter().any(|e| e.message.contains("pode ser nil")),
            "{errs:?}"
        );
        // E só o uso de **fora** reclama: o de dentro tipou.
        assert_eq!(errs.len(), 1, "{errs:?}");
    }

    /// Nem para o `else` do mesmo `if`: lá o valor continua podendo ser nil.
    #[test]
    fn t68_narrowing_nao_vaza_para_o_else() {
        let errs = check_source(&em_main(
            "    local x: integer? = 10\n\
             \x20   if x ~= nil then\n\
             \x20   else\n\
             \x20       local y: integer = x\n\
             \x20   end",
        ))
        .unwrap_err();
        assert!(
            errs.iter().any(|e| e.message.contains("pode ser nil")),
            "{errs:?}"
        );
    }

    /// Nem para o ramo `elseif` seguinte, que é um bloco irmão.
    #[test]
    fn t68_narrowing_nao_vaza_para_o_elseif_seguinte() {
        let errs = check_source(&em_main(
            "    local x: integer? = 10\n\
             \x20   if x ~= nil then\n\
             \x20   elseif 1 == 1 then\n\
             \x20       local y: integer = x\n\
             \x20   end",
        ))
        .unwrap_err();
        assert!(
            errs.iter().any(|e| e.message.contains("pode ser nil")),
            "{errs:?}"
        );
    }

    /// O lado direito de um `and` enxerga o que o lado esquerdo estreitou —
    /// senão o idioma mais natural do teste (`x ~= nil and x > 0`) não
    /// tiparia.
    #[test]
    fn t68_narrowing_atravessa_o_and_da_esquerda_para_a_direita() {
        let stats = typed_body_stats(&em_main(
            "    local x: integer? = 10\n\
             \x20   if x ~= nil and x > 0 then\n\
             \x20       local y: integer = x\n\
             \x20   end",
        ));
        let TypedStat::If { thens, .. } = &stats[1] else {
            panic!("esperava If, obteve {:?}", stats[1]);
        };
        assert_eq!(thens[0].narrowed, vec!["x".to_string()]);
    }

    /// Dois opcionais testados no mesmo `and` estreitam os dois.
    #[test]
    fn t68_and_estreita_os_dois_nomes_testados() {
        let stats = typed_body_stats(&em_main(
            "    local x: integer? = 1\n\
             \x20   local y: integer? = 2\n\
             \x20   if x ~= nil and y ~= nil then\n\
             \x20       local s: integer = x + y\n\
             \x20   end",
        ));
        let TypedStat::If { thens, .. } = &stats[2] else {
            panic!("esperava If, obteve {:?}", stats[2]);
        };
        assert_eq!(thens[0].narrowed, vec!["x".to_string(), "y".to_string()]);
    }

    /// `or` não estreita: nenhum dos lados sabe o que o outro testou.
    #[test]
    fn t68_or_nao_estreita() {
        let errs = check_source(&em_main(
            "    local x: integer? = 1\n\
             \x20   if x ~= nil or 1 == 1 then\n\
             \x20       local y: integer = x\n\
             \x20   end",
        ))
        .unwrap_err();
        assert!(
            errs.iter().any(|e| e.message.contains("pode ser nil")),
            "{errs:?}"
        );
    }

    /// Estreitar não impede atribuir: dentro do ramo o nome continua sendo
    /// a mesma declaração, e o fix-up de mutabilidade a alcança.
    #[test]
    fn t68_atribuicao_dentro_do_ramo_estreitado_alcanca_a_declaracao() {
        let stats = typed_body_stats(&em_main(
            "    local x: integer? = 1\n\
             \x20   if x ~= nil then\n\
             \x20       x = 2\n\
             \x20   end",
        ));
        let TypedStat::Decl { mutable, .. } = &stats[0] else {
            panic!("esperava Decl, obteve {:?}", stats[0]);
        };
        assert!(mutable, "a declaração estreitada deveria sair `mut`");
    }

    #[test]
    fn t68_comparar_opcional_com_nil_e_boolean() {
        let stats = typed_body_stats(&em_main(
            "    local x: integer? = 1\n    local b: boolean = x ~= nil",
        ));
        let TypedStat::Decl { ty, .. } = &stats[1] else {
            panic!("esperava Decl, obteve {:?}", stats[1]);
        };
        assert_eq!(*ty, Type::Boolean);
    }

    /// Comparar um opcional com um valor que não é `nil` é o erro de "usar
    /// sem testar", não o de tipos incomparáveis.
    #[test]
    fn t68_comparar_opcional_com_valor_pede_o_teste() {
        let errs =
            check_source(&em_main("    local x: integer? = 1\n    local b: boolean = x == 10"))
                .unwrap_err();
        assert!(
            errs.iter().any(|e| e.message.contains("pode ser nil")),
            "{errs:?}"
        );
    }

    #[test]
    fn t68_local_com_interrogacao_no_nome_infere_o_opcional() {
        let stats = typed_body_stats(&em_main("    local x? = 10"));
        let TypedStat::Decl { ty, value, .. } = &stats[0] else {
            panic!("esperava Decl, obteve {:?}", stats[0]);
        };
        assert_eq!(
            *ty,
            Type::Option {
                base: Box::new(Type::Integer)
            }
        );
        assert!(matches!(value.kind, TypedExpKind::SomeOf(_)));
    }

    #[test]
    fn t68_local_com_interrogacao_a_partir_de_nil_nao_infere() {
        let errs = check_source(&em_main("    local x? = nil")).unwrap_err();
        assert!(
            errs.iter().any(|e| e.message.contains("não dá para inferir")),
            "{errs:?}"
        );
    }

    /// Um `T?` não é inferido por acidente: `local y = x` com `x: integer?`
    /// propagaria a ausência sem o usuário ter pedido.
    #[test]
    fn t68_tipo_opcional_nao_e_inferido_sem_pedir() {
        let errs =
            check_source(&em_main("    local x: integer? = 1\n    local y = x")).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("tipo opcional não é inferido")),
            "{errs:?}"
        );
    }

    #[test]
    fn t68_parametro_e_retorno_opcionais_tipam() {
        let typed = check_source(
            "function primeiro(xs: {integer}): integer?\n\
             \x20   if #xs == 0 then\n\
             \x20       return nil\n\
             \x20   end\n\
             \x20   return xs[1]\n\
             end\n\
             function usa(x: integer?): integer\n\
             \x20   if x ~= nil then\n\
             \x20       return x\n\
             \x20   end\n\
             \x20   return 0\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   local xs: {integer} = {1, 2}\n\
             \x20   return usa(primeiro(xs))\n\
             end",
        );
        assert!(typed.is_ok(), "{:?}", typed.unwrap_err());
    }

    /// A injeção `T → T?` também vale em argumento e em `return`.
    #[test]
    fn t68_injecao_vale_em_argumento_e_em_retorno() {
        let typed = check_source(
            "function f(x: integer?): integer\n\
             \x20   return 0\n\
             end\n\
             function g(): integer?\n\
             \x20   return 7\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   return f(10)\n\
             end",
        );
        assert!(typed.is_ok(), "{:?}", typed.unwrap_err());
    }

    #[test]
    fn t68_atribuicao_de_nil_e_de_valor_a_local_opcional() {
        let typed = check_source(&em_main(
            "    local x: integer? = nil\n    x = 10\n    x = nil",
        ));
        assert!(typed.is_ok(), "{:?}", typed.unwrap_err());
    }

    #[test]
    fn t68_indexar_array_opcional_pede_o_teste() {
        let errs =
            check_source(&em_main("    local xs: {integer}? = nil\n    local y: integer = xs[1]"))
                .unwrap_err();
        assert!(
            errs.iter().any(|e| e.message.contains("pode ser nil")),
            "{errs:?}"
        );
    }

    #[test]
    fn t68_concatenar_opcional_pede_o_teste() {
        let errs =
            check_source(&em_main("    local s: string? = \"a\"\n    local t: string = s .. \"b\""))
                .unwrap_err();
        assert!(
            errs.iter().any(|e| e.message.contains("pode ser nil")),
            "{errs:?}"
        );
    }

    /// `nil?` não faz sentido: o "ausente" já é o próprio `nil`.
    /// (`value?` tem o mesmo destino, mas ainda não chega até lá: `value`
    /// segue rejeitado antes, desde a T22.)
    #[test]
    fn t68_nil_opcional_e_recusado() {
        let errs = check_source(&em_main("    local x: nil? = nil")).unwrap_err();
        assert!(
            errs.iter().any(|e| e.message.contains("`nil?`")),
            "{errs:?}"
        );
    }

    /// `Option` continua invariante em `compatible` (ADR 0008): a injeção
    /// que a T68 acrescentou é `T → T?`, não variância entre opcionais.
    #[test]
    fn t68_opcionais_de_bases_diferentes_nao_se_misturam() {
        let errs = check_source(&em_main(
            "    local x: integer? = 1\n    local y: float? = x",
        ))
        .unwrap_err();
        assert!(
            errs.iter().any(|e| e.message.contains("incompatíveis")),
            "{errs:?}"
        );
    }

    #[test]
    fn t68_variavel_de_controle_do_for_nao_pode_ser_opcional() {
        let errs = check_source(&em_main("    for i? = 1, 3 do\n    end")).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("variável de controle do `for` não pode ser opcional")),
            "{errs:?}"
        );
    }

    #[test]
    fn t68_interrogacao_no_nome_nao_vale_em_declaracao_multipla() {
        let errs = check_source(&em_main("    local a?, b = 1, 2")).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("não vale em declaração múltipla")),
            "{errs:?}"
        );
    }

    /// Num destino `{T}?`/`{K: V}?`/`Nome?`, quem desambigua o `{...}` é o
    /// tipo base: o `?` fala do destino, não da forma do valor.
    #[test]
    fn t68_literal_composto_se_desambigua_pelo_tipo_base_do_opcional() {
        let stats = typed_body_stats(&em_main("    local xs: {integer}? = {1, 2}"));
        let TypedStat::Decl { ty, value, .. } = &stats[0] else {
            panic!("esperava Decl, obteve {:?}", stats[0]);
        };
        assert_eq!(
            *ty,
            Type::Option {
                base: Box::new(Type::Array {
                    elem: Box::new(Type::Integer)
                })
            }
        );
        let TypedExpKind::SomeOf(inner) = &value.kind else {
            panic!("esperava SomeOf, obteve {:?}", value.kind);
        };
        assert!(matches!(inner.kind, TypedExpKind::ArrayLit(_)));
    }

    /// Record, map e string opcionais tipam e estreitam pelo mesmo caminho
    /// dos arrays — `p.x`, `m["a"]` e `print(s)` dentro do ramo.
    #[test]
    fn t68_record_map_e_string_opcionais_tipam_e_estreitam() {
        let typed = check_source(
            "record Ponto\n\
             \x20   x: integer\n\
             \x20   y: integer\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   local p: Ponto? = {x = 1, y = 2}\n\
             \x20   local m: {string: integer}? = {[\"a\"] = 1}\n\
             \x20   local s: string? = \"oi\"\n\
             \x20   if p ~= nil then\n\
             \x20       print(\"x=\" .. p.x)\n\
             \x20   end\n\
             \x20   if m ~= nil then\n\
             \x20       print(\"a=\" .. m[\"a\"])\n\
             \x20   end\n\
             \x20   if s ~= nil then\n\
             \x20       print(s)\n\
             \x20   end\n\
             \x20   return 0\n\
             end",
        );
        assert!(typed.is_ok(), "{:?}", typed.unwrap_err());
    }

    /// Um composto estreitado entra numa chamada como o composto que é —
    /// o estreitamento vale para tudo que o tipo base vale.
    #[test]
    fn t68_composto_estreitado_pode_ser_passado_a_funcao() {
        let typed = check_source(
            "function soma(xs: {integer}): integer\n\
             \x20   return #xs\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   local xs: {integer}? = {1, 2}\n\
             \x20   if xs ~= nil then\n\
             \x20       print(\"n=\" .. soma(xs))\n\
             \x20   end\n\
             \x20   return 0\n\
             end",
        );
        assert!(typed.is_ok(), "{:?}", typed.unwrap_err());
    }

    /// O `while` **não** estreita: o corpo do laço poderia atribuir `nil` e
    /// a volta seguinte entraria com o valor ausente.
    #[test]
    fn t68_while_nao_estreita() {
        let errs = check_source(&em_main(
            "    local x: integer? = 1\n\
             \x20   while x ~= nil do\n\
             \x20       local y: integer = x\n\
             \x20   end",
        ))
        .unwrap_err();
        assert!(
            errs.iter().any(|e| e.message.contains("pode ser nil")),
            "{errs:?}"
        );
    }

    /// Dentro do ramo estreitado o nome tem o tipo **base**, então atribuir
    /// `nil` ali é recusado — o estreitamento não vira uma janela por onde
    /// a ausência volta a entrar.
    #[test]
    fn t68_atribuir_nil_dentro_do_ramo_estreitado_e_recusado() {
        let errs = check_source(&em_main(
            "    local x: integer? = 1\n\
             \x20   if x ~= nil then\n\
             \x20       x = nil\n\
             \x20   end",
        ))
        .unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("atribuição incompatível para 'x'")),
            "{errs:?}"
        );
    }

    /// Um `local` interno de mesmo nome sombreia o estreitado, e o
    /// estreitamento volta ao normal quando o `if` fecha — as duas coisas
    /// saem da mesma pilha de escopos, sem caso especial.
    #[test]
    fn t68_sombreamento_dentro_do_ramo_estreitado() {
        let typed = check_source(&em_main(
            "    local x: integer? = 1\n\
             \x20   if x ~= nil then\n\
             \x20       local x: string = \"sombra\"\n\
             \x20       print(x)\n\
             \x20   end\n\
             \x20   local depois: integer? = x",
        ));
        assert!(typed.is_ok(), "{:?}", typed.unwrap_err());
    }

    /// Testar com `~= nil` um valor que **não** é opcional segue sendo erro:
    /// a resposta seria constante, e a pergunta denuncia um tipo escrito
    /// errado. Vale também para um nome já estreitado, testado de novo.
    #[test]
    fn t68_testar_nao_opcional_contra_nil_continua_erro() {
        for corpo in [
            "    local x: integer = 1\n    if x ~= nil then\n    end",
            "    local x: integer? = 1\n\
             \x20   if x ~= nil then\n\
             \x20       if x ~= nil then\n\
             \x20       end\n\
             \x20   end",
        ] {
            let errs = check_source(&em_main(corpo)).unwrap_err();
            assert!(
                errs.iter()
                    .any(|e| e.message.contains("não é possível comparar integer com nil")),
                "{corpo}: {errs:?}"
            );
        }
    }

    /// Hover/autocomplete (T49/T50) enxergam o tipo estreitado dentro do
    /// ramo: o estreitamento é um símbolo de verdade na symtab, não um
    /// truque local do `check_exp`.
    #[test]
    fn t68_escopo_do_ramo_estreitado_reporta_o_tipo_base() {
        let source = em_main(
            "    local x: integer? = 1\n\
             \x20   if x ~= nil then\n\
             \x20       local y: integer = x\n\
             \x20   end",
        );
        let tokens = lex(&source).expect("fonte válida");
        let program = parse(&tokens).expect("fonte válida");
        let checked = check(&program).expect("fonte válida");
        assert!(
            checked.scopes.iter().any(|escopo| escopo
                .symbols
                .iter()
                .any(|s| s.name == "x" && s.type_name == "integer")),
            "nenhum escopo reportou `x: integer`"
        );
    }

    // ---- T70: cast `as` --------------------------------------------------

    /// Extrai o `TypedExp` do `local x = ...` do corpo.
    fn t70_valor_do_decl(source: &str) -> TypedExp {
        let stats = typed_body_stats(source);
        let TypedStat::Decl { value, .. } = &stats[0] else {
            panic!("esperava Decl, obteve {:?}", stats[0]);
        };
        value.clone()
    }

    #[test]
    fn t70_cast_numerico_tipa_nos_dois_sentidos() {
        let v = t70_valor_do_decl(&em_main("    local x: float = 1 as float"));
        assert!(v.ty.equals(&Type::Float));
        assert!(matches!(
            v.kind,
            TypedExpKind::Cast {
                kind: CastKind::IntToFloat,
                ..
            }
        ));

        let v = t70_valor_do_decl(&em_main("    local x: integer = 3.9 as integer"));
        assert!(v.ty.equals(&Type::Integer));
        assert!(matches!(
            v.kind,
            TypedExpKind::Cast {
                kind: CastKind::FloatToInt,
                ..
            }
        ));
    }

    /// Qualquer tipo sobe para `value` — inclusive composto e record.
    #[test]
    fn t70_qualquer_tipo_sobe_para_value() {
        for corpo in [
            "    local x: value = 1 as value",
            "    local x: value = \"a\" as value",
            "    local x: value = true as value",
            "    local x: value = nil as value",
        ] {
            let v = t70_valor_do_decl(&em_main(corpo));
            assert!(v.ty.equals(&Type::Value), "{corpo}");
            assert!(
                matches!(
                    v.kind,
                    TypedExpKind::Cast {
                        kind: CastKind::ToValue,
                        ..
                    }
                ),
                "{corpo}"
            );
        }
    }

    #[test]
    fn t70_value_desce_para_primitiva() {
        let stats = typed_body_stats(&em_main(
            "    local v: value = 1 as value\n\
             \x20   local x: integer = v as integer",
        ));
        let TypedStat::Decl { value, .. } = &stats[1] else {
            panic!("esperava Decl, obteve {:?}", stats[1]);
        };
        assert!(value.ty.equals(&Type::Integer));
        assert!(matches!(
            value.kind,
            TypedExpKind::Cast {
                kind: CastKind::FromValue,
                ..
            }
        ));
    }

    /// O critério de aceite da T70: `"a" as integer` é erro claro — cast não
    /// é parsing.
    #[test]
    fn t70_cast_de_string_para_integer_e_erro_claro() {
        let errs = check_source(&em_main("    local x: integer = \"a\" as integer")).unwrap_err();
        let msg = errs[0].to_string();
        assert!(msg.contains("não existe cast de string para integer"), "{msg}");
        assert!(msg.contains("não interpreta texto"), "{msg}");
    }

    /// Cast entre compostos não existe: nem por elemento, nem por
    /// reinterpretação.
    #[test]
    fn t70_cast_entre_compostos_e_erro() {
        let errs = check_source(&em_main(
            "    local a: {integer} = {1}\n\
             \x20   local b: {float} = a as {float}",
        ))
        .unwrap_err();
        assert!(
            errs[0].to_string().contains("não existe cast"),
            "{}",
            errs[0]
        );
    }

    /// A descida de `value` só vai a primitiva: para composto, a conversão
    /// falharia no meio do caminho.
    #[test]
    fn t70_value_nao_desce_para_composto() {
        let errs = check_source(&em_main(
            "    local v: value = 1 as value\n\
             \x20   local a: {integer} = v as {integer}",
        ))
        .unwrap_err();
        assert!(
            errs[0].to_string().contains("só desce para tipo primitivo"),
            "{}",
            errs[0]
        );
    }

    /// Cast para o próprio tipo é identidade: passa, e sem nó de conversão.
    #[test]
    fn t70_cast_de_identidade_nao_gera_no() {
        let v = t70_valor_do_decl(&em_main("    local x: integer = 5 as integer"));
        assert!(v.ty.equals(&Type::Integer));
        assert!(
            matches!(v.kind, TypedExpKind::Integer(5)),
            "esperava o literal intacto, obteve {:?}",
            v.kind
        );
    }

    /// `value` deixou de ser rejeitado como anotação (a T25 o recusava porque
    /// o codegen não sabia emiti-lo).
    #[test]
    fn t70_value_e_tipo_valido_em_anotacao_parametro_e_retorno() {
        check_source(
            "function f(v: value): value\n\
             \x20   return v\n\
             end\n\
             function main(args: {string}): integer\n\
             \x20   return 0\n\
             end",
        )
        .expect("`value` deveria ser tipo válido em parâmetro e retorno");
    }

    /// `value?` continua sem sentido — `value` já aceita `nil` (regra que a
    /// T68 escreveu e a T70 não afrouxa).
    #[test]
    fn t70_value_opcional_continua_recusado() {
        let errs = check_source(&em_main("    local x: value? = nil")).unwrap_err();
        assert!(errs[0].to_string().contains("`value?` não faz sentido"), "{}", errs[0]);
    }
}
