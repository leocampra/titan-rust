# ADR 0010 — `string` é sempre `String`: fim da dualidade `&str`/`String`

## Status

Aceito.

## Contexto

Desde a Fase 0, o codegen tratava `string` de forma dupla: literais viravam
`&'static str` e strings computadas viravam `String`, decisão espalhada por
cinco funções acopladas — `emit_slot_value`, `str_ord_operand`,
`concat_operand`, `coerce_to_borrowed_str`, `is_owned_string_expr`. O
doc-comment dessas funções já admitia a fragilidade: como o checker não
anotava o "nascimento" de cada variável `string` (se veio de um literal ou de
uma computação), o codegen tratava todo `Var: string` como dono do buffer e
coagia sempre — funcionava, mas por sorte de a Fase 0/1 nunca precisarem
comparar duas `string` diretamente (`str_ord_operand` existia só para
contornar isso).

A Fase 2 introduz `Vec<string>` e records com campo `string`. Com a
dualidade, cada um desses lugares herdaria a mesma ambiguidade —
`Vec<&'static str>` vs `Vec<String>` teria que ser decidida por posição, e um
record com campo `string` precisaria da mesma decisão para cada instância.
Cinco funções acopladas hoje virariam mais, espalhadas por mais lugares,
para um ganho que nunca foi ganho real: literais de string em Titan não são
tratados como statically-scoped no sentido do Rust (`'static`), então a
dualidade não comprava nada em segurança — só complexidade.

A alternativa considerada e rejeitada foi anotar `Owned`/`Borrowed` no
`TypedExp` (o checker decidiria e o codegen só obedeceria) — mas isso
propagaria a mesma dualidade para **dentro** de `Vec<String>` e dos campos de
record, multiplicando os casos em vez de eliminá-los.

## Decisão

`Type::String` mapeia para `"String"` em **toda** posição — parâmetro,
retorno, campo de record, elemento de array, variável local. O caso especial
de `rust_param_type_name` que existia só para strings deixa de existir. As
cinco funções acopladas colapsam em no máximo duas: uma que decide `.clone()`
(reusando a mesma regra `precisa_clone` do [ADR 0006](0006-semantica-de-valor-clone-na-atribuicao.md))
e outra que decide `&` na fronteira com o `titan-runtime` (funções como
`concat` recebem `&str`, então uma `String` do lado do usuário precisa de
`&` no call site, não de mudar seu tipo de armazenamento).

Comparação de ordem (`s1 < s2`) passa a funcionar direto via
`String: PartialOrd<String>` da stdlib, eliminando `str_ord_operand`
inteiramente — não é mais um caso especial, é o operador padrão do tipo.

O shim de entrada (`ENTRY_SHIM`) passa a passar `&mut args` para
`titan_main`, e `main(args: {string})` vira `&mut Vec<String>` — isso também
elimina a última exceção de `rust_type_name` (`&[String]` só para o parâmetro
de `main`), deixando o mapeamento de tipos com **zero** casos especiais.

Custo aceito: `f("literal")` passa a alocar uma `String` no call site, onde
antes um `&'static str` não alocava. É o mesmo tipo de custo O(n) que o
[ADR 0006](0006-semantica-de-valor-clone-na-atribuicao.md) já aceitou para
compostos, em troca de eliminar uma classe inteira de bugs de tipo.

## Consequências

- O mapeamento de tipos Titan → Rust fica genuinamente uniforme para
  `string` — nenhuma posição precisa saber se está lidando com um literal ou
  um valor computado.
- `Vec<String>` e campos `string` de record não reabrem a dualidade — a
  decisão da Fase 2 (arrays/records) não precisa de nenhum caso especial para
  string dentro de composto, exatamente o motivo desta ADR ter sido resolvida
  **antes** de T26 ([ADR 0007](0007-parametros-compostos-por-mut.md) e
  [0008](0008-indexacao-checada-e-variancia-invariante.md) dependem de string
  já ser uniforme).
- Todo `hello.titan` e `nucleo.titan` (Fases 0 e 1) seguem compilando com a
  mesma saída — a mudança é só na representação interna do Rust gerado, não
  no comportamento observável.
- Rust gerado permanece sem warnings apesar da alocação extra — não há
  `&'static str` não utilizado nem mistura de tipos que o `rustc` reclamaria.
