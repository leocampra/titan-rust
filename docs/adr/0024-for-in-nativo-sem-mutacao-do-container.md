# ADR 0024 — `for`-in nativo do Rust, com mutação do container proibida

## Status

Aceito.

## Contexto

A T71 traz `for x in v do ... end` sobre `{T}` e `for k, v in m do ... end`
sobre `{K: V}` — a forma de laço que a Fase 1 adiou por depender de `array` e
`map`. Três decisões ficaram em aberto ao implementá-la.

**Qual template emitir.** O `for` numérico é emitido como `loop` do Rust com o
incremento no topo ([ADR 0022](0022-for-como-loop-com-incremento-no-topo.md)),
e a tentação é reaproveitar esse template. Mas ele existe por motivos que não
valem aqui: `Range` não aceita passo negativo nem `f64`, e o incremento
precisou subir para o topo para que `continue` não pulasse por cima dele. Nada
disso aparece ao percorrer um container — o iterador do Rust já avança sozinho
antes de cada volta, que é exatamente a propriedade que o ADR 0022 teve de
construir à mão.

**O que acontece se o corpo mutar o container.** `for x in v do v[1] = 0 end`
é uma pergunta que toda linguagem com `for`-in precisa responder. Lua responde
com comportamento indefinido; Rust responde com o borrow checker, recusando o
programa — em inglês, o que quebraria a convenção deste projeto de nunca
mostrar erro do `rustc` ao usuário.

**Qual a ordem de iteração de um map.** `{K: V}` é `HashMap` do Rust, que não
garante ordem nenhuma — nem a de inserção, nem a das chaves, e nem a mesma
ordem entre duas execuções do mesmo binário.

## Decisão

**`for` nativo do Rust sobre `.iter()`**, não o template de `loop` do ADR 0022:
`for titan_forin_x in v.iter() { ... }` e `for (titan_forin_k,
titan_forin_v) in m.iter() { ... }`. `break` e `continue` (T63) caem dentro
dele sem caso especial nenhum, porque o `for` do Rust é um laço de verdade.

O nome que o iterador liga é sempre `titan_forin_*` — uma **referência** — e o
nome do usuário nasce logo dentro do corpo, por valor: `let x: i64 =
*titan_forin_x;` para escalares, `.clone()` para compostos e `string` (ADR
0006). É o mesmo idioma do estreitamento de opcionais (T68), e faz com que o
corpo leia `x` com exatamente o tipo `T` que o checker lhe deu. Variável que o
corpo nunca lê não ganha ligação, e o iterador a descarta com `_`, porque o
critério de aceite herdado da T69 é Rust gerado **sem warnings**.

**`.iter()`, nunca `.iter_mut()`**, porque **mutar o container durante a
iteração é erro do checker**, em português, antes de o `rustc` ver o programa.
Contam como mutação as duas formas que o [ADR 0007](0007-parametros-compostos-por-mut.md)
já reconhece como uso mutável de um composto: escrever no container ou dentro
dele (`v = ...`, `v[i] = ...`, `v.campo = ...`, inclusive como um dos alvos de
uma atribuição múltipla), e passá-lo como argumento de função — porque o
codegen emite `&mut` no call site independentemente do que o callee faz.

A varredura é sintática, roda sobre a AST do corpo antes de tipá-lo, e alcança
laços aninhados. Ela só se aplica quando o container é uma variável: `for x in
f() do` itera um temporário que o corpo não tem como nomear.

`for k, k in m do` é recusado: as duas variáveis são ligadas pelo **mesmo**
padrão do `for` do Rust, e repetir um nome ali é `identifier bound more than
once` — mais um erro do `rustc` que não pode chegar ao usuário.

**A ordem de iteração de um map é não especificada** — documentado no README
como diferença observável, não deixado para o usuário descobrir.

## Consequências

- `for`-in é um nó de AST próprio (`StatForIn`), e não uma variante de
  `StatFor`. Juntar as duas formas num nó só espalharia `Option` por todos os
  campos e obrigaria checker e codegen a decidir, em cada braço, qual forma é.
- A variável do laço é uma **cópia** do elemento: escrever nela é permitido
  (ela é `SymbolKind::ForVar`, como a do `for` numérico) e não alcança o
  container, coerente com a semântica de valor do ADR 0006. A ligação sai
  `let mut` exatamente quando o corpo escreve nela — `let` puro quando não,
  para não disparar `unused_mut`. Compostos pagam um `clone()` por volta — o
  mesmo custo O(n) que o ADR 0006 já aceitou conscientemente na atribuição.
- Quem precisa mutar enquanto percorre escreve um `for` numérico sobre os
  índices, que continua disponível e não passa por esta restrição. A mensagem
  de erro não sugere essa saída de propósito: sugere iterar sobre uma cópia ou
  coletar as mudanças, que são as duas que preservam a intenção original.
- A varredura não tem escopo. Um `local v = ...` que **sombreie** o container
  é aceito (declarar não é mutar), mas escrever no `v` novo depois disso é
  reportado como se fosse o container. É conservador na direção segura —
  recusa um programa válido em vez de aceitar um que o `rustc` recusaria
  depois —, e trocar o nome resolve.
- Programas que dependam da ordem de iteração de um map não são portáveis nem
  entre duas execuções do mesmo binário. Ordenar as chaves antes de iterar é
  responsabilidade de quem escreve.
