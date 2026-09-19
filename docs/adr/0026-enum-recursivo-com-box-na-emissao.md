# ADR 0026 — `enum` recursivo com `Box` inserido na emissão

## Status

Aceito.

## Contexto

O [ADR 0009](0009-records-como-struct-rust-nominal.md) decidiu **rejeitar**
record recursivo: `record No prox: No end` seria um tipo de tamanho infinito
em Rust, o `rustc` o recusaria em inglês (`recursive type has infinite size`)
sobre código que o usuário não escreveu, e a Fase 2 não tinha como expressar
a base do recursivo. A última consequência daquele ADR deixou o assunto
aberto: "revisitar quando `Option` entrar no sistema de tipos".

Os tipos soma da Fase 5 (T74–T77) são esse revisitar, e com uma diferença que
muda o sinal da decisão: para um `enum`, a base do recursivo é uma **variante
sem payload**, que a própria declaração já expressa.

```lua
enum Exp
    ExpInteger(integer)
    ExpBinop(string, Exp, Exp)
end
```

Esse `Exp` não é um caso de borda — é o motivo de ser da fase, que tem o
parser e o checker do Titan escritos em Titan como alvo (T84–T89). Rejeitá-lo
seria rejeitar a fase.

Em Rust, o `enum` correspondente precisa de indireção nos dois campos
recursivos, e havia três lugares onde ela poderia entrar:

- **No fonte Titan**, com o usuário escrevendo a indireção à mão. Exigiria
  expor `Box` (ou equivalente) na linguagem — um conceito de representação de
  memória que o Titan não tem em nenhum outro lugar, e que contradiz o resto
  do desenho, onde arrays, maps e records não pedem nada ao usuário.
- **No checker**, marcando os campos e fazendo o codegen ler a marca. Moveria
  para a checagem de tipos uma decisão que não é de tipos: o tipo de
  `ExpBinop` é o mesmo com ou sem `Box`, e o checker passaria a carregar
  informação que só o backend Rust usa (e que um backend futuro para outra
  linguagem-alvo descartaria).
- **Na emissão**, detectando o ciclo no codegen.

> **Nota de numeração.** O `PRD.md` reservou o número `0021` para esta
> decisão, quando a Fase 5 foi planejada. As tarefas T59–T73 chegaram antes e
> ocuparam `0021`–`0025` com outras decisões (bitwise, `for` como `loop`,
> `continue`, `for`-in, `foreign function`), então este ADR entra no próximo
> número livre. O índice em [`README.md`](README.md) é a fonte de verdade da
> numeração; a tabela do PRD registra o plano, não o resultado.

## Decisão

**O `Box` é inserido pelo codegen**, sem aparecer no fonte Titan nem no tipo
que o checker manipula. Um campo de variante sai `Box<T>` quando seu tipo
**alcança o próprio enum**, direta ou indiretamente (`campos_boxeados`,
`codegen.rs`).

Três decisões derivadas, que a implementação fixou:

**1. A busca para na indireção que já existe.** `{Exp}` é `Vec<Exp>` e
`{string: Exp}` é `HashMap<String, Exp>`: os dois já põem os elementos no
heap, e `Exp?` é `Option<Exp>`, cujo tamanho é o do maior braço — todos
finitos sem ajuda. Encaixotá-los compilaria e só acrescentaria uma alocação
por valor, então a travessia não entra em array, map nem opcional. Ela
atravessa `Sum` **pelo nome** (consultando a tabela de enums do programa,
porque um `Sum` aninhado chega do checker como placeholder de variantes
vazias, e é só assim que a recursão mútua entre dois enums aparece) e
atravessa os campos de um `record` embutido.

**2. O `Box` fica do lado do `enum`, nunca do record.** No ciclo indireto
`enum Exp ExpNo(Caixa) end` + `record Caixa e: Exp end` — que a checagem de
ciclo de record não vê, porque `Exp` não é um record —, é o campo da
**variante** que ganha o `Box`. Encaixotar o campo do record mudaria o tipo
que todo `c.e` do programa enxerga, e o `Box` de um lado só já fecha o
tamanho dos dois.

**3. A detecção de ciclo existe para suportar, não para rejeitar.** É a
inversão em relação ao `checker.rs`, que detecta ciclo de record para recusar.
A checagem de ciclo de record **não** se aplica a `enum`, e continua valendo
para record (o ADR 0009 segue de pé no que é dele).

**Tipo soma tem semântica de valor, mas não passa por `&mut`.** Um `enum` é
dono de buffer próprio como um record, então a atribuição, o argumento de
chamada e a ligação de um campo clonam ([ADR 0006](0006-semantica-de-valor-clone-na-atribuicao.md),
via `valor_com_buffer_proprio`) — sem isso `local b = a` moveria `a` e a
semântica de valor deixaria de valer para exatamente um tipo. Mas ele **não**
entra em `is_composite`, então o parâmetro sai por valor e não por `&mut`
([ADR 0007](0007-parametros-compostos-por-mut.md)): um `Exp` recursivo é um
`Box` no bolso, não uma `Vec` para mutar no lugar, e o idioma in-place que o
`&mut` existe para preservar não tem análogo aqui.

**O escrutinado do `match` sai emprestado** (`match &e { .. }`). Com valor, um
padrão que liga campos moveria o escrutinado para dentro do braço, e
`local b = a` seguido de `match a` — que o checker aceita — deixaria de
compilar por `E0382` sobre código que o usuário não escreveu. Cada campo
ligado chega como referência e volta a valor num `let` no topo do braço, com a
mesma regra do `for`-in (T71): clone para quem tem buffer próprio, deref para
escalar, um deref a mais para o campo encaixotado.

**Tipo soma não sobe para `value`.** `titan_runtime::Value` tem um braço por
forma de valor do Titan — primitiva, array, map, record, opcional — e nenhum
para um `enum` do usuário. `check_cast` recusa a subida em português, e a
recusa vale para o tipo soma **dentro** de um composto (`{Cor}`, um record com
campo `Cor`), que a conversão elemento a elemento acabaria alcançando sem
braço para escrever.

## Consequências

- `enum Exp ExpBinop(string, Exp, Exp) end` compila, e uma mini-AST recursiva
  avaliada por `match` recursivo dá o resultado correto — provado por teste
  que compila o Rust gerado com o `rustc` de verdade e confere a saída
  (`codegen.rs`, `t77_mini_ast_recursiva_avalia_pelo_match_e_imprime_o_resultado`).
- O usuário nunca escreve nem lê `Box` no fonte Titan, e o checker nunca
  carrega a decisão: um backend futuro para outra linguagem-alvo resolve a
  recursão do jeito que a sua linguagem pedir, sem desfazer marca nenhuma.
- Custo: uma alocação por campo recursivo construído, e um `clone` profundo
  por campo ligado que o braço usa. É o preço da semântica de valor do ADR
  0006, agora também para tipo soma — revisitar se um perfil mostrar isso
  como gargalo real (o mesmo caveat que o 0006 registrou).
- Recursão **mútua** entre enums encaixota os dois lados, e não o mínimo
  necessário para fechar o ciclo. Uma alocação a mais em troca de a decisão de
  cada variante não depender da ordem em que os enums foram visitados.
- `enum` não chega a `value`, então um `enum` não atravessa a fronteira de uma
  capability nem entra num `{value}` heterogêneo. Quem precisa disso extrai o
  conteúdo com `match` primeiro — o que é a operação que o tipo soma existe
  para oferecer.
