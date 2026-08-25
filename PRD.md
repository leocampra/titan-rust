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
