# Arquitetura

## O pipeline

```mermaid
flowchart LR
    src[".titan\n(fonte)"] --> lexer["lexer.rs\nTokens"]
    lexer --> parser["parser.rs\nAST\n(ast.rs)"]
    parser --> checker["checker.rs\nAST tipada"]
    checker --> codegen["codegen.rs\nRust legível"]
    codegen --> driver["driver.rs"]
    driver -->|escreve| proj["build/&lt;nome&gt;/\nsrc/main.rs + Cargo.toml"]
    proj -->|cargo build --release| bin["build/&lt;nome&gt;/target/release/&lt;nome&gt;"]
    bin -->|copia| out["./&lt;nome&gt;\n(executável final)"]

    rt["titan-runtime\n(print, concat)"] -.link em tempo de build.-> proj
```

Cada seta é uma função pura de um módulo para o próximo — `lex(&str) ->
Vec<Token>`, `parse(&[Token]) -> Program`, `check(&Program) -> TypedProgram`,
`generate(&TypedProgram) -> String` — orquestradas por `driver::compile`, que é
quem lê o arquivo, grava `build/<nome>/`, invoca o `cargo` como subprocesso e
copia o binário final. `main.rs` só faz parsing de argumentos da CLI e chama
`driver::compile`.

Todo erro em qualquer etapa (léxico, sintático, de tipo, de I/O, ou falha do
próprio `cargo build`) vira uma variante de `Result`/enum de erro com posição
e mensagem em português — nunca um `panic!`. Isso é deliberado: um compilador
que "quebra" ao ver uma construção não suportada é pior que um que recusa com
uma mensagem clara.

## Por que o backend do Titan não foi reaproveitado

O Titan original (`titan/titan-compiler/coder.lua`, 3182 linhas) gera **C**, e
não C portável qualquer: gera C fortemente acoplado à API **interna** do Lua
5.3. Alguns exemplos do acoplamento:

- `String` no Titan vira `TString*` — a struct interna de strings do Lua,
  gerenciada pelo GC do Lua.
- `Array` vira `Table*` — a mesma struct usada para tabelas Lua.
- **Toda** função gerada recebe um `lua_State *L` como parâmetro implícito,
  porque qualquer alocação passa pelo alocador/GC do Lua.

Não existe uma camada intermediária nesse C que separe "a lógica do programa"
de "como o Lua representa valores na memória" — as duas coisas estão
entrelaçadas em cada linha. Trocar o alvo de C-acoplado-ao-Lua para Rust não é
uma tradução mecânica de sintaxe: é escrever um backend inteiro do zero, para
um modelo de memória completamente diferente (ownership/borrow em vez de GC).
Na prática, a fração reaproveitável desse arquivo para um alvo Rust é ~0%.

Some a isso uma barreira prática: o toolchain do Titan (Lua 5.3.5 + LuaRocks)
não está disponível nesta máquina, então nem seria possível iterar sobre o
compilador original mesmo se o backend fosse portável.

**O que *foi* reaproveitado**, então, é o *desenho*, não o código:

| Etapa do Titan (Lua) | Equivalente aqui (Rust) | O que foi herdado |
|---|---|---|
| `lexer.lua` (LPeg) | `lexer.rs` (varredura manual) | conjunto de tokens, regras de literais/comentários |
| `parser.lua` (PEG) | `parser.rs` (descida recursiva) | gramática, precedência, nomes de nó |
| `ast.lua` | `ast.rs` | os mesmos nomes de variante (`ExpString`, `StatCall`, `TopLevelFunc`...) |
| `types.lua` | `types.rs` | as mesmas variantes de tipo e a relação `compatible` (gradual typing) |
| `checker.lua` + `symtab.lua` | `checker.rs` | duas passadas (assinaturas → corpos), pilha de escopos |
| `coder.lua` | `codegen.rs` | **nada do código** — só a ideia de "uma função por variante de nó" |

Manter os mesmos nomes de nó da AST entre os dois projetos é intencional: o
Titan original continua servindo como referência viva sempre que uma dúvida
de comportamento aparece (por exemplo, "o que `checker.lua:1593` faz quando
`main` tem assinatura errada?").

## Modelo de tipos do código gerado

O mapeamento Titan → Rust está isolado em duas funções de
`crates/titanc/src/codegen.rs` (`rust_type_name` e `rust_param_type_name`),
de propósito — desde a Fase 0 essas duas funções foram desenhadas para conter
qualquer troca de modelo de memória sem se espalhar pelo resto do codegen. A
Fase 2 (arrays, maps, records) foi onde essa escolha deixou de poder ser
adiada; o modelo escolhido é **semântica de valor com `clone()`**, sem
`Rc<RefCell<...>>`, sem arena, sem GC — ver
[ADR 0006](adr/0006-semantica-de-valor-clone-na-atribuicao.md).

| Titan | Rust |
|---|---|
| `integer` | `i64` |
| `float` | `f64` |
| `boolean` | `bool` |
| `string` (qualquer posição) | `String` |
| `nil` (retorno) | `()` |
| `{T}` (array) | `Vec<T>` |
| `{K: V}` (map) | `std::collections::HashMap<K, V>` |
| `record Nome` | `struct Nome` própria, `#[derive(Clone, Debug, PartialEq)]` |
| `{string}` (parâmetro de `main`) | `&mut Vec<String>` |
| parâmetro de função de tipo composto | `&mut T` (array/map/record) |

`print` e `concat` (suporte ao operador `..`) não são geradas pelo compilador
— vêm de `titan-runtime`, um crate Rust comum, referenciado por caminho
absoluto no `Cargo.toml` gerado. O Titan original **não tem `print`**; aqui
ela é stdlib, não palavra-chave. A partir da Fase 2, o `titan-runtime` também
fornece a indexação checada de arrays e maps (`array_get`, `array_get_mut`,
`array_set`, `map_get`) — toda leitura/escrita fora da faixa aborta com
mensagem em português em vez de produzir um valor `T?` como no original (ver
[ADR 0008](adr/0008-indexacao-checada-e-variancia-invariante.md)).

**Cópia vs. referência**, resumido (detalhado nos ADRs 0006–0009):

- `local b = a` com `a` composto → `let b = a.clone();` (cada variável é dona
  da sua cópia).
- `f(a)` com `a` composto → o parâmetro correspondente é `&mut T`; a função
  enxerga e pode mutar o mesmo valor do chamador (preserva o idioma in-place
  de `selection_sort.titan`).
- `string` segue a mesma regra de clone que os demais compostos desde a
  Fase 2 — não há mais distinção entre string "literal" e "computada" no
  codegen ([ADR 0010](adr/0010-string-sempre-string.md)).

## Duas armadilhas do Cargo (por que `driver.rs` faz o que faz)

1. O `Cargo.toml` gerado em `build/<nome>/` leva um `[workspace]` **vazio**.
   Sem isso, o `cargo` tenta anexar esse diretório ao workspace pai
   (`titan-rust/Cargo.toml`) e a build quebra.
2. `titan-runtime` é referenciado por **caminho absoluto** no `Cargo.toml`
   gerado — sem rede, sem registry. O projeto gerado em `build/<nome>/` não é
   parte do workspace do compilador, então um caminho relativo dependeria de
   onde `--out` aponta.
