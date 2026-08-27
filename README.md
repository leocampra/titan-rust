# titan-rust

Compilador, escrito em Rust, para uma linguagem tipada inspirada em Lua que gera
código Rust nativo. O projeto [Titan](../titan) original (escrito em Lua) serve
como **especificação de referência** de gramática, AST e sistema de tipos — não
como base de código. Veja [`PRD.md`](PRD.md) para o plano de tarefas completo e
[`docs/arquitetura.md`](docs/arquitetura.md) para o porquê das decisões abaixo.

Estado atual: **Fase 3 ("Capability Runtimes")** — além do
hello world da Fase 0, do núcleo da linguagem da Fase 1 e dos tipos
compostos da Fase 2, o compilador já cobre `import`, módulos, tipos opacos e
a capability `titan-data` (leitura de CSV e agregações sobre Polars), o
suficiente para levar `examples/dados.titan` até um executável nativo.

## Como compilar

Da raiz do workspace (`titan-rust/`):

```bash
cargo build --release
```

Isso produz `target/release/titanc`.

## Como rodar o hello world

```bash
./target/release/titanc examples/hello.titan
./hello
# → Olá, mundo!
echo $?
# → 0
```

## Como rodar o exemplo da Fase 1 (núcleo da linguagem)

```bash
./target/release/titanc examples/nucleo.titan
./nucleo
# → Fatorial de 5: 120
# → Fibonacci de 10: 55
echo $?
# → 0
```

## Como rodar o exemplo da Fase 2 (tipos compostos)

```bash
./target/release/titanc examples/compostos.titan
./compostos
echo $?
# → 0
```

`examples/compostos.titan` exercita record (construção, leitura e escrita de
campo), array (literal, `#`, indexação, mutação in-place por função, push via
`#res+1`), array de floats e map — inclusive as duas provas centrais da fase:
`local copia = qs; copia[1] = 999` não altera `qs` (semântica de valor,
[ADR 0006](docs/adr/0006-semantica-de-valor-clone-na-atribuicao.md)), e
`dobrar_estoque(qs)` muda o `qs` do chamador (parâmetros compostos por
`&mut`, [ADR 0007](docs/adr/0007-parametros-compostos-por-mut.md)).

## Como rodar o exemplo da Fase 3 (capability runtime `titan-data`)

```bash
./target/release/titanc examples/dados.titan
./dados
echo $?
# → 0
```

`examples/dados.titan` importa a capability `data` (`import data`), lê
`examples/vendas.csv`, imprime dimensões e colunas, extrai uma coluna como
array Titan (exercitando a Fase 2 sobre o resultado) e agrega uma coluna
**pelas duas formas equivalentes** — `data.soma(df, "valor")` (função de
módulo) e `df.soma("valor")` (método sobre o tipo opaco
`data.DataFrame`, [ADR 0014](docs/adr/0014-metodo-com-ponto-nao-dois-pontos.md)).

> **Custo de build/disco:** este é o único exemplo que invoca o `cargo
> build --release` sobre uma dependência do Polars — leva **~2 minutos** e
> deixa **~3GB** em `build/dados/target/` (medido antes da Fase 3 começar).
> Como o `titanc` gera um projeto Cargo por programa compilado
> (`build/<nome>/`), esse custo é por programa, não uma vez só — um
> programa sem `import data` nunca paga esse preço
> ([ADR 0015](docs/adr/0015-api-data-como-contrato-backend-trocavel.md)).

O `titanc` lê o `.titan`, gera um projeto Cargo temporário em `build/<nome>/`,
compila-o com `cargo build --release` e copia o binário resultante para o
diretório atual como `<nome>`.

Para inspecionar o Rust gerado sem compilar:

```bash
./target/release/titanc --emit-rust examples/nucleo.titan
```

### CLI

```
titanc [--emit-rust] [--out DIR] [-v] <arquivo.titan>
```

- `--emit-rust` — imprime o Rust gerado e para, sem invocar o `cargo`.
- `--out DIR` — diretório onde `build/<nome>/` é criado e onde o executável
  final é copiado (default: diretório atual).
- `-v` — mostra a invocação do `cargo build --release`.

> O `titanc` **não** é instalado no PATH global nesta fase — invoque sempre
> pelo caminho explícito (`./target/release/titanc` ou
> `./target/debug/titanc`).

## Testes

```bash
cargo test
```

Cobre as unidades de cada etapa do pipeline (lexer, parser, checker, codegen,
driver) e um teste de integração que invoca o binário `titanc` de verdade,
conferindo stdout e exit code do executável gerado, além de uma suíte de casos
negativos (erro claro, nunca panic).

## Relação com o Titan original

O [Titan](../titan) (`titan/`, escrito em Lua) já tem lexer, parser, AST,
checker e symbol table para uma linguagem muito parecida com esta. O
`titan-rust` **reaproveita o desenho** dessas etapas — mesma gramática, mesmos
nomes de nó de AST (`ExpString`, `StatCall`, `TopLevelFunc`...), mesma
estratégia de verificação de tipos em duas passadas — mas é uma
**implementação nova, em Rust, do zero**. Nada do código Lua é executado ou
transpilado; o Titan serve apenas como referência viva para conferir se o
comportamento bate.

O que **não** foi reaproveitado, e por quê, está em
[`docs/arquitetura.md`](docs/arquitetura.md) e em
[`docs/adr/0001-compilador-novo-em-rust.md`](docs/adr/0001-compilador-novo-em-rust.md).
Duas decisões da Fase 1 divergem deliberadamente do comportamento do Titan
original — o `for` numérico desaçucarado para `while`
([ADR 0004](docs/adr/0004-for-desacucarado-para-while.md)) e `and`/`or`
exigindo booleano estrito em vez de truthy/falsy
([ADR 0005](docs/adr/0005-and-or-boolean-estrito.md)). A Fase 2 soma mais
cinco: semântica de valor com `clone()` em vez de aliasing
([ADR 0006](docs/adr/0006-semantica-de-valor-clone-na-atribuicao.md)),
parâmetros compostos por `&mut` para preservar o idioma in-place
([ADR 0007](docs/adr/0007-parametros-compostos-por-mut.md)), indexação
checada no runtime com `T` em vez de `T?`
([ADR 0008](docs/adr/0008-indexacao-checada-e-variancia-invariante.md)),
records como `struct` nominal
([ADR 0009](docs/adr/0009-records-como-struct-rust-nominal.md)) e `string`
sempre `String`
([ADR 0010](docs/adr/0010-string-sempre-string.md)). A Fase 3 soma mais
cinco: `import data` como declaração de topo fixa, sem alias
([ADR 0011](docs/adr/0011-import-como-acucar-sintatico.md)), módulo como
`SymbolKind` em vez de `Type`
([ADR 0012](docs/adr/0012-modulo-como-symbolkind-nao-tipo.md)), tipo opaco
composto por herança de `is_composite`
([ADR 0013](docs/adr/0013-tipo-opaco-composto-por-heranca.md)), método
chamado com `.` em vez de `:`
([ADR 0014](docs/adr/0014-metodo-com-ponto-nao-dois-pontos.md)) e a API
`data.*` como contrato estável sobre um backend (Polars) trocável
([ADR 0015](docs/adr/0015-api-data-como-contrato-backend-trocavel.md)).

`titan/` e `lua/` (usado para checar comportamento de referência do Lua) são
**somente leitura** neste repositório — repositórios de terceiros, nunca
editados.

## O que já está implementado

Fase 0 (hello world) + Fase 1 (núcleo da linguagem) + Fase 2 (tipos
compostos) + Fase 3 (capability runtimes):

- `function`/`local function`, `local x [: T] = exp`, `return`.
- Operadores aritméticos `+ - * / % ^`, relacionais `== ~= < > <= >=`,
  lógicos `and or not`, unário `-`/`not`; `..` (concatenação, coage
  número→string).
- Controle de fluxo: `if`/`elseif`/`else`, `while`, `for` numérico
  (`for x = start, finish[, inc] do ... end`).
- Atribuição single-target: `nome = exp` para local já declarada, incluindo
  `v[i] = x` e `p.campo = x`.
- Tipos compostos: `array` (`{T}`, literal, indexação `v[i]`, `#v`, mutação
  in-place por função via `&mut`), `record` (declaração `record Nome ... end`,
  literal exaustivo, leitura/escrita de campo `p.campo`) e `map`
  (`{K: V}`, `map_get`/`map_set` via indexação).
- `import data` (declaração de topo, sem alias) e namespaces de módulo
  (`data.read_csv(...)`, `data.DataFrame`).
- Tipos opacos de capability (`data.DataFrame`) e métodos com ponto sobre
  eles (`df.soma("valor")`, açúcar da forma de módulo
  `data.soma(df, "valor")`).
- Capability runtime `titan-data`: leitura de CSV (`data.read_csv`),
  inspeção (`linhas`, `colunas`, `coluna_integer`/`coluna_float`) e
  agregação (`soma`, `media`, `minimo`, `maximo`) sobre Polars.

## O que não está implementado ainda

Ficam para fases futuras (veja o roadmap no [`PRD.md`](PRD.md)):

- `repeat`/`until`, `break`/`continue`, `for`-in.
- Retornos múltiplos, multi-assign (`a, b = ...`).
- Bitwise (`& | ~ << >>`), `//`.
- `foreign import`, `import` com alias (`import data as d`),
  `local m = import "data"` (a forma do original), módulos definidos pelo
  usuário (um `.titan` importando outro `.titan`).
- Chamada de método com dois-pontos (`df:soma()`, forma do original — aqui
  só `.`).
- `Option`/`?` e cast de tipo (`as`).
- `titan-crypto`, `titan-ai` (fases 3b/3c) e self-hosting (fase 4).

Qualquer construção fora desse subconjunto é rejeitada pelo `checker` (ou
pelo `parser`, quando a sintaxe já é o problema) com uma mensagem de erro em
português, nunca com um panic.

## Estrutura do workspace

```
titan-rust/
├── crates/
│   ├── titanc/          # o compilador: lexer, parser, checker, codegen, driver, CLI
│   ├── titan-runtime/   # runtime mínimo (print, concat) chamado pelo Rust gerado
│   └── titan-data/      # capability `data`: leitura de CSV e agregações sobre Polars
├── examples/
│   ├── hello.titan
│   ├── nucleo.titan
│   ├── compostos.titan
│   ├── dados.titan
│   └── vendas.csv
├── docs/
│   ├── arquitetura.md
│   └── adr/
└── build/                # gerado pelo titanc, não versionado
```
