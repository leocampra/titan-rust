# ADR 0001 — Compilador novo, escrito em Rust, do zero

## Status

Aceito.

## Contexto

O projeto parte da ideia de uma linguagem inspirada em Lua que compila para
Rust nativo em vez de depender de uma VM. O [Titan](../../../titan) já existe
como uma linguagem tipada derivada do Lua, com lexer, parser, AST, checker e
symbol table prontos (`titan/titan-compiler/`) — a pergunta inicial era se
esse código poderia ser a base do novo compilador, trocando só o backend.

O backend do Titan (`titan/titan-compiler/coder.lua`, 3182 linhas) gera C
fortemente acoplado à API **interna** do Lua 5.3: `String` vira `TString*`,
`Array` vira `Table*`, e toda função gerada recebe um `lua_State *L`, porque
toda alocação passa pelo alocador/GC do Lua. Não há uma camada intermediária
nesse C que separe a lógica do programa da representação de valores em
memória do Lua — as duas coisas estão entrelaçadas linha a linha. Além disso,
o toolchain do Titan (Lua 5.3.5 + LuaRocks) não está disponível nesta
máquina.

## Decisão

Construir um compilador **novo, escrito em Rust**, para uma linguagem tipada
inspirada em Lua (`titan-rust`, binário `titanc`). O projeto Titan original
(`titan/`, em Lua) é usado como **especificação de referência** — gramática,
AST, sistema de tipos — mas não como base de código. Nenhuma linha de
`coder.lua` é reaproveitada.

O que *é* reaproveitado é o desenho das etapas anteriores ao backend: os
mesmos nomes de nó de AST (`ExpString`, `StatCall`, `TopLevelFunc`...), a
mesma estrutura de checker em duas passadas, a mesma relação de
`compatible`/`equals` do gradual typing. Ver
[`docs/arquitetura.md`](../arquitetura.md) para a tabela completa de
correspondência.

`titan/` e `lua/` (referência de comportamento do Lua) permanecem no
repositório como dependências **somente leitura** — nunca editadas.

## Consequências

- Nenhuma dependência de Lua/LuaRocks para compilar ou rodar `titan-rust`.
- O ganho de reaproveitar o Titan fica limitado a *desenho*, não a *código* —
  lexer, parser, checker e AST foram reescritos em Rust, não portados
  mecanicamente.
- O Titan original continua útil como oráculo de comportamento durante o
  desenvolvimento ("o que o `checker.lua` original faz nesse caso?"), o que
  justifica manter os mesmos nomes de nó mesmo sem compartilhar código.
- Fica em aberto, para fases futuras, o modelo de memória do Rust gerado
  (`Rc<RefCell<...>>`, arena, ou GC próprio) — decisão que o backend do Titan
  não ajuda a tomar, porque delegava toda alocação ao GC do Lua.
