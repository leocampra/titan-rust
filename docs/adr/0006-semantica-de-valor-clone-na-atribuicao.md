# ADR 0006 — Semântica de valor com `clone()` na atribuição de compostos

## Status

Aceito.

## Contexto

A Fase 2 introduz tipos compostos (`array`, `map`, `record`) e precisa decidir
o que `local b = a` significa quando `a` é composto. Há três modelos
razoáveis:

- **Aliasing**, como o Titan original: `a` e `b` passam a apontar para a
  mesma `Table*`; mutar `b` muta `a`. É o comportamento do Lua e do Titan de
  referência (`coder.lua:474-488`).
- **Move**, o padrão idiomático do Rust: `let b = a;` invalida `a`. Exigiria
  o checker rastrear "vida" de valores (fiel ao borrow checker) para saber
  quando um uso posterior de `a` é válido — máquina de empréstimos que a
  linguagem Titan não tem conceito para expressar.
- **Cópia profunda (`clone()`)**: `b` é uma cópia independente de `a`; mutar
  um não afeta o outro.

Aliasing exigiria `Rc<RefCell<...>>` (ou equivalente) em todo tipo composto,
contaminando o codegen inteiro com `.borrow()`/`.borrow_mut()` e reintroduzindo
o tipo de erro em tempo de execução (`already borrowed`) que Rust foi
escolhido para evitar. Move exigiria um checker de posse próprio, muito além
do escopo desta fase. Nenhum dos dois cabe no orçamento da Fase 2.

## Decisão

Tipos compostos têm **semântica de valor**: `{integer}` vira `Vec<i64>`,
record vira uma `struct` Rust própria, e **toda atribuição/declaração que
copia um composto emite `.clone()`** (`local b = a;` → `let b = a.clone();`).
Não há `Rc`, não há `RefCell`, não há aliasing — cada variável é dona de sua
própria cópia.

Essa é uma **divergência deliberada** do Titan original, que aliasa. O custo
é O(n) por cópia, aceito conscientemente em troca de eliminar toda uma classe
de bugs de aliasing e de erros de borrow em runtime.

A regra de quando clonar é centralizada numa única função,
`precisa_clone` (`codegen.rs:620`): `Var`/`Index`/`Field` de tipo composto ou
`String` ganham `.clone()`; literais, chamadas e construtores passam direto
(já são donos do valor que acabaram de criar).

## Consequências

- `local copia = qs; copia[1] = 999` não altera `qs` — provado por teste de
  integração ponta a ponta (`examples/compostos.titan`, linha
  `Original preservado: ...`).
- Programas que dependiam do aliasing do Titan original (por exemplo, dois
  nomes para a mesma tabela, mutação vista por ambos) não compilam para o
  mesmo comportamento — precisam ser reescritos usando parâmetros `&mut`
  (ver [ADR 0007](0007-parametros-compostos-por-mut.md)) quando a intenção é
  mutação compartilhada de verdade.
- Custo de performance O(n) em toda cópia de composto — aceitável na Fase 2,
  que prioriza corretude e simplicidade do modelo de memória sobre
  desempenho; revisitar se um perfil futuro mostrar isso como gargalo real.
- `String` recebeu o mesmo tratamento por decisão derivada
  ([ADR 0010](0010-string-sempre-string.md)): é sempre dona do seu buffer,
  então cai na mesma regra de `precisa_clone`.
