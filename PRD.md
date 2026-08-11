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
