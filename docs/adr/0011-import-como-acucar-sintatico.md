# ADR 0011 — `import data` como açúcar de `local data = import "data"`

## Status

Aceito (revisado na T72 — o alias `import data as d` passou a ser aceito;
ver "Revisão" abaixo).

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
`ast.rs`), nunca uma expressão. O parser (`parse_toplevel_import`) exige um
**nome**, não uma string — `import "data"` é rejeitado com erro claro
(`parse_import_com_string_produz_erro_claro`). Sem a cláusula `as`, o nome
local é igual ao nome do módulo (`localname == modname`).

No checker, o módulo importado entra na tabela de símbolos como
`SymbolKind::Module { name }`, não como uma variável de tipo comum — ver
[ADR 0012](0012-modulo-como-symbolkind-nao-tipo.md) para o porquê dessa
escolha específica.

Esta decisão **diverge do Titan original** (que não tem este mecanismo) e é
mais restrita do que a forma 1 consideraria: sem string. A motivação do
corte é eliminar uma classe de pergunta que o checker teria que responder —
`import` de um nome que não existe em tempo de compilação é erro de parser,
não de runtime.

## Revisão (T72) — o alias

A decisão original também recusava o alias (`import data as d`), com a
justificativa de que "dois nomes locais diferentes para o mesmo módulo nunca
acontece, então a tabela de símbolos não precisa reconciliar dois
`SymbolKind::Module` apontando para a mesma capability". A T72 aceitou o
alias, e a justificativa não se sustentou: não havia nada a reconciliar.

`import data as d` é exatamente o mesmo açúcar já descrito por esta ADR,
com `localname != modname` — `local d = import "data"`. `SymbolKind::Module
{ name }` já guardava o nome do módulo separado do nome local desde a T38, e
`Capability::titan_name` já era a fonte do nome real. O que a T72 precisou
ajustar foi só **qual dos dois nomes vai a cada lugar**:

- o **nome local** é o que colide com declarações existentes, o que vira
  símbolo, o que chaveia `Checker::modules` e o que aparece nas mensagens de
  erro (é o que o programa escreveu);
- o **nome real do módulo** (`Capability::titan_name`) é o que vai em
  `Callee::Module { module }` e `Type::Opaque { module }`, porque é o que o
  codegen resolve contra `capabilities::lookup_module` para achar o caminho
  Rust.

Dois aliases para o mesmo módulo (`import data as a` / `import data as b`)
convivem sem conflito justamente por isso: são duas chaves em `modules`
apontando para a mesma `&'static Capability`, e a capability é imutável.

Com alias, o nome real **deixa de estar em escopo**: depois de `import data
as d`, escrever `data.read_csv(...)` é erro de módulo não importado. Quem
escreveu `as d` escolheu `d`.

## Consequências

- `import data` e `import data as d` são aceitos; `import "data"` (nome como
  string) segue erro de parser claro, nunca panic.
- Um alias que colide com um nome já declarado (função, outro import) é erro
  claro do checker — a mesma checagem de colisão da T38, agora sobre o nome
  local.
- `foreign import` — a forma do Titan original para trazer módulos C — segue
  fora de escopo desta fase (nunca foi considerada: não há FFI nesta fase).
- Módulos definidos pelo usuário (um `.titan` importando outro `.titan`)
  seguem fora de escopo — `import data` só resolve capabilities internas
  (`titan-data` nesta fase; `titan-crypto`/`titan-ai` nas fases 3b/3c
  reaproveitam o mesmo `import NOME`).
- Testado em `integration.rs` (movido de caso negativo para positivo na T44,
  ver `T44` no `PRD.md`) e exercitado ponta a ponta em
  `examples/dados.titan` (T45). O alias tem execução real própria em
  `compila_e_executa_import_com_alias`, sobre o módulo `texto` — de
  propósito, para provar o mecanismo sem pagar o build do Polars.
