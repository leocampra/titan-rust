# titan-rust

Compilador, escrito em Rust, para uma linguagem tipada inspirada em Lua que gera
código Rust nativo. O projeto [Titan](../titan) original (escrito em Lua) serve
como **especificação de referência** de gramática, AST e sistema de tipos — não
como base de código. Veja [`PRD.md`](PRD.md) para o plano de tarefas completo e
[`docs/arquitetura.md`](docs/arquitetura.md) para o porquê das decisões abaixo.

Estado atual: **Fase 0 ("Hello World")** — um subconjunto mínimo da linguagem,
o suficiente para levar `examples/hello.titan` até um executável nativo.

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

O `titanc` lê o `.titan`, gera um projeto Cargo temporário em `build/<nome>/`,
compila-o com `cargo build --release` e copia o binário resultante para o
diretório atual como `<nome>`.

Para inspecionar o Rust gerado sem compilar:

```bash
./target/release/titanc --emit-rust examples/hello.titan
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

`titan/` e `lua/` (usado para checar comportamento de referência do Lua) são
**somente leitura** neste repositório — repositórios de terceiros, nunca
editados.

## O que não está implementado ainda

A Fase 0 cobre só o suficiente para compilar e rodar o hello world. Ficam para
fases futuras (veja o roadmap no [`PRD.md`](PRD.md)):

- Controle de fluxo: `if`, `while`, `for`, `repeat`.
- Operadores aritméticos, relacionais e lógicos; operador unário.
- Retornos múltiplos e atribuição (`x = exp`).
- Tipos compostos manipuláveis: arrays, maps, records; inicializadores `{...}`.
- `import` e `foreign import`.
- Métodos e chamadas de método.
- `Option`/`?` e cast de tipo (`as`).
- Capability runtimes (`titan-ai`, `titan-crypto`, ...) e self-hosting.

Qualquer construção fora desse subconjunto é rejeitada pelo `checker` com uma
mensagem de erro em português, nunca com um panic.

## Estrutura do workspace

```
titan-rust/
├── crates/
│   ├── titanc/          # o compilador: lexer, parser, checker, codegen, driver, CLI
│   └── titan-runtime/   # runtime mínimo (print, concat) chamado pelo Rust gerado
├── examples/
│   └── hello.titan
├── docs/
│   ├── arquitetura.md
│   └── adr/
└── build/                # gerado pelo titanc, não versionado
```
