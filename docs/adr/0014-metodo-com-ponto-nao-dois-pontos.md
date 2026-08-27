# ADR 0014 — Método chamado com `.`, não `:`

## Status

Aceito.

## Contexto

O Titan original (como o Lua que o inspira) usa dois-pontos para chamada de
método (`obj:metodo(args)`), açúcar sintático para `obj.metodo(obj, args)` —
o dois-pontos insere o receptor como primeiro argumento implícito. Ao
desenhar como `df.soma(df, "valor")` e a forma abreviada equivalente
coexistiriam nesta fase (o `PRD.md` da T45 exige que ambas funcionem:
`data.soma(df, "valor")` **e** `df.soma("valor")`), a sintaxe do dois-pontos
era a escolha natural por precedente do original.

Mas o dois-pontos exige uma regra gramatical própria (`:` como token de
chamada, distinto de `.` como token de acesso a campo) só para essa
diferença semântica — inserir o receptor implicitamente. O parser desta fase
já resolve `data.soma(...)` e `df.soma(...)` pelo mesmo caminho sintático
(`VarDot` seguido de chamada, `parser.rs:854`); a única diferença entre os
dois é **semântica**, resolvida no checker (`SymbolKind::Module` vs. tipo
`Opaque` do receptor, `checker.rs:2161` e `checker.rs:2178`), não sintática.

## Decisão

Chamada de método usa exclusivamente `.` — não existe token `:` na
gramática desta fase. `df.soma("valor")` é parseado como qualquer acesso a
campo seguido de chamada (`VarDot` + `ExpCall`); o checker é quem decide, ao
ver que a base (`df`) tem `Type::Opaque`, que `soma` deve ser resolvido
contra `capability.find_method` (`checker.rs:2196`) em vez de um campo de
record.

`df:soma("valor")` (dois-pontos) não é um caso rejeitado explicitamente pelo
checker — é rejeitado **pelo parser**, com erro de sintaxe, porque o token
`:` não é reconhecido nessa posição (o lexer emite `Colon` só para uso em
anotação de tipo de parâmetro, `parser.rs:272`). Isso está coberto pelo caso
negativo de `df:soma()` adicionado na T44 (curadoria dos testes de
integração).

## Consequências

- Uma única regra de parsing (`VarDot` + chamada) cobre função de módulo
  (`data.read_csv(...)`), método sobre opaco (`df.soma(...)`) e leitura de
  campo de record (`p.campo`) — a distinção entre os três é inteiramente do
  checker, resolvida por `SymbolKind`/`Type` do receptor, nunca por sintaxe
  diferente.
- Diverge deliberadamente do Titan original: qualquer programa `.titan` que
  use `obj:metodo()` (o idioma do original) precisa ser reescrito com `.`
  para compilar aqui — não há suporte a ambas as formas nem plano de
  adicionar dois-pontos como alias de `.` em fase futura, porque não haveria
  ganho (nenhuma diferença semântica a expressar) pelo custo de duas formas
  de sintaxe para a mesma coisa.
- `data.soma(df, "valor")` (forma de função de módulo, receptor explícito) e
  `df.soma("valor")` (forma de método, receptor implícito) coexistem e
  chamam o mesmo `CapabilityFn` — a segunda é açúcar da primeira, resolvida
  em `checker.rs:2196` inserindo o receptor como primeiro argumento no
  codegen. Provado por `examples/dados.titan` (T45), que exercita as duas
  formas sobre o mesmo `df` e confere que o relatório bate.
