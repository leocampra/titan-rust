# PRD — Titan-Rust · Fase 0: Hello World

> Lista de tarefas derivada de `plano.md` e do planejamento de arquitetura.
> Objetivo da fase: `titanc examples/hello.titan && ./hello` imprime `Olá, mundo!`.

## Resumo executivo

Construir um compilador **novo, escrito em Rust**, para uma linguagem tipada inspirada em Lua que
gera código Rust nativo. O projeto Titan original (`titan/`, escrito em Lua) serve como
**especificação de referência** de gramática, AST e sistema de tipos — não como base de código.

**Por que não reaproveitar o backend do Titan:** o `titan/titan-compiler/coder.lua` (3182 linhas)
não gera C portável; gera C acoplado à API **interna** do Lua 5.3 (`String → TString*`,
`Array → Table*`, toda função recebe `lua_State *L`). É ~0% reaproveitável para um alvo Rust.
Além disso, o Titan exige Lua 5.3.5 + LuaRocks, ausentes nesta máquina.

**Decisões fixadas:** extensão `.titan` · binário `titanc` · runtime `titan-runtime` ·
`print` vem da stdlib em Rust (o Titan original **não tem `print`**) · Redox fora de escopo.

**Convenções de trabalho** (valem para todas as tarefas):
- `titan/` e `lua/` são **somente leitura** — repositórios de terceiros usados como referência.
- Cada tarefa termina com `cargo test` verde antes da seguinte.
- Mensagens de erro do compilador em português.

**Skills transversais** (aplicáveis a praticamente toda tarefa de código):
`rust-pro` · `clean-code` · `test-driven-development` · `verification-before-completion`

---

## T0 — Fundação: workspace Cargo e runtime

**Objetivo:** provar a fundação (Rust chamando o runtime e imprimindo) **antes** de existir compilador.

**Entregáveis:**
- `Cargo.toml` na raiz: `members = ["crates/*"]`, `exclude = ["titan", "lua", "build"]`
  — sem o `exclude`, o cargo tenta interpretar os diretórios de referência como membros.
- `crates/titan-runtime/src/lib.rs` com `print(&str)` e `concat(&str, &str) -> String`.
- `crates/titanc/` com esqueleto de binário.
- `.gitignore` cobrindo `/target` e `/build`.

**Critério de aceite:** um `main.rs` escrito à mão que chama `titan_runtime::print("Olá, mundo!")`
compila e imprime. `cargo build` e `cargo test` verdes na raiz.

**Skills:** `systems-programming-rust-project` (scaffolding de projeto Rust) · `rust-pro` ·
`monorepo-architect` (layout de workspace multi-crate) · `clean-code`

---

## T1 — `ast.rs`: estruturas da AST

**Objetivo:** espelhar `titan/titan-compiler/ast.lua` (80 linhas) em enums Rust.

**Detalhes:**
- Definir os enums **completos** (todos os nós do `ast.lua`), mesmo que parser/checker só tratem um
  subconjunto agora — evita refatorar tipos a cada fase futura.
- Manter os **mesmos nomes** dos nós (`ExpString`, `StatCall`, `TopLevelFunc`…) para que o Titan
  siga servindo de referência viva.
- Todo nó carrega `loc: Loc { line, col }`.
- Um programa é `Vec<TopLevel>` — o Titan também não tem nó `Program`.

**Referência:** `titan/titan-compiler/ast.lua`

**Skills:** `rust-pro` · `typescript-advanced-types` (modelagem de ADTs/tipos algébricos — o
raciocínio se aplica a enums Rust) · `clean-code`

---

## T2 — `types.rs`: sistema de tipos

**Objetivo:** espelhar `titan/titan-compiler/types.lua:3-25`.

**Detalhes:**
- Variantes: `Invalid`, `Nil`, `Boolean`, `Integer`, `Float`, `String`, `Value`,
  `Function{params, rettypes}`, `Array{elem}`, `Map{keys, values}`, `Record{…}`, `Option{base}`.
- Implementar `equals` e `compatible` (o Titan tem *gradual typing*: `Value` é compatível com tudo).
- Nesta fase só primitivas + `Function`/`Array` são exercitadas.

**Referência:** `titan/titan-compiler/types.lua` (`equals:199`, `compatible:150`)

**Skills:** `rust-pro` · `clean-code` · `test-driven-development`

---

## T3 — `lexer.rs`: análise léxica

**Objetivo:** porta manual de `titan/titan-compiler/lexer.lua` (230 linhas, LPeg → lexer manual).

**Detalhes:**
- Keywords: `function`, `local`, `return`, `end`, `true`, `false`, `nil`, e as reservadas de tipo
  `boolean integer float string value` (`lexer.lua:170`).
- Literais numéricos: **distinguir inteiro de float** — a diferença define o tipo.
- Strings: aspas simples/duplas com escapes, e long strings `[[...]]`.
- Símbolos: `( ) { } , : ; ..` · Comentários: `--` e `--[[ ]]`.
- Emite `Vec<Token>` com posição (linha/coluna).

**Critério de aceite:** testes de tokenização de `hello.titan`, incluindo string com acentos
(UTF-8) e caso de string não terminada produzindo erro com linha/coluna.

**Skills:** `rust-pro` · `test-driven-development` · `clean-code`

---

## T4 — `parser.rs`: descida recursiva

**Objetivo:** substituir a gramática PEG de `titan/titan-compiler/parser.lua` (575 linhas) por um
parser de descida recursiva que produz `Vec<TopLevel>`.

**Sintaxe a suportar nesta fase:**
```lua
[local] function nome(p: T, ...) [: TipoRetorno] ... end
local x [: T] = exp
```
Statements: `StatCall`, `StatReturn`, `StatDecl`.
Expressões: `ExpString`, `ExpInteger`, `ExpFloat`, `ExpBool`, `ExpNil`, `ExpVar`, `ExpCall`,
`ExpConcat` (`..`).
Tipos: `integer`, `float`, `boolean`, `string`, `nil`, `{T}`.

**Detalhes:** tipo de retorno omitido vira `TypeNil` (`parser.lua:44-47`). Erros com linha/coluna e
mensagem em português, no espírito de `titan/titan-compiler/syntax_errors.lua`.

**Critério de aceite:** teste que produz a AST esperada para `hello.titan`; erro claro (sem panic)
para `end` faltando.

**Skills:** `rust-pro` · `test-driven-development` · `clean-code` · `error-handling-patterns`

---

## T5 — `checker.rs`: análise semântica e tipos

**Objetivo:** verificar tipos e anotar a AST, espelhando a estratégia de
`titan/titan-compiler/checker.lua` (1662 linhas).

**Detalhes:**
- **Duas passadas** (como o Titan): (1) coleta assinaturas top-level, permitindo chamada antes da
  declaração; (2) verifica corpos, anotando cada `Exp` com o tipo resolvido.
- Símbolos com escopo em pilha — espelhar `titan/titan-compiler/symtab.lua` (67 linhas).
- `print` é registrado no escopo global como `Function{params:[String], rettypes:[Nil]}`, originado
  do runtime — **não** é palavra-chave.
- Validar assinatura de `main`: `main(args: {string}): integer`
  (ver `titan/titan-compiler/checker.lua:1593-1607`).

**Erros a cobrir com teste** (mensagem clara, nunca panic):
`print(42)` → argumento incompatível · `funcao_inexistente()` → não declarada ·
`main` retornando `string` → retorno incompatível · `if x then end` → construção não suportada ·
arquivo `.titan` do Titan original (com `foreign import`/records) → construção não suportada.

**Skills:** `rust-pro` · `test-driven-development` · `architect-review` · `error-handling-patterns`

---

## T6 — `codegen.rs`: backend Rust

**Objetivo:** traduzir a AST tipada em código Rust legível.

**Mapeamento de tipos (Fase 0):**

| Titan | Rust |
|---|---|
| `integer` | `i64` |
| `float` | `f64` |
| `boolean` | `bool` |
| `string` (literal) | `&'static str` |
| `string` (computada) | `String` |
| `nil` (retorno) | `()` |
| `{string}` (só param de `main`) | `&[String]` |

**Detalhes:**
- Estrutura espelhando o `coder.lua` (`codestat`/`codeexp` por variante), mas emitindo Rust.
- Funções → `fn nome(...) -> T`; `local function` → sem `pub`.
- Mangling com prefixo (`titan_main`) para evitar colisão com o `fn main` do shim e com keywords do Rust.
- Shim de entrada:
  ```rust
  fn main() {
      let args: Vec<String> = std::env::args().skip(1).collect();
      std::process::exit(titan_main(&args) as i32);
  }
  ```
- Emitir já indentado e legível (o Rust gerado é artefato de depuração) — sem passo de reindent.

> ⚠️ **Manter o mapeamento de tipos isolado numa única função.** Nada na Fase 0 deve assumir que
> valores são `Copy`. Na Fase 2 (arrays/maps/records) será preciso escolher um modelo de memória, e
> essa troca precisa ser uma mudança localizada.

**Skills:** `rust-pro` · `clean-code` · `test-driven-development` · `architect-review`

---

## T7 — `driver.rs` + `main.rs`: CLI e invocação do cargo

**Objetivo:** amarrar o pipeline e produzir o executável nativo.

**CLI:** `titanc [--emit-rust] [--out DIR] [-v] <arquivo.titan>`

**Fluxo:**
1. lê o fonte → lexer → parser → checker;
2. gera `build/<nome>/src/main.rs` e `build/<nome>/Cargo.toml` (com `titan-runtime` por path absoluto);
3. invoca `cargo build --release` nesse diretório;
4. copia o executável para o diretório atual como `<nome>`.

`--emit-rust` para no passo 2 e imprime o Rust gerado (essencial para depurar).
`-v` mostra a invocação do cargo (como o `-v` do `titanc` original).

> ⚠️ **Duas armadilhas do Cargo:**
> 1. O `Cargo.toml` gerado precisa de um `[workspace]` **vazio**, senão o cargo tenta anexá-lo ao
>    workspace pai e a build quebra.
> 2. `titan-runtime` deve ser referenciado por **caminho absoluto** — sem rede nem registry.

**Skills:** `rust-pro` · `bash-defensive-patterns` (invocação robusta de subprocesso) ·
`error-handling-patterns` · `clean-code`

---

## T8 — Integração ponta a ponta e testes

**Objetivo:** validar o fluxo completo e blindar contra regressão.

**Entregáveis:**
- `examples/hello.titan`:
  ```lua
  function main(args: {string}): integer
      print("Olá, mundo!")
      return 0
  end
  ```
- Teste de integração que compila e executa, conferindo stdout **e** exit code.
- Suíte de casos negativos de T4/T5 (erro claro, sem panic).

**Verificação:**
```bash
cd /home/leonardo/titan-rust
cargo build --release
./target/release/titanc examples/hello.titan
./hello          # → Olá, mundo!
echo $?          # → 0
./target/release/titanc --emit-rust examples/hello.titan   # inspeção
cargo test
```

> ⚠️ **Não instalar `titanc` no PATH global nesta fase.** O `titanc` original é um script Lua em
> `titan/titanc`; invocar sempre por caminho explícito elimina qualquer ambiguidade.

**Skills:** `test-automator` · `verification-before-completion` · `webapp-testing` (padrões de teste
de integração/E2E) · `find-bugs`

---

## T9 — Documentação e fechamento

**Objetivo:** deixar o projeto compreensível e reprodutível por terceiros.

**Entregáveis:**
- `README.md`: o que é, como compilar, como rodar o hello world, relação com o Titan original,
  e o que **não** está implementado ainda.
- Documento curto de arquitetura: o pipeline, e por que o backend do Titan não foi reaproveitado.
- Registrar as decisões de projeto (compilador em Rust do zero; `print` via runtime; extensão
  `.titan`) como ADRs.

**Skills:** `readme` · `docs-architect` · `architecture-decision-records` · `mermaid-expert`
(diagrama do pipeline)

---

## Revisão de qualidade (contínua)

Ao fim de T6 e novamente ao fim de T8, passar o código por revisão antes de seguir:

**Skills:** `code-reviewer` · `architect-review` · `find-bugs` · `code-review-checklist`

---

## Fora de escopo nesta fase

Rejeitar com erro claro do checker (nunca panic): `if`/`while`/`for`, records, maps, arrays
manipuláveis, `import`, `foreign import`, métodos, operadores aritméticos, retornos múltiplos,
`Option`/`?`.

**Redox OS** fora de escopo — compilar para Linux nativo. Nada na arquitetura impede um `--target`
depois, já que o alvo real é o `cargo`.

---

## Roadmap além da Fase 0

| Fase | Escopo | Complexidade |
|---|---|---|
| **0. Hello world** | pipeline completo, subconjunto mínimo (~1500-2000 linhas Rust) | **Baixa-média** |
| 1. Núcleo da linguagem | int/float/bool, aritmética, `if`/`while`/`for`, funções | Média |
| 2. Tipos compostos | arrays, maps, records, strings dinâmicas | **Alta** — ownership/borrow mordem aqui |
| 3. Capability Runtimes | AI, Crypto, Data (`titan-ai`, `titan-crypto`…) | Alta (engenharia de bibliotecas) |
| 4. Self-hosting / LSP | compilador escrito na própria linguagem | Muito alta |

O custo real do projeto está na **Fase 2**: escolher o modelo de memória em Rust
(`Rc<RefCell<…>>`, arena, ou GC próprio). A Fase 0 é desenhada para não fechar essa porta.

---
---

# PRD — Titan-Rust · Fase 1: Núcleo da linguagem

> Continuação da Fase 0 (T0–T9, **concluída**). Objetivo da fase:
> `titanc examples/nucleo.titan && ./nucleo` calcula e imprime fatorial e fibonacci usando
> aritmética, `if`, `while`, `for` e atribuição.

## Resumo executivo

A Fase 0 entregou o pipeline completo para um subconjunto mínimo. A Fase 1 o transforma num
núcleo de linguagem: operadores aritméticos/relacionais/lógicos, `if`/`while`/`for` numérico e
atribuição single-target.

**Ponto de partida favorável:** a `ast.rs` já é completa — `StatIf`, `StatWhile`, `StatFor`,
`StatAssign`, `ExpBinop`, `ExpUnop`, `Then` já existem. A fase não adiciona nós à AST bruta;
ela ensina o **parser** a produzi-los, o **checker** a verificá-los (os enums
`TypedStat`/`TypedExpKind` são fechados e ganham variantes novas) e o **codegen** a traduzi-los.

**Referências vivas no Titan original** (`titan/`, somente leitura): tabela de precedência em
`parser.lua:369-395` · regras de tipo de operadores em `checker.lua:910-1122` · semântica de
`for`/`if`/`while`/`assign` em `checker.lua:239-288` e `365-457`.

**Decisões fixadas (confirmadas com o usuário):**
1. `for` só numérico: `for x = start, finish[, inc] do ... end` (integer ou float). Sem for-in.
2. `StatAssign` single-target: `nome = exp` para local já declarada. Sem multi-assign,
   sem índice/campo.
3. Operadores: `+ - * / % ^` · `== ~= < > <= >=` · `and or not` · unário `-` e `not`.
   **Sem** bitwise (`& | ~ << >>`), **sem** `//`, **sem** `#`.
4. `..` coage número→string (`"x: " .. 42` funciona), espelhando o `trytostr` do original.
5. `for` exige tipos idênticos: `start`/`finish`/`inc` batem exatamente com o tipo da variável
   de controle (fiel ao original; `for x = 1, 10.0` → erro claro).
6. Mutabilidade rastreada no checker: `TypedStat::Decl` ganha `mutable: bool`; codegen emite
   `let mut` só quando há reatribuição — Rust gerado sem warnings.
7. `and`/`or` boolean estrito: os dois lados boolean, resultado boolean, mapeiam para `&&`/`||`.
   Divergência deliberada do truthy/falsy do original (`Value`/`Option` fora de uso nesta fase)
   — documentar no código.

**Convenções de trabalho** (herdadas da Fase 0, seguem valendo):
- `titan/` e `lua/` são **somente leitura**.
- Cada tarefa termina com `cargo test` verde antes da seguinte.
- Mensagens de erro do compilador em português — nunca panic.

**Skills transversais:** `rust-pro` · `clean-code` · `test-driven-development` ·
`verification-before-completion`

---

## T10 — `lexer.rs`: operadores e keywords de controle de fluxo

**Objetivo:** estender o lexer com os tokens da Fase 1, sem tocar no que já existe.

**Detalhes:**
- Novos `TokenKind`: `Plus` `Minus` `Star` `Slash` `Percent` `Caret` (aritméticos) ·
  `Eq` `Ne` `Lt` `Gt` `Le` `Ge` (relacionais) · keywords `And` `Or` `Not` `If` `Then` `Elseif`
  `Else` `While` `Do` `For` (mesmo mecanismo das keywords atuais).
  **Não** adicionar `Repeat`/`Until`/`Break` — fora de escopo.
- Lookahead de 1 char no padrão já usado para `.` vs `..`: `=` vs `==`, `<` vs `<=`, `>` vs `>=`.
- `~` só existe em `~=`; `~` isolado → erro léxico claro (sem bitwise nesta fase).

**Critério de aceite:** testes de tokenização para cada token novo, ambiguidades
(`<`/`<=`, `=`/`==`, `..` intacto) e o caso negativo `~` isolado.

**Skills:** `rust-pro` · `test-driven-development` · `clean-code`

---

## T11 — `parser.rs`: precedência de expressões + `if`/`while`/`for`/atribuição

**Objetivo:** substituir `parse_exp` (hoje só delega a `parse_concat_exp`) por uma cascata
completa de níveis de precedência e adicionar os statements novos.

**Abordagem: cascata de funções por nível** (não Pratt) — mantém o estilo de descida recursiva
já usado no arquivo; são só 5 níveis novos (bitwise fora de escopo), custo baixo. Hierarquia
(espelhando `parser.lua:369-395`, níveis bitwise omitidos):

```text
parse_exp → parse_or_exp
or_exp     : and_exp (or and_exp)*                       — assoc. esquerda
and_exp    : rel_exp (and rel_exp)*                      — assoc. esquerda
rel_exp    : concat_exp ((== ~= < > <= >=) concat_exp)?  — SEM encadear (fiel ao original)
concat_exp : add_exp (.. concat_exp)?                    — já existe, assoc. direita
add_exp    : mul_exp ((+ -) mul_exp)*                    — assoc. esquerda
mul_exp    : unary_exp ((* / %) unary_exp)*              — assoc. esquerda
unary_exp  : (not | -)* pow_exp
pow_exp    : simple_exp (^ unary_exp)?                   — assoc. direita (2^3^2 = 2^(3^2))
```

**Detalhes:**
- `ExpBinop`/`ExpUnop` com `op: String` usando exatamente as strings do original
  (`"+"`, `"~="`, `"and"`, ...) — é o que o checker vai casar.
- **`if`**: `if exp then block (elseif exp then block)* (else block)? end` →
  `StatIf { thens: Vec<Then>, elsestat }`.
- **`while`**: `while exp do block end`.
- **`for`**: `for nome [: T] = exp, exp [, exp] do block end` — reusa `parse_decl_opt_type`.
- **`StatAssign` vs `StatCall`** (sem backtracking): `parse_suffixed_exp()` primeiro; se o
  próximo token é `=` e a exp é `ExpVar(VarName)` → `StatAssign`; se é `=` mas a exp é
  `ExpCall` → erro "não é possível atribuir a uma chamada de função"; senão exige `ExpCall`
  (caminho atual). Mesma técnica do original (`suffixedexp => exp_is_var` + checar `ASSIGN`,
  `parser.lua:354-358`).
- **Remover** o teste `if_fora_do_subconjunto_produz_erro_claro_sem_panic` (testava o
  comportamento antigo); substituir por teste positivo da estrutura do `StatIf`.

**Critério de aceite:** testes negativos (`if` sem `then` · `while` sem `do` · `for x = 1 do` ·
`f() = 1` · `1 + = 2`) e testes estruturais de precedência (`1 + 2 * 3` associa `*` primeiro ·
`2 ^ 3 ^ 2` associa à direita · `a == b and c == d` · `- -1` · `not not true`).

**Skills:** `rust-pro` · `test-driven-development` · `clean-code` · `error-handling-patterns`

---

## T12 — `checker.rs`: novas variantes de `TypedStat` (If/While/For/Assign)

**Objetivo:** estender a AST tipada e a checagem de statements, reusando a `SymTab` em pilha.

**Novas variantes:**
```rust
TypedStat::If     { thens: Vec<TypedThen>, elsestat: Option<Box<TypedStat>> }
TypedStat::While  { condition: TypedExp, block: Box<TypedStat> }
TypedStat::For    { name, ty, start, finish, inc: TypedExp, block }  // inc sempre presente:
                                                                     // default 1|1.0 se omitido
TypedStat::Assign { name: String, value: TypedExp }
// TypedStat::Decl ganha `mutable: bool`
```

**Detalhes:**
- **Condições** (`if`/`elseif`/`while`): tipo `Boolean` (ou `Value` via `compatible`), senão
  erro claro. Corpos com `open_block`/`close_block` (reuso direto da `SymTab`).
- **`for`** (espelha `checkfor`, `checker.lua:239-288`): checar `start`/`finish`/`inc` **antes**
  de declarar a variável (elas não podem referenciá-la); tipo da variável = anotação explícita
  ou inferido de `start`; deve ser `Integer` ou `Float`; `start`/`finish`/`inc` com tipo
  **idêntico** ao da variável (decisão 5); `inc` omitido vira `1`/`1.0` conforme o tipo;
  variável declarada num bloco próprio que não vaza para fora do laço.
- **`Assign`**: single-target `VarName`; símbolo deve existir (senão "'x' não foi declarado");
  rejeitar atribuição a `Type::Function` ("não é possível atribuir a uma função",
  `checker.lua:401`); valor `compatible` com o tipo do símbolo.
- **Rastreio de mutabilidade** (decisão 6): a `SymTab` passa a guardar `(Type, decl_id)`; cada
  `StatAssign` registra o `decl_id` num `HashSet` (a variável de `for` é sempre mut no template
  do T15, não precisa de rastreio); fix-up ao final do corpo da função seta `mutable` nos
  `TypedStat::Decl` correspondentes. Shadowing é respeitado naturalmente — o id vem do símbolo
  resolvido na pilha. Mesmo espírito do `var._decl._assigned = true` do original.

**Erros a cobrir com teste:** `if 42 then end` · `while "oi" do end` · `for x = "a", 10 do end` ·
`for x = 1, 10.0 do end` (tipos não idênticos) · `x = 10` sem declarar · `print = 1` ·
multi-assign via AST montada à mão (defensivo — o parser da T11 nunca produz).

**Skills:** `rust-pro` · `test-driven-development` · `architect-review` ·
`error-handling-patterns`

---

## T13 — `checker.rs`: `Binop`/`Unop` em `TypedExpKind` + coerção numérica

**Objetivo:** regras de tipo dos operadores, com a coerção int→float centralizada numa função
reutilizável (usada também por T12 e pelo `..`).

**Novas variantes:**
```rust
TypedExpKind::Binop { op: BinOp, lhs, rhs }  // enum BinOp { Add, Sub, Mul, Div, Mod, Pow,
                                             //              Eq, Ne, Lt, Gt, Le, Ge, And, Or }
TypedExpKind::Unop  { op: UnOp, exp }        // enum UnOp { Neg, Not }
```
Enums (não `String`) na AST tipada → `match` exaustivo no codegen. Conversão `String → BinOp`
uma vez em `check_exp`, com erro claro (não panic) no braço `_`.

**Regras** (espelhando `checker.lua:910-1122`, sem bitwise/gradual typing):
- **`+ - * %`**: ambos numéricos; ambos `Integer` → `Integer`; qualquer `Float` → coage o outro
  lado e resulta `Float`. Coerção centralizada numa função só.
- **`/` e `^`**: sempre coagem ambos para `Float`; resultado sempre `Float` (mesmo int/int).
- **`== ~=`**: lados `compatible` entre si (int/float com coerção, string/string, bool/bool);
  resultado `Boolean`.
- **`< > <= >=`**: number-number (com coerção) ou string-string; nunca bool; resultado `Boolean`.
- **`and or`**: boolean estrito nos dois lados (decisão 7); resultado `Boolean`; comentário no
  código registrando a divergência deliberada do original.
- **Unário `-`**: operando numérico, resultado do mesmo tipo. **`not`**: `Boolean` → `Boolean`.
- **`..` (`ExpConcat`)**: operandos `String`, `Integer` ou `Float` (decisão 4); resultado
  `String`; `Boolean`/`Nil` → erro.
- O checker **não** emite nó de cast: o codegen decide o `as f64` comparando o tipo do operando
  com o tipo do resultado.

**Skills:** `rust-pro` · `test-driven-development` · `architect-review` ·
`error-handling-patterns`

---

## T14 — `codegen.rs`: If/While/Assign/Binop/Unop

**Objetivo:** emitir Rust idiomático para os nós novos que não envolvem `for` (isolado na T15).

**Detalhes:**
- **If** → `if c0 { } else if c1 { } else { }` (omitir `else` se `None`); reusa
  `emit_block_stats`/`indent`.
- **While** → `while cond { }` (tradução direta — sem `break`/`continue` não há divergência).
- **Assign** → `nome = valor;`. **Decl** emite `let mut` só quando `mutable == true` (T12).
- **Binop**: mapeamento `Add→+` ... `Ne→!=` (atenção: Titan `~=`, Rust `!=`) `And→&&` `Or→||`;
  emitir **sempre com parênteses** `(lhs op rhs)` para não depender da precedência do Rust
  coincidir com a do Titan. Operando `Integer` em resultado `Float` → envolver com `(x as f64)`.
  `Pow` → `(lhs as f64).powf(rhs as f64)` — Rust não tem `^` de potência (é XOR).
- **Unop**: `Neg → (-e)` · `Not → (!e)`.
- **Concat com números**: operando `Integer`/`Float` vira `&x.to_string()` para o
  `titan_runtime::concat` existente.

**Critério de aceite** (padrão atual: `--emit-rust` + execução real via rustc): `10 / 3`
imprime `3.333…` (não `3`) · `2 ^ 10` usa `.powf` e dá `1024` · `~=` vira `!=` · precedência
preservada nos parênteses · `let mut` só nas variáveis reatribuídas (Rust gerado sem warnings).

**Skills:** `rust-pro` · `clean-code` · `test-driven-development` · `architect-review`

---

## T15 — `codegen.rs`: `StatFor` desaçucarado para `while`

**Objetivo:** estratégia única e correta de codegen para o `for` numérico.

**Decisão: sempre desaçucarar para `while`, nunca emitir `Range` do Rust** — `.step_by` não
aceita passo negativo nem float, `Range<f64>` nem implementa `Iterator`; um único template
cobre integer/float, `inc` omitido, `inc` negativo e `inc` só conhecido em runtime. Sem caminho
otimizado para `inc=1` literal nesta fase (documentar como otimização futura). Registrar como
ADR (T18).

```rust
{
    let mut nome: T = start;
    let titan_for_finish: T = finish;
    let titan_for_inc: T = inc;
    let titan_for_asc: bool = titan_for_inc > 0 as T;
    while (titan_for_asc && nome <= titan_for_finish)
        || (!titan_for_asc && nome >= titan_for_finish) {
        // corpo
        nome += titan_for_inc;
    }
}
```

- Bloco externo isola as auxiliares e a variável de controle (não vazam — semântica Titan).
- Prefixo `titan_` segue a convenção de mangling existente (`mangle_fn_name`).
- `T` = `i64`|`f64` vindo de `TypedStat::For::ty` — o template é idêntico para os dois.
- A direção (`titan_for_asc`) é computada uma única vez antes do laço.

**Critério de aceite** (execução real): `for i = 1, 5` → 5 iterações · `for i = 5, 1, -1` →
decrescente · `for i = 1, 10, 2` → 1,3,5,7,9 · `for x = 0.0, 1.0, 0.25` → float, conferir por
contagem (não comparação exata de float) · `for i = 1, 0` → zero iterações.

**Skills:** `rust-pro` · `clean-code` · `test-driven-development` · `architect-review`

---

## T16 — Testes negativos consolidados e regressão

**Objetivo:** garantir que tudo que continua fora de escopo segue rejeitado com erro claro,
sem panic — e que a Fase 1 não afrouxou nada além do pretendido.

**Detalhes:**
- **Remover** `checker.rs::if_fora_do_subconjunto_produz_erro_claro_sem_panic` — o valor do
  teste era "checker rejeita `if`", que deixou de ser verdade. Não inventar caso sintético
  substituto para forma de AST que o parser não produz mais.
- Casos negativos novos via parser real: `1 + "a"` · `"a" < 1` · `true + false` · `1 and 2` ·
  `true .. "x"` · os listados em T12.
- **Regressão:** todos os testes de rejeição da Fase 0 (record, `import`, `foreign import`,
  métodos, `v[i]`/`{...}`, retornos múltiplos, `Option`) continuam passando sem alteração —
  incluindo o teste com arquivo `.titan` do Titan original.

**Skills:** `test-automator` · `verification-before-completion` · `find-bugs`

---

## T17 — Exemplo `.titan` e integração ponta a ponta

**Objetivo:** um programa novo que exercite aritmética + `if` + `while` + `for` + atribuição +
funções, com teste de integração real (mesmo padrão do T8).

**Entregável — `examples/nucleo.titan`:**
```lua
function fatorial(n: integer): integer
    if n <= 1 then
        return 1
    end
    local resultado: integer = 1
    local i: integer = 2
    while i <= n do
        resultado = resultado * i
        i = i + 1
    end
    return resultado
end

function fibonacci(n: integer): integer
    if n <= 1 then
        return n
    end
    local a: integer = 0
    local b: integer = 1
    for j = 2, n do
        local prox: integer = a + b
        a = b
        b = prox
    end
    return b
end

function main(args: {string}): integer
    print("Fatorial de 5: " .. fatorial(5))
    print("Fibonacci de 10: " .. fibonacci(10))
    return 0
end
```

**Critério de aceite:** teste de integração invoca o binário `titanc` real, executa o binário
gerado, confere stdout completo (`Fatorial de 5: 120` / `Fibonacci de 10: 55`) **e** exit
code 0.

**Verificação:**
```bash
cd /home/leonardo/titan-rust/titan-rust
cargo build --release
./target/release/titanc examples/nucleo.titan
./nucleo         # → Fatorial de 5: 120 · Fibonacci de 10: 55
echo $?          # → 0
./target/release/titanc --emit-rust examples/nucleo.titan   # inspeção (sem warnings de mut)
./target/release/titanc examples/hello.titan && ./hello     # regressão Fase 0
cargo test
```

**Skills:** `test-automator` · `verification-before-completion` · `find-bugs`

---

## T18 — Documentação e fechamento da Fase 1

**Objetivo:** deixar o projeto compreensível e as decisões registradas.

**Entregáveis:**
- `README.md`: mover `if`/`while`/`for`, operadores e `x = exp` da seção "O que não está
  implementado ainda" para o coberto; atualizar o estado atual para Fase 1.
- ADR `docs/adr/0004-for-desacucarado-para-while.md` (decisão mais não-óbvia da fase) e ADR ou
  nota para o `and`/`or` boolean estrito (divergência deliberada da referência).
- Marcar a Fase 1 como concluída no roadmap.

**Skills:** `readme` · `docs-architect` · `architecture-decision-records`

---

## Revisão de qualidade (contínua)

Como na Fase 0: revisão de código ao fim de T14 e novamente ao fim de T17, antes de seguir.

**Skills:** `code-reviewer` · `architect-review` · `find-bugs` · `code-review-checklist`

---

## Fora de escopo nesta fase

Rejeitar com erro claro (nunca panic): records, maps, arrays manipuláveis (`v[i]`, `{...}`),
`import`/`foreign import`, métodos, retornos múltiplos, `Option`/`?`, `repeat`/`until`,
`break`/`continue`, bitwise (`& | ~ << >>`), `//`, `#`.

**Redox OS** segue fora de escopo — compilar para Linux nativo.

---

## Roadmap atualizado

| Fase | Escopo | Estado |
|---|---|---|
| **0. Hello world** | pipeline completo, subconjunto mínimo | ✅ **Concluída** |
| **1. Núcleo da linguagem** | int/float/bool, aritmética, `if`/`while`/`for`, funções | ✅ **Concluída** |
| 2. Tipos compostos | arrays, maps, records, strings dinâmicas — ownership/borrow mordem aqui | Pendente |
| 3. Capability Runtimes | AI, Crypto, Data (`titan-ai`, `titan-crypto`…) | Pendente |
| 4. Self-hosting / LSP | compilador escrito na própria linguagem | Pendente |

---
---

# PRD — Titan-Rust · Fase 2: Tipos compostos

> Continuação da Fase 1 (T10–T18, **concluída**). Objetivo da fase:
> `titanc examples/compostos.titan && ./compostos` cria e manipula arrays,
> records e maps — inclusive um array ordenado in-place por uma função.

## Resumo executivo

A Fase 2 é a que o roadmap sempre marcou como **alta complexidade**: é onde a
escolha do modelo de memória em Rust deixa de poder ser adiada. A Fase 0 foi
desenhada para não fechar essa porta, isolando o mapeamento de tipos em
`rust_type_name`/`rust_param_type_name` (`codegen.rs:617,634`).

**Ponto de partida favorável:** a `ast.rs` já é completa — `ExpInitList`, `Field`,
`VarBracket`, `VarDot`, `TopLevelRecord`, `TypeMap`, `TypeName` já existem. Como
nas fases anteriores, a fase não adiciona nós à AST bruta (a única exceção é o
campo `Field.name`, T27, que ganha fidelidade ao original). O `types.rs` também
já tem `Array`, `Map`, `Record` e `Option` completos.

**Referências vivas no Titan original** (`titan/`, somente leitura): decisão
array/map/record de `ExpInitList` em `checker.lua:646-662` · indexação em
`checker.lua:541-564` · records em `checker.lua:1441-1461` · `#` em
`checker.lua:852-860` · representação de memória em `coder.lua:474-488`.

**Decisões fixadas (confirmadas com o usuário):**

1. **Semântica de valor**: `{integer}` → `Vec<i64>`, record → `struct` Rust
   própria; `local b = a` emite `let b = a.clone();`. Sem `Rc`, sem `RefCell`,
   sem aliasing. Diverge do original (que aliasa) e do Rust idiomático (que
   moveria). Custo O(n) aceito conscientemente.
2. **Escopo**: arrays + records + maps. Fora: `Option`/`?`, cast `as`, métodos,
   `import`, multi-assign, retornos múltiplos, bitwise.
3. **`v[i]` tem tipo `T`** (não `T?` como o original), com checagem de faixa no
   `titan-runtime` e mensagem em português — nunca o panic cru do Rust.
4. **Parâmetros compostos por `&mut`** — decisão independente da 1: clonar na
   atribuição não obriga a clonar na passagem. É o que faz
   `selection_sort(xs: {integer}): nil` (o idioma central da referência,
   `titan/testfiles/selection_sort.titan`) funcionar de verdade; por valor, ele
   ordenaria uma cópia e o chamador não veria nada — bug silencioso.
5. **Escrever em `#v + 1` faz append**; qualquer outro índice fora da faixa
   aborta com mensagem clara. Não replica o crescimento livre do original, que
   exigiria inventar valores default (impossível para `{Ponto}`).

**Decisões técnicas derivadas:**

6. **`string` passa a ser sempre `String`**, em toda posição — colapsa a
   dualidade `&str`/`String` hoje espalhada por 5 funções do codegen
   (`:403,492,566,589,607`). A alternativa (anotar `Owned`/`Borrowed` no
   `TypedExp`) foi rejeitada: propagaria a dualidade para dentro de
   `Vec<String>` e dos campos de record, multiplicando os casos.
7. **`Array`/`Map` viram invariantes em `compatible`** (`types.rs:80`) — a
   covariância atual (`{value}` aceita `{integer}`) é *unsound* com arrays
   mutáveis passados por `&mut`.
8. **Construções que o rustc recusaria em inglês passam a ser rejeitadas pelo
   checker, em português**: `f(xs, xs)` (duplo empréstimo), record recursivo,
   nome de record colidindo com tipo do Rust, chave de map `float`.

**Convenções de trabalho** (herdadas, seguem valendo):
- `titan/` e `lua/` são **somente leitura**.
- Cada tarefa termina com `cargo test` verde antes da seguinte.
- Mensagens de erro do compilador em português — nunca panic.

**Grafo de dependências:**
`T19` → `T20`,`T21` → `T22` → `T23` → `T24` → `T25`,`T26` → `T27` → `T28` →
`T29` → `T30` → `T31` → `T32` → `T33`.
(T20/T21 e T25/T26 são paralelizáveis entre si.)

**Skills transversais:** `rust-pro` · `clean-code` · `test-driven-development` ·
`verification-before-completion`

---

## T19 — Restaurar `examples/nucleo.titan` e reverdear a baseline

**Objetivo:** a Fase 2 não começa sobre teste vermelho.

**Contexto:** `cargo test` **não está verde** — 1 de 143 testes falha. O commit
`e7bf378` (T18) alterou `examples/nucleo.titan` de forma que quebra os
algoritmos: além de `fatorial(5)`→`fatorial(50)` e `fibonacci(10)`→`fibonacci(100)`,
mudou os **casos-base** de `n <= 1` para `n <= 10` nas duas funções. O resultado
estoura o `i64` e produz `Fatorial de 50: -3258495067890909184`, enquanto
`crates/titanc/tests/integration.rs:110` espera `Fatorial de 5: 120`. Não é bug
do compilador.

**Detalhes:**
- `git show f617127:examples/nucleo.titan > examples/nucleo.titan`.
- Conferir se `README.md` ou `docs/` citam os valores alterados; se sim, alinhar.
- Registrar em uma linha que o T18 introduziu a regressão — o histórico importa.

**Critério de aceite:** `cargo test` verde (143/143).
`./target/release/titanc examples/nucleo.titan && ./nucleo` imprime
`Fatorial de 5: 120` / `Fibonacci de 10: 55`, exit 0.

**Depende de:** nada. **É pré-requisito de toda a fase.**

**Skills:** `verification-before-completion`

---

## T20 — `lexer.rs`: `[` `]` `.` `#` e keywords `record`/`as`

**Objetivo:** os tokens que arrays, records e maps exigem, sem quebrar long strings.

**Detalhes:**
- Novos `TokenKind`: `LBracket`, `RBracket`, `Dot`, `Hash`, `KwRecord`, `KwAs`.
- `lex_name_or_keyword` (`lexer.rs:433`) ganha `record` e `as`. **Atenção:** `as`
  deixa de ser identificador válido — é reservada no original, mas registrar a
  quebra compatível.
- `.` isolado → `Dot`: o braço `'.' if self.peek2() == Some('.')` (que produz
  `Concat`) ganha um `else`. Não afeta `lex_number`, que já testa
  `peek() == Some('.') && peek2() != Some('.')`.
- **Armadilha do `long_bracket_level`** (`lexer.rs:199,500`): hoje qualquer `[`
  seguido de `[` ou `=*[` vira long string **antes** de virar `LBracket`. A boa
  notícia é que **`a[b[1]]` não colide** — o `[b` faz `long_bracket_level`
  devolver `None`. O único caso ambíguo é `a[[b]]`, que nenhum programa Titan
  válido escreve. **Manter a precedência atual** (long string ganha, como no
  Lua) e documentar com teste.

**Critério de aceite:** testes de tokenização para `v[1]`, `p.campo`, `#v`,
`record Ponto`, `x as integer`, `a[b[1]]` (indexação aninhada, 5 tokens), e o
teste que documenta `a[[b]]` como long string. Ajustar os trechos esperados de
`integration.rs:288,353` (sem `#[ignore]`).

**Depende de:** T19. **Paralela a T21.**

**Skills:** `rust-pro` · `test-driven-development` · `clean-code`

---

## T21 — `types.rs`: variância invariante e braços faltantes

**Objetivo:** fechar o buraco de soundness antes que arrays mutáveis o explorem.

**Contexto:** `compatible` (`types.rs:73`) hoje é covariante em `Array`
(`:80`), fazendo `{value}` aceitar `{integer}`. Com arrays mutáveis passados por
`&mut` (decisão 4), isso é *unsound* da forma clássica: escrever uma `string`
através de uma referência `{value}` que aponta para um `{integer}`.

**Detalhes:**
- Braços `Array` e `Map` passam de `compatible` recursivo para `equals`.
- `Record` (nominal, via `equals`) e `Option` (invariante) ganham braço
  explícito, saindo do `_ => false` — a intenção passa a estar escrita.
- `Value` continua compatível com tudo **no topo** da função: a invariância é só
  *dentro* do composto.
- Atualizar `arrays_compativeis_por_elemento_compativel` (`types.rs:175`), que
  hoje **afirma** a covariância, com comentário apontando o ADR 0008.

**Critério de aceite:** `{value}` não aceita `{integer}` nem vice-versa;
`{integer}` aceita `{integer}`; `Record{"P"}` aceita `Record{"P"}` e recusa
`Record{"Q"}`; `value` segue compatível com `{integer}`.

**Depende de:** T19. **Paralela a T20.**

**Skills:** `rust-pro` · `test-driven-development`

---

## T22 — `parser.rs`: `parse_type` completo (map, `TypeName`, `value`)

**Objetivo:** anotações de tipo dos compostos, e correção de um bug latente.

**Detalhes:**
- **Bug latente**: `TokenKind::KwValue` existe no lexer, mas `parse_type`
  (`parser.rs:242`) não tem braço para ele — `local x: value` dá "Esperava um
  tipo". Acrescentar `KwValue` → `TypeValue`. (O `resolve_type` já o mapeia; a
  rejeição passa a ser do checker, em T25.)
- `TokenKind::Name(_)` → `Type::TypeName { loc, name }` — é assim que um record é
  referenciado; **não existe `TypeRecord`**.
- **`{T}` vs `{K: V}`**: ambos começam com `{`. O original usa backtrack de PEG
  (tenta Map, depois Array); aqui, sendo descida recursiva determinística:
  consumir `{`, parsear um tipo, e **olhar o próximo token** — `:` → map;
  `}` → array; outra coisa → erro claro. Sem keyword `map`.
- `?` (option) **não** entra — segue fora de escopo, com erro léxico claro.
- Atualizar a mensagem do braço `_` para listar as formas novas.

**Critério de aceite:** parseiam `{integer}`, `{{integer}}`, `{string: integer}`,
`Ponto`, `value`; erro claro para `{integer:}`, `{: integer}`, `{integer`.

**Depende de:** T20, T21.

**Skills:** `rust-pro` · `test-driven-development` · `error-handling-patterns`

---

## T23 — `parser.rs`: loop de sufixos (`[`, `.`, `(`) e `record` no topo

**Objetivo:** o ponto central da sintaxe — `v[i]`, `p.campo` e a declaração de
record.

**Detalhes:**
- **`parse_suffixed_exp` (`parser.rs:658`)** — hoje um `while self.check(&LParen)`
  que só trata chamada. Vira um loop de três sufixos:
  `(` → `ExpCall` (como hoje) · `[` → `VarBracket` (espera `]`) ·
  `.` → `VarDot` (espera `Name`). `VarBracket`/`VarDot` são embrulhados em
  `ExpVar` para poderem seguir sendo sufixados (`a[1].campo[2]`).
- **`parse_stat_assign` (`:415`) não muda** — já aceita qualquer `Exp::ExpVar` e
  extrai o `Var`, então `v[i] = x` e `p.campo = x` passam de graça.
- **`parse_toplevel` (`:128`)** ganha `KwRecord` → `parse_toplevel_record`,
  produzindo `TopLevelRecord { loc, name, fields: Vec<Decl> }` (campos são
  `Decl`, reusando `parse_decl`, até `end`; `;` opcional entre campos).
- **Não replicar o desaçúcar do original** (`parser.lua:215-229`, que gera um
  `TopLevelStatic` sintético `Nome.new`): métodos estáticos estão fora do escopo,
  e implementá-los só para o construtor traria um caso especial que nada mais
  usa. Comentário no código apontando o ADR 0009.

**Critério de aceite:** parseiam `v[1]`, `v[i+1]`, `a[1][2]`, `p.x`, `p.a.b`,
`f()[1].c`, `v[1] = 2`, `p.x = 3`, `record Ponto x: float y: float end`. Erro
claro para `v[]`, `v[1`, `p.`, `record end`, `record P x end`. Testes **de
parser** (AST montada) — o checker ainda rejeita tudo isso.

**Depende de:** T22.

**Skills:** `rust-pro` · `test-driven-development` · `error-handling-patterns`

---

## T24 — `string` é sempre `String`: fim da dualidade `&str`/`String`

**Objetivo:** pagar a dívida técnica **antes** que strings dentro de compostos a
tornem insustentável.

**Contexto:** hoje a dualidade está espalhada por cinco funções acopladas —
`emit_slot_value` (`:403`), `str_ord_operand` (`:492`), `concat_operand` (`:566`),
`coerce_to_borrowed_str` (`:589`), `is_owned_string_expr` (`:607`) — e o próprio
doc-comment em `:598-606` admite que o checker não anota o "nascimento" da
variável, então trata todo `Var: string` como dono e coage sempre. Com
`Vec<String>` e campos de record isso não escala.

**Detalhes:**
- `Type::String` → `"String"` em **toda** posição; apagar o caso especial de
  `rust_param_type_name` (`:634`), que pode deixar de existir.
- Colapsar as 5 funções em ≤2: uma que decide `.clone()` e outra que decide `&`
  na fronteira do runtime. Comparação de ordem passa a funcionar direto
  (`String: PartialOrd<String>`), eliminando `str_ord_operand`.
- **`ENTRY_SHIM` (`:47`)**: passar `&mut args`, e `main(args: {string})` vira
  `&mut Vec<String>` — elimina a última exceção `&[String]` de
  `rust_type_name:624`, deixando o mapeamento com zero casos especiais.
- Custo aceito: `f("literal")` passa a alocar. É o mesmo O(n) que a decisão 1 já
  aceitou, em troca de eliminar uma classe inteira de bugs.

**Critério de aceite:** `hello.titan` e `nucleo.titan` seguem compilando com a
saída correta (regressão ponta a ponta); comparação `s1 < s2` entre duas
variáveis funciona; Rust gerado **sem warnings**; contagem de funções de string
no codegen cai de 5 para ≤2.

**Depende de:** T23. **É pré-requisito duro de T26** — `Vec<&str>` vs
`Vec<String>` é o pântano que a fase inteira tenta evitar.

**Skills:** `rust-pro` · `clean-code` · `architect-review`

---

## T25 — `checker.rs`: tabela de builtins e refatoração da AST tipada

**Objetivo:** preparar as estruturas que o resto da fase preenche — sem ainda
aceitar construção nova. Tarefa **puramente estrutural**.

**Detalhes (A — builtins):**
- Novo `crates/titanc/src/builtins.rs` (declarado em `main.rs`; o `titanc` é
  crate binário puro, sem `lib.rs`): `struct Builtin { titan_name, rust_path,
  params, rettype }`, `const BUILTINS`, `fn lookup`.
- `Checker::new` (`checker.rs:312`) itera `BUILTINS` em vez do `print`
  hard-coded; `emit_call` (`codegen.rs:573`) consulta `lookup` em vez do
  `if callee == "print"`. Hoje as duas pontas conhecem `print` de forma
  independente e não sincronizada.

**Detalhes (B — AST tipada):**
- `TypedTopLevel` (`:134`) ganha `Record { loc, name, fields }` — **isso quebra
  `codegen.rs:61`** (o `let ... = top;` irrefutável), que é exatamente o
  mecanismo desejado: falha em tempo de compilação.
- `TypedStat::Assign { name: String }` (`:195`) vira
  `Assign { target: TypedLValue, value }`, com
  `enum TypedLValue { Name, Index, Field }`.
- `TypedExpKind` (`:224`) ganha `Index`, `Field`, `ArrayLit`, `RecordLit`,
  `MapLit` — três nós de literal distintos, não um `InitList` genérico: o checker
  já desambiguou, e o codegen ganha `match` exaustivo.
- `UnOp` (`:288`) ganha `Len`.
- `struct Checker` (`:296`) ganha `records: HashMap<String, Type>` — não há
  tabela de tipos nomeados hoje.
- **Rejeitar `Type::Value` explicitamente**: T22 fez `value` chegar ao checker, e
  o codegen não sabe emiti-lo — cairia no `unreachable!` de `rust_type_name:625`,
  que é panic e violaria a convenção.
- Atualizar `fixup_mutability` (`:1302`) e
  `collect_referenced_names_stat`/`_exp` (`codegen.rs:115,162`) para os braços
  novos — o `match` exaustivo faz o compilador listar tudo.

**Critério de aceite:** compila; **nenhum comportamento muda** (todos os testes
negativos existentes seguem passando). Se algum teste de comportamento mudar,
algo saiu do escopo.

**Depende de:** T24. **Paralela a T26.**

**Skills:** `rust-pro` · `architect-review` · `clean-code`

---

## T26 — `titan-runtime`: superfície de arrays, records e maps

**Objetivo:** a indexação checada da decisão 3, com mensagens em português.

**Contexto:** o runtime tem hoje 58 linhas e duas funções (`print`, `concat`).
Toda a superfície da Fase 2 nasce aqui.

**Detalhes:**
```rust
array_get / array_get_mut / array_set / array_len
string_len            // `#` sobre string
map_get / map_set
```
- **1-based → 0-based** convertido dentro do runtime (o original é 1-based,
  `coder.lua:1994`), num lugar só.
- **Abortar, não `panic!`**: `fn abortar(msg: &str) -> !` com `eprintln!` em
  português + `std::process::exit(1)`. Satisfaz "nunca panic cru em inglês"
  literalmente — sem `thread 'main' panicked`, sem backtrace.
- Mensagens: `"índice 99 fora da faixa (array tem 3 elementos)"` ·
  `"índice 0 inválido: arrays em Titan começam em 1"` ·
  `"índice 5 fora da faixa (array tem 3 elementos; só é possível escrever em
  1..3 ou fazer append em 4)"` · `"chave não encontrada no map"`.
- `array_set` implementa a decisão 5: escreve em `1..#v`, faz **push** em
  `#v + 1`, aborta no resto.
- **Testabilidade sem subprocesso**: expor variantes `*_checked -> Result<_, String>`
  e fazer as versões abortantes um wrapper fino sobre elas.

**Critério de aceite:** testes do runtime cobrindo faixa válida, limite, append
em `#v+1`, índice 0, negativo e além do fim; mensagens conferidas em português.

**Depende de:** T24. **Paralela a T25.**

**Skills:** `rust-pro` · `test-driven-development` · `error-handling-patterns`

---

## T27 — `ast.rs`: `Field.name` vira `FieldName`

**Objetivo:** representar `{["a"] = 1}` antes que o checker se acomode ao
`Option<String>`.

**Contexto:** `Field.name: Option<String>` não representa chave-expressão de map.
No original, `Field.name` é **polimórfico** (`false | string | Exp`,
`ast.lua:77-79`) — então isto não é "adicionar nó à AST", é corrigir a fidelidade
de um campo existente.

**Detalhes:**
```rust
pub enum FieldName {
    None,           // posicional → array
    Name(String),   // campo de record
    Key(Box<Exp>),  // chave de map — cobre {["a"] = 1}
}
```
Mudança mecânica: o parser só constrói, o checker só lê.

**Critério de aceite:** `{1,2}`, `{x=1}` e `{["a"]=1}` são representáveis e
distinguíveis.

**Depende de:** T25, T26.

**Skills:** `rust-pro` · `clean-code`

---

## T28 — `parser.rs`: `ExpInitList` (literais de array, record e map)

**Objetivo:** o `{` em posição de expressão.

**Detalhes:**
- `parse_simple_exp` (`parser.rs:618`) ganha braço `TokenKind::LCurly` — hoje
  ausente, produzindo `"Esperava uma expressão."` (é o que `integration.rs:291`
  espera).
- Campo decidido por lookahead: `[` exp `]` `=` exp → `FieldName::Key` ·
  `Name` `=` exp → `FieldName::Name` (**lookahead de 2**: `Name` seguido de `=`;
  senão é expressão que começa com nome) · exp → `FieldName::None`.
- Separadores `,` e `;`, vírgula final opcional, `{}` vazio válido.
- **O parser não decide** se é array/map/record — a desambiguação é semântica
  (T29), como no original (`checker.lua:646-662`), porque `{}` vazio só se
  resolve por contexto.

**Critério de aceite:** parseiam `{}`, `{1,2,3}`, `{1,2,}`, `{x=1, y=2}`,
`{["a"]=1}`, `{{1},{2}}`, e o misto `{1, x=2}` (parseia; o checker rejeita). Erro
claro para `{1,,2}`, `{x=}`, `{1` sem fechar.

**Depende de:** T27.

**Skills:** `rust-pro` · `test-driven-development` · `error-handling-patterns`

---

## T29 — `checker.rs`: o núcleo semântico

**Objetivo:** tipar tudo que T20–T28 destravaram. **Maior tarefa da fase.**

**Detalhes — passada 1 (`collect_signature`, `:373`):**
- Trocar a rejeição de `TopLevelRecord` por registro em `self.records` como
  `Type::Record { name, fields }`.
- **Vira duas sub-passadas**: records primeiro, funções depois — uma função pode
  receber `Ponto`.
- Rejeitar com erro claro: nome de record duplicado, campo duplicado, campo sem
  tipo, **nome reservado do Rust** (`String`, `Vec`, `Option`, `Box`, `Result`,
  `HashMap`), e **record recursivo** (`record No prox: No end` seria
  infinitamente grande em Rust sem `Box`; sem essa checagem o rustc recusa em
  inglês).

**Detalhes — `resolve_type`:**
- `TypeMap` (`:427`) → `Type::Map`, validando a chave: só `integer`, `string` e
  `boolean`. O `HashMap` do Rust exige `Eq + Hash`, que `f64` não tem e que
  `Vec`/struct não derivam nesta fase. O original já proíbe chave `nil`/option
  (`checker.lua:118`).
- `TypeName` (`:442`) → consulta `self.records`; não achou → `"tipo 'X'
  desconhecido."` (mensagem que já existe, agora com significado real).

**Detalhes — `ExpInitList` (`:983`)**, espelhando `checker.lua:646-662`:
1. **Contexto primeiro** (anotação de `local`, tipo de parâmetro, de retorno, de
   campo) decide array/map/record.
2. Sem contexto → forma do 1º campo: `Name` → record, mas **sem contexto não se
   sabe qual** → erro claro; `Key` → map; senão array.
3. `{}` vazio sem contexto → erro claro.
- **Exige propagar `context: Option<&Type>` por `check_exp`** — a refatoração
  mais invasiva da fase (14 call sites), sem alternativa boa: `{}` vazio precisa
  de contexto. Hoje `check_exp(&mut self, exp: &Exp)` não o recebe; no original é
  `checkexp(node, st, errors, context)`.
- **Record exige exaustividade**: todo campo presente, nenhum extra, nenhum
  posicional — erros separados e claros para cada caso.

**Detalhes — `check_var` (`:1199,1203`):**
- `VarBracket` → base `Array` (índice `integer`, resulta `elem`) ou `Map` (índice
  compatível com `keys`, resulta `values`); base `String` → erro claro.
  **Resultado é `T`, não `T?`** (decisão 3).
- `VarDot` → base `Record`; campo inexistente → `"o record 'Ponto' não tem campo
  'z'."`.

**Detalhes — `check_assign` (`:862`) e mutabilidade:**
- Constrói `TypedLValue`; valida o tipo do valor contra o do alvo.
- `root_decl_id(lvalue)` desce a cadeia de índices/campos até o `VarName` base e
  alimenta o `HashSet<DeclId>` existente — `v[i]=x` e `p.campo=x` marcam a
  **variável-raiz** como mut. Cobre aninhamento (`m[i].campo[j] = x` marca `m`).
- `SymbolKind::Param` **composto** passa a aceitar `xs[i] = v` (é `&mut`);
  escalar segue rejeitado (`:881`). Distinguir consultando o tipo
  (`is_composite`), sem inflar `SymbolKind`.
- Atribuição ao parâmetro composto **inteiro** (`xs = {}`) → erro claro.

**Detalhes — `check_unop` (`:1137`) e `check_call`:**
- `#` (`UnOp::Len`) sobre `Array` ou `String` → `Integer`. **Erro sobre `Map`** —
  o original também proíbe (`checker.lua:855`).
- `check_call`: rejeitar `f(xs, xs)` (duplo empréstimo mutável, que o rustc
  recusaria em inglês) **e** inserir os `DeclId` de argumentos compostos no
  `assigned` — passar array a função é **uso mutável** sob `&mut`, e sem isso o
  Rust gerado não compila (`cannot borrow as mutable`).

**Critério de aceite:** ~20 testes cobrindo aceitação (array literal, indexação,
`#`, record completo, map, `{{integer}}`, record contendo array) e rejeição com
mensagem conferida (record incompleto, campo extra, campo inexistente, índice
não-integer, `#` de map, chave de map float, record recursivo, nome reservado,
`f(xs,xs)`, `{}` sem contexto, record sem contexto).

**Depende de:** T28.

**Skills:** `rust-pro` · `test-driven-development` · `architect-review` ·
`error-handling-patterns`

---

## T30 — `codegen.rs`: emissão de arrays, records e maps

**Objetivo:** traduzir tudo que T29 tipou, sem um único warning no Rust gerado.

**Detalhes:**
- **`rust_type_name` (`:617`) preenchido ANTES de qualquer teste ponta a ponta** —
  `{integer}` hoje cai no `unreachable!` da linha 625, que é panic e violaria a
  convenção. `Array{elem}` → `Vec<T>` (o caso especial `&[String]` sai, T24 já
  mudou o shim) · `Map{k,v}` → `std::collections::HashMap<K, V>` ·
  `Record{name}` → `Nome`. Garantir por teste que todo `Type` construível pelo
  checker tem braço aqui.
- **Structs de record**: `generate` (`:31`) ganha um laço prévio emitindo todas
  as `TypedTopLevel::Record` antes das funções, com
  `#[derive(Clone, Debug, PartialEq)]` e campos `pub`. `Clone` é obrigatório
  (decisão 1); `Copy` **nunca** (records podem conter `String`/`Vec`). Sem
  mangling no nome do tipo — o namespace de tipos do Rust não colide com o
  `fn main` do shim (ADR 0009).
- **`emit_toplevel` (`:60`)**: parâmetro composto → `&mut Vec<T>` / `&mut Nome` /
  `&mut HashMap<K,V>`. Conserta o `match` quebrado por T25, tratando `Record`.
- **Clone (decisão 1)**: uma única função `precisa_clone(exp)` centraliza a
  regra — `Var`/`Index`/`Field` de tipo composto ou string ganham `.clone()`;
  literais, chamadas e construtores passam direto (já são donos).
- **Argumento composto**: `&mut expr`; quando já é parâmetro, o reborrow
  implícito do Rust cobre (emitir o nome cru).
- **Indexação**: leitura via `array_get` (deref para escalar, `.clone()` para
  composto/string); escrita via `array_set`. `#` → `array_len`/`string_len`.
- **Literais**: `vec![a, b, c]` · `Nome { x: .., y: .. }` ·
  `HashMap::from([(k,v), ..])`.

**Critério de aceite** (padrão da fase: `--emit-rust` + execução real via o
helper `compila_e_executa`): array criado, indexado, escrito, `#`; record
construído, campo lido e escrito; map criado e consultado; **função que ordena um
array in-place muda o array do chamador** (prova a decisão 4); **`local b = a;
b[1] = 9` não altera `a`** (prova a decisão 1). Rust gerado **sem warnings**.

**Depende de:** T29, T26.

**Skills:** `rust-pro` · `clean-code` · `test-driven-development` ·
`architect-review`

---

## T31 — Curadoria dos testes de integração

**Objetivo:** reclassificar o que saiu de "fora de escopo" e corrigir uma
asserção que deixa de ser verdadeira.

**Detalhes:**
- `CASOS_FORA_DE_ESCOPO_FASE_1` (`integration.rs:284`) → `..._FASE_2`. **Removem-se**
  `indexacao_de_array` (`:286`), `construtor_de_array` (`:291`) e
  `operador_length` (`:351`) — viram casos **positivos**. **Ajustam-se** os
  trechos de `chamada_de_metodo` e `tipo_option` (com `.` e `[` lexados, as
  mensagens mudam de camada). **Mantêm-se** bitwise, `//`, `import`, `repeat`,
  `break`, retornos múltiplos.
- Acrescentar ~10 negativos novos: `cast_as`, `metodo_com_dois_pontos`,
  `multi_assign`, `nome_de_record_reservado`, `record_construtor_incompleto`,
  `record_campo_extra`, `map_com_chave_float`, `duplo_emprestimo` (`f(xs,xs)`),
  `length_de_map`, `record_recursivo`.
- **`arquivos_reais_do_titan_original_produzem_erro_claro_sem_panic` (`:374`)**
  afirma hoje que *todo* `.titan` real é rejeitado — o que **deixa de ser
  verdade**: `testfiles/sieve.titan` usa exclusivamente arrays, e
  `testfiles/selection_sort.titan` idem. Reescrever a **asserção** para a
  propriedade que sempre importou (e que o nome do teste já enuncia): *compila
  **ou** falha com erro claro; nunca panica, nunca com stderr vazio*. Renomear
  para `..._nunca_panicam`. Acrescentar lista nomeada dos arquivos que a Fase 2
  **espera compilar** — isso transforma o teste de guarda-corpo em medida de
  progresso.

**Critério de aceite:** `cargo test` verde; a suíte reflete o escopo real da
fase; `selection_sort.titan` compila.

**Depende de:** T30.

**Skills:** `test-automator` · `verification-before-completion` · `find-bugs`

---

## T32 — `examples/compostos.titan` e integração ponta a ponta

**Objetivo:** um programa que exercite arrays + records + maps, com teste de
integração real (mesmo padrão do T8/T17).

**Entregável — `examples/compostos.titan`**, cobrindo: record (construção por
contexto, leitura e escrita de campo), array (literal, `#`, indexação, mutação
in-place por função, push via `#res+1`), array de floats, e map.

Duas linhas do programa são as mais importantes da suíte, porque **provam as
decisões da fase**:
- `Original preservado: ...` — depois de `local copia = qs; copia[1] = 999`, o
  `qs` original está intacto (decisão 1, semântica de valor).
- `Primeiro estoque dobrado: ...` — depois de `dobrar_estoque(qs)`, o chamador
  vê a mutação (decisão 4, `&mut`).

**Atenção:** não usar `as` no exemplo — cast está fora de escopo. Onde for
preciso misturar `integer` e `float`, usar a promoção que `numeric_result` já faz
desde a Fase 1.

**Critério de aceite:** teste de integração invoca o `titanc` real, executa o
binário, confere **stdout completo** e exit code 0.

**Verificação:**
```bash
cd /home/leonardo/titan-rust/titan-rust
cargo build --release
./target/release/titanc examples/compostos.titan
./compostos
echo $?          # → 0
./target/release/titanc --emit-rust examples/compostos.titan   # structs, Vec, HashMap, &mut, .clone()
./target/release/titanc examples/hello.titan  && ./hello       # regressão Fase 0
./target/release/titanc examples/nucleo.titan && ./nucleo      # regressão Fase 1
./target/release/titanc ../titan/testfiles/selection_sort.titan  # idioma in-place do original
cargo test
cargo clippy --all-targets -- -D warnings
```

**Depende de:** T31.

**Skills:** `test-automator` · `verification-before-completion` · `find-bugs`

---

## T33 — ADRs, documentação e fechamento da Fase 2

**Objetivo:** deixar as decisões não-óbvias registradas e o projeto compreensível.

**Entregáveis:**
- ADRs `0006`–`0010` no formato Status/Contexto/Decisão/Consequências, seguindo
  `0004-for-desacucarado-para-while.md`:

  | ADR | Decisão |
  |---|---|
  | 0006 | Semântica de valor com clone na atribuição (diverge do aliasing do original) |
  | 0007 | Parâmetros compostos por `&mut`, preservando o idioma in-place |
  | 0008 | Indexação checada no runtime, `T` em vez de `T?`; variância invariante |
  | 0009 | Records como `struct` Rust nominal |
  | 0010 | `string` é sempre `String` — fim da dualidade `&str`/`String` |

- `docs/adr/README.md`: acrescentar as 5 linhas à tabela.
- `README.md`: mover arrays/records/maps/`#`/indexação de "o que não está
  implementado ainda" para o coberto; atualizar o estado para Fase 2.
- `PRD.md`: marcar a Fase 2 como concluída no roadmap.
- `docs/arquitetura.md`: atualizar a tabela de mapeamento de tipos (hoje
  descreve só a Fase 0 e antecipa a escolha do modelo de memória).

**Depende de:** T32.

**Skills:** `readme` · `docs-architect` · `architecture-decision-records`

---

## Revisão de qualidade (contínua)

Como nas fases anteriores: revisão de código ao fim de **T30** e novamente ao fim
de **T32**, antes de seguir.

**Skills:** `code-reviewer` · `architect-review` · `find-bugs`

---

## Riscos da fase

1. **`check_exp` ganhar o parâmetro `context`** (T29) toca 14 call sites — é a
   refatoração mais invasiva e não tem alternativa boa: `{}` vazio precisa de
   contexto para ser tipado.
2. **Marcar `mut` por *uso*, não só por atribuição** (dependência cruzada
   T29↔T30): passar um array a uma função é uso mutável sob `&mut`. Se
   esquecido, o Rust gerado não compila e o erro vem em inglês do rustc.
3. **T24 antes de T26** não é negociável: `Vec<&str>` vs `Vec<String>` é
   exatamente o pântano que a fase tenta evitar.
4. **Toda construção que o rustc rejeitaria em inglês** precisa de rejeição no
   checker, em português. A lista identificada: `f(xs, xs)`, record recursivo,
   nome de record reservado, chave de map `float`. Revisitá-la ao fim de T30.

---

## Fora de escopo nesta fase

Rejeitar com erro claro (nunca panic): `Option`/`?`, cast (`as`), métodos e
chamadas de método, `import`/`foreign import`, retornos múltiplos, multi-assign
(`a, b = ...`), declaração múltipla, `repeat`/`until`, `break`/`continue`,
bitwise (`& | ~ << >>`), `//`.

**Redox OS** segue fora de escopo — compilar para Linux nativo.
