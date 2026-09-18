# ADR 0025 — `foreign function` com assinatura Titan, não `foreign import` de header

## Status

Aceito.

## Contexto

A T73 abre a porta de FFI que o `plano.md` sempre previu. O nó
`TopLevelForeignImport` estava na AST desde a T1 — copiado do Titan original,
onde a grafia é `foreign import stdio "stdio.h"` — e nunca foi produzido pelo
parser: o checker o rejeitava com "`foreign import` não é suportado nesta
fase". Implementá-lo agora exigia responder três perguntas que o nó herdado
não responde.

**De onde vem a assinatura.** `foreign import stdio "stdio.h"` nomeia um
header C e mais nada. Para saber que `abs` recebe um `int` e devolve um `int`,
o compilador teria de **ler o header** — parsear C, com seus `#include`,
macros, `typedef` e variação por plataforma. É um gerador de bindings, que é
justamente o que o PRD recusa ao pedir "manter mínimo e explícito — é uma
porta de FFI, não um gerador de bindings".

**O que pode atravessar a fronteira.** Um `String` do Rust é ponteiro +
tamanho + capacidade; um `Vec<T>` e um `HashMap<K, V>` têm layout escolhido
pelo compilador; um `struct` sem `#[repr(C)]` pode ter os campos reordenados.
Nenhum deles tem representação que uma função C saiba ler. Passá-los
compilaria — o `extern "C"` aceita qualquer tipo — e leria memória errada em
tempo de execução.

**De quem é a responsabilidade quando a assinatura está errada.** Declarar
`foreign function abs(n: float): float` para a `abs` da libc compila e produz
lixo. Nenhum compilador pode verificar isso sem ler o header, e mesmo lendo o
header não pode verificar o que o linker vai resolver.

## Decisão

**A assinatura é escrita em Titan, uma função por declaração**, e nenhum
header é lido:

```lua
foreign function abs(n: integer): integer
foreign function strlen(s: string): integer
```

Não há corpo, e por isso não há `end`. O nó `TopLevelForeignImport` é
substituído por `TopLevelForeignFunc { loc, name, params, rettypes }`, que
reusa `parse_param_list` e `parse_rettypes_opt` — a assinatura de uma função
externa é lida exatamente como a de uma `function` comum.

A grafia do original (`foreign import ... "..."`) tem **erro próprio**, que
aponta a grafia que existe em vez de só dizer que a outra não existe.

**Só escalares e `string` atravessam a fronteira**: `integer` (`i64`), `float`
(`f64`), `boolean` (`bool`) e `string`. `nil` vale só como retorno, o `void`
do C. Tudo mais — `{T}`, `{K: V}`, `record`, `value`, `T?` — é recusado pelo
**checker**, com mensagem em português, antes de o `rustc` ver o `extern "C"`.
Retorno múltiplo (T66) também: a ABI C devolve um valor só.

`string` é a única que custa trabalho, e ele mora no runtime, não inline no
Rust gerado: `ffi_cstring` na ida (`String` → `CString`, abortando em
português se houver byte zero no meio) e `ffi_string` na volta (`*const
c_char` → `String`, checando o nulo). Mesmo critério de `array_get` — o caso
de erro sai em português, nunca como `panic!` cru.

**A chamada sai envolta em `unsafe`, e isso é o desenho, não um detalhe.** O
`rustc` exige, e o Rust gerado diz em voz alta o que a declaração assumiu: a
responsabilidade pela assinatura é de quem escreveu o `foreign function`.

## Consequências

- **Nenhuma dependência nova.** A libc já vem linkada com a std; o
  `Cargo.toml` gerado não ganha `libc` nem nada. Um `.titan` que chama `abs`
  paga exatamente o mesmo build de um que não chama.
- **O nome externo não passa pelo mangling** de `mangle_fn_name` (`titan_`):
  é o símbolo que o linker procura. Colisão com o `fn main` do shim não é
  possível — o checker recusa redeclarar um nome já declarado, e `main` em
  Titan é uma `function` comum, que o mangling afasta.
- **Um bloco `unsafe extern "C"` por declaração**, na ordem do fonte, em vez
  de um bloco só com todas. Cada `extern` fica ao lado do que o originou.
- **Um argumento `string` gera uma ligação `let`** dentro de um bloco, porque
  a `CString` precisa continuar viva durante a chamada — `.as_ptr()` sobre um
  temporário seria ponteiro pendurado. O bloco sai entre parênteses: em
  posição de statement, um `{ ... };` cru seria lido como bloco-statement
  seguido de statement vazio, e o valor da chamada se perderia em silêncio.
  Sem argumento `string`, nenhuma ligação aparece.
- **`foreign function` é `SymbolKind::Global`**, como qualquer função
  top-level: para escopo, colisão de nome e atribuição, um símbolo externo é
  idêntico a uma função Titan. Uma variante nova de `SymbolKind` forçaria um
  braço a mais em todo `match` sem dizer nada de novo. O que **é** diferente
  é só a emissão, e isso viaja num conjunto à parte (`Checker::foreigns`) até
  `resolve_callee`, que o converte em `Callee::Foreign`.
- **Assinatura errada continua sendo erro do programador**, e em silêncio. O
  compilador garante que os tipos declarados atravessam a fronteira; não
  garante que eles são os tipos que a função externa realmente tem. É o
  mesmo contrato de qualquer FFI, e é por isso que a declaração é explícita
  em vez de gerada.
- **Sem `string` com byte zero no meio.** Em C a string termina no primeiro
  zero, então uma `string` Titan que tenha um não tem como atravessar; o
  runtime aborta em português em vez de deixar a função externa ler menos do
  que o programa escreveu.
- Bytes que não formam UTF-8 válido, na volta, viram `U+FFFD`
  (`to_string_lossy`), no mesmo espírito do [ADR 0010](0010-string-sempre-string.md):
  uma `string` Titan é sempre uma `string` válida, nunca bytes crus que
  estouram mais adiante.
