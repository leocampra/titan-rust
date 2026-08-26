# ADR 0007 — Parâmetros compostos passados por `&mut`

## Status

Aceito.

## Contexto

[ADR 0006](0006-semantica-de-valor-clone-na-atribuicao.md) fixou semântica de
valor para `local b = a`: cada atribuição clona. Mas essa decisão é
independente de como um composto é **passado como argumento de função** — e
aqui clonar sempre quebraria o idioma central do Titan original,
`titan/testfiles/selection_sort.titan`:

```lua
function selection_sort(xs: {integer}): nil
    -- reordena xs in-place
end
```

Se `xs` fosse recebido por valor (com clone na entrada da função, como uma
leitura ingênua do ADR 0006 sugeriria), `selection_sort` ordenaria uma cópia
local e o chamador não veria nenhuma mudança no array original — o programa
"funcionaria" (compila, roda, não crasha) mas produziria um resultado
silenciosamente errado. Esse é exatamente o tipo de bug que a convenção de
"nunca panic, sempre erro claro" deste projeto não protege, porque não há
erro algum: é uma divergência de comportamento silenciosa.

## Decisão

Parâmetros de função de tipo composto (`array`, `map`, `record`) são
recebidos por referência mutável — `&mut Vec<T>`, `&mut HashMap<K, V>`,
`&mut Nome` — nunca por valor. Passar um composto como argumento (`f(xs)`)
não clona; o callee enxerga e pode mutar o mesmo valor do chamador.

Isso é ortogonal à decisão do ADR 0006: clonar na **atribuição** (`local b =
a`) não implica clonar na **passagem de parâmetro** (`f(a)`). São dois pontos
de cópia diferentes no código gerado, com regras independentes.

Consequência direta no checker: passar um composto como argumento é **uso
mutável**, mesmo que a função nunca escreva nele — o `codegen.rs` emite
`&mut expr` no call site independentemente do corpo do callee, então o
checker precisa marcar a variável-raiz do argumento como `mut` (inserida no
`assigned: HashSet<DeclId>` só por aparecer numa chamada), senão o Rust
gerado tenta pegar `&mut` de uma binding não-`mut` e o `rustc` recusa a
compilação (em inglês, quebrando a convenção do projeto).

Decisão relacionada: chamar `f(xs, xs)` — o mesmo array duas vezes na mesma
chamada — exigiria dois empréstimos mutáveis simultâneos, que o `rustc`
recusa. O checker rejeita esse caso explicitamente, em português, antes de
chegar ao codegen.

## Consequências

- `dobrar_estoque(qs)` muta o `qs` do chamador — provado por teste de
  integração (`examples/compostos.titan`, linha
  `Primeiro estoque dobrado: ...`) e pelo próprio `selection_sort.titan`
  compilando e ordenando de verdade.
- Atribuir a um parâmetro composto **inteiro** dentro do corpo da função
  (`xs = {}`) é rejeitado pelo checker com erro claro — o parâmetro é uma
  referência para o array do chamador, não uma variável local que pode ser
  redirecionada para outro valor.
- Parâmetros compostos são sempre mutáveis do ponto de vista do Rust gerado,
  mesmo quando a função só lê — não há modo "somente leitura" (`&T`) para
  compostos nesta fase. Simplifica o codegen (uma única regra por tipo
  composto) ao custo de não expressar intenção de "não vou mutar isso" na
  assinatura.
- `f(xs, xs)` e qualquer variação que produziria dois empréstimos mutáveis
  simultâneos é erro do checker, não do `rustc` — mantém a convenção de nunca
  deixar o usuário ver uma mensagem de erro em inglês do compilador Rust
  subjacente.
