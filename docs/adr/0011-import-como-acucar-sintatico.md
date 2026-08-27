# ADR 0011 — `import data` como açúcar de `local data = import "data"`

## Status

Aceito.

## Contexto

O Titan original não tem capability runtimes nem o mecanismo de módulos desta
fase — o mais próximo é `foreign import`, que também está fora de escopo
aqui. A Fase 3 precisa de uma forma de trazer um módulo (`data`, e no futuro
`crypto`, `ai`) para o escopo do programa.

Duas formas concorriam pela sintaxe de importação:

1. `local m = import "data"` — `import` como expressão que recebe uma
   **string** com o nome do módulo, resultado atribuível a qualquer nome
   local (`m`), no molde de um `require` genérico.
2. `import data` — `import` como declaração de topo, `data` como **nome**
   (não string), sem alias: o nome local é sempre igual ao nome do módulo.

## Decisão

`import data` é uma declaração de topo (`TopLevel::TopLevelImport`,
`ast.rs:107`), nunca uma expressão. O parser (`parser.rs:164`,
`parse_toplevel_import`) exige um **nome**, não uma string — `import "data"`
é rejeitado com erro claro (`parser.rs:1849`,
`parse_import_com_string_produz_erro_claro`). Não há alias: `import data as
d` também é rejeitado explicitamente (`parser.rs:173`,
`parse_import_com_as_produz_erro_claro`) — o nome local é sempre igual ao
nome do módulo (`localname == modname`, comentário em `parser.rs:161`).

No checker, o módulo importado entra na tabela de símbolos como
`SymbolKind::Module { name }` (`checker.rs:695`), não como uma variável de
tipo comum — ver [ADR 0012](0012-modulo-como-symbolkind-nao-tipo.md) para o
porquê dessa escolha específica.

Esta decisão **diverge do Titan original** (que não tem este mecanismo) e é
mais restrita do que a forma 1 consideraria: sem string, sem alias. A
motivação é a mesma em ambos os cortes — cada restrição elimina uma classe de
pergunta que o checker teria que responder (`import` de um nome que não
existe em tempo de compilação é erro de parser, não de runtime; dois nomes
locais diferentes para o mesmo módulo nunca acontece, então a tabela de
símbolos não precisa reconciliar dois `SymbolKind::Module` apontando para a
mesma capability).

## Consequências

- `import data` é a única forma aceita; `import "data"` e `import data as d`
  são erros de parser claros, nunca panics, e nunca chegam ao checker.
- `foreign import` — a forma do Titan original para trazer módulos C — segue
  fora de escopo desta fase (nunca foi considerada: não há FFI nesta fase).
- Módulos definidos pelo usuário (um `.titan` importando outro `.titan`)
  seguem fora de escopo — `import data` só resolve capabilities internas
  (`titan-data` nesta fase; `titan-crypto`/`titan-ai` nas fases 3b/3c
  reaproveitam o mesmo `import NOME`).
- Testado em `integration.rs` (movido de caso negativo para positivo na T44,
  ver `T44` no `PRD.md`) e exercitado ponta a ponta em
  `examples/dados.titan` (T45).
