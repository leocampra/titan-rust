# titan-rust

Compilador, escrito em Rust, para uma linguagem tipada inspirada em Lua que gera
código Rust nativo. O projeto [Titan](../titan) original (escrito em Lua) serve
como **especificação de referência** de gramática, AST e sistema de tipos — não
como base de código. Veja [`PRD.md`](PRD.md) para o plano de tarefas completo e
[`docs/arquitetura.md`](docs/arquitetura.md) para o porquê das decisões abaixo.

Estado atual: **Fase 4 ("Self-hosting / LSP")** — além do hello world da Fase
0, do núcleo da linguagem da Fase 1, dos tipos compostos da Fase 2 e das
capability runtimes da Fase 3 (`import`, módulos, tipos opacos, `titan-data`),
o compilador agora expõe um language server (`titan-lsp`) que dá diagnóstico,
hover, go-to-definition e autocomplete num `.titan` aberto no VS Code, e a
linguagem ganhou `break`, a capability `texto` (acesso a texto por byte) e a
capability `io` (leitura de arquivo) — o suficiente para
`examples/lexer.titan`, um lexer do Titan escrito em Titan, tokenizar
`examples/nucleo.titan`.

## Como compilar

Da raiz do workspace (`titan-rust/`):

```bash
cargo build --release
```

Isso produz `target/release/titanc`.

## Como rodar o hello world

```bash
./target/release/titanc examples/hello.titan
./hello
# → Olá, mundo!
echo $?
# → 0
```

## Como rodar o exemplo da Fase 1 (núcleo da linguagem)

```bash
./target/release/titanc examples/nucleo.titan
./nucleo
# → Fatorial de 5: 120
# → Fibonacci de 10: 55
echo $?
# → 0
```

## Como rodar o exemplo da Fase 2 (tipos compostos)

```bash
./target/release/titanc examples/compostos.titan
./compostos
echo $?
# → 0
```

`examples/compostos.titan` exercita record (construção, leitura e escrita de
campo), array (literal, `#`, indexação, mutação in-place por função, push via
`#res+1`), array de floats e map — inclusive as duas provas centrais da fase:
`local copia = qs; copia[1] = 999` não altera `qs` (semântica de valor,
[ADR 0006](docs/adr/0006-semantica-de-valor-clone-na-atribuicao.md)), e
`dobrar_estoque(qs)` muda o `qs` do chamador (parâmetros compostos por
`&mut`, [ADR 0007](docs/adr/0007-parametros-compostos-por-mut.md)).

## Como rodar o exemplo da Fase 3 (capability runtime `titan-data`)

```bash
./target/release/titanc examples/dados.titan
./dados
echo $?
# → 0
```

`examples/dados.titan` importa a capability `data` (`import data`), lê
`examples/vendas.csv`, imprime dimensões e colunas, extrai uma coluna como
array Titan (exercitando a Fase 2 sobre o resultado) e agrega uma coluna
**pelas duas formas equivalentes** — `data.soma(df, "valor")` (função de
módulo) e `df.soma("valor")` (método sobre o tipo opaco
`data.DataFrame`, [ADR 0014](docs/adr/0014-metodo-com-ponto-nao-dois-pontos.md)).

O método também pode ser chamado com dois-pontos, no idioma do Titan
original: `df:soma("valor")` gera exatamente o mesmo Rust que
`df.soma("valor")`. `.` é a forma preferida (é a mesma sintaxe do acesso a
campo e da função de módulo); `:` existe para quem traz código do original.

O `import` aceita alias: `import data as d` traz o módulo sob o nome `d`,
e a partir daí é `d` que qualifica tudo (`d.DataFrame`, `d.read_csv(...)`).
Com alias, o nome original sai de escopo — depois de `import data as d`,
`data.read_csv(...)` é erro de módulo não importado.

> **Custo de build/disco:** este é o único exemplo que invoca o `cargo
> build --release` sobre uma dependência do Polars — leva **~2 minutos** e
> deixa **~3GB** em `build/dados/target/` (medido antes da Fase 3 começar).
> Como o `titanc` gera um projeto Cargo por programa compilado
> (`build/<nome>/`), esse custo é por programa, não uma vez só — um
> programa sem `import data` nunca paga esse preço
> ([ADR 0015](docs/adr/0015-api-data-como-contrato-backend-trocavel.md)).

## Como rodar o exemplo da Fase 4 (self-hosting: lexer em Titan)

```bash
./target/release/titanc examples/lexer.titan
./lexer examples/nucleo.titan
echo $?
# → 0
```

`examples/lexer.titan` importa `texto` e `io`, lê o `.titan` passado por
`args`, tokeniza um subconjunto do idioma (identificadores, palavras-chave,
inteiros, strings, comentários `--` e os símbolos da Fase 1/2) e imprime a
lista de tokens — um pedaço do compilador rodando na própria linguagem
([ADR 0020](docs/adr/0020-self-hosting-por-etapas.md)). O estilo é
deliberadamente deselegante (constantes como função no lugar de tipo soma,
estado da varredura num `record` passado por parâmetro no lugar de retorno
múltiplo) — evidência do que falta para a Fase 5, não defeito desta.

## Como rodar o projeto da Fase 5 (self-hosting: o pipeline em Titan)

```bash
./target/release/titanc --manifesto selfhost --out .
./titanself examples/nucleo.titan
# → -- tipos --
# → fatorial: (integer) -> integer
# → fibonacci: (integer) -> integer
# → main: ({string}) -> integer
# → -- arvore --
# → function fatorial(n: integer): integer
# →   if (n <= 1)
# →     return 1
# →   ...
echo $?
# → 0
```

`selfhost/` é um projeto multi-módulo declarado por
[`titan.toml`](selfhost/titan.toml), e não um arquivo solto: é assim que um
compilador escrito em Titan passa a caber em mais de um arquivo. Ele traz o
pipeline inteiro — `ast.titan` (a AST sobre tipos soma), `lexer.titan`,
`parser.titan` e `checker.titan` —, e o `main.titan` os amarra: lê o fonte,
tokeniza, parseia, checa, e imprime a **AST tipada** — as assinaturas que a
análise resolveu e a forma da árvore que o parser produziu.

A `ast.titan` é onde os tipos soma aparecem inteiros: `enum Exp` **recursivo de
verdade** (`ExpBinop(Loc, string, Exp, Exp)`), `enum Stat`, `enum TopLevel`,
`enum Var`, `enum Tipo` e `record Loc`.

As outras saídas do driver, cada uma uma etapa do pipeline:

```bash
./titanself                                 # a Exp de 1 + 2 * 3, percorrida com match
./titanself --tokens examples/nucleo.titan  # só o lexer: um token por linha
./titanself --arvore examples/nucleo.titan  # lexer + parser: a árvore sintática
./titanself --checar examples/nucleo.titan  # + o checker: "ok: <arquivo>" ou os erros
```

**Que o pipeline em Titan concorda com o `titanc` não é afirmação, é teste.**
Um teste de integração roda os dois sobre os mesmos `examples/*.titan` — o
`titanc` entra como lib ([ADR 0018](docs/adr/0018-titanc-lib-lsp-reusa-pipeline.md)),
com as mesmas funções `lex`/`parse`/`check` que o compilador de verdade usa — e
compara a lista de tokens, a forma da árvore e o conjunto de erros de tipo.
Divergência é falha de teste.

Vale ler `selfhost/ast.titan` lado a lado com `examples/lexer.titan`: o mesmo
compilador, antes e depois dos tipos soma. Lá, `TokenKind` é `integer` e cada
constante vira uma função sem argumento
(`function TK_NAME(): integer return 1 end`); aqui, cada variante carrega
exatamente os campos que tem, e esquecer um caso no `match` é erro do checker,
em português. O `Box` que fecha a recursão no Rust gerado é do codegen
([ADR 0026](docs/adr/0026-enum-recursivo-com-box-na-emissao.md)) — não aparece
no fonte Titan.

`examples/lexer.titan` continua **intocado**: é o registro histórico que o
[ADR 0020](docs/adr/0020-self-hosting-por-etapas.md) cita como evidência
empírica de que faltavam tipos soma. Consertá-lo destruiria a comparação.

O `titanc` lê o `.titan`, gera um projeto Cargo temporário em `build/<nome>/`,
compila-o com `cargo build --release` e copia o binário resultante para o
diretório atual como `<nome>`.

Para inspecionar o Rust gerado sem compilar:

```bash
./target/release/titanc --emit-rust examples/nucleo.titan
```

### CLI

```
titanc [--emit-rust] [--out DIR] [-v] <arquivo.titan>
```

- `--emit-rust` — imprime o Rust gerado e para, sem invocar o `cargo`.
- `--out DIR` — diretório onde `build/<nome>/` é criado e onde o executável
  final é copiado (default: diretório atual).
- `-v` — mostra a invocação do `cargo build --release`.

> O `titanc` **não** é instalado no PATH global nesta fase — invoque sempre
> pelo caminho explícito (`./target/release/titanc` ou
> `./target/debug/titanc`).

## Como rodar o LSP e a extensão VS Code

```bash
cargo build --release
```

produz também `target/release/titan-lsp`, o language server (diagnósticos,
hover, go-to-definition, autocomplete — T48/T49/T50). Ele conversa por
stdio e nunca invoka o `cargo` — roda `lex → parse → check` em memória sobre
o buffer do editor ([ADR 0018](docs/adr/0018-titanc-lib-lsp-reusa-pipeline.md)).

O cliente é a extensão mínima em `editors/vscode/` (não publicada no
marketplace nesta fase): `cd editors/vscode && npm install && npm run
compile`, abrir essa pasta no VS Code e pressionar **F5** sobe uma janela
com a extensão carregada — veja
[`editors/vscode/README.md`](editors/vscode/README.md) para o passo a passo
completo e como apontar `titan.serverPath` para o binário compilado.

## Testes

```bash
cargo test
```

Cobre as unidades de cada etapa do pipeline (lexer, parser, checker, codegen,
driver) e um teste de integração que invoca o binário `titanc` de verdade,
conferindo stdout e exit code do executável gerado, além de uma suíte de casos
negativos (erro claro, nunca panic).

## Relação com o Titan original

O [Titan](../titan) (`titan/`, escrito em Lua) já tem lexer, parser, AST,
checker e symbol table para uma linguagem muito parecida com esta. O
`titan-rust` **reaproveita o desenho** dessas etapas — mesma gramática, mesmos
nomes de nó de AST (`ExpString`, `StatCall`, `TopLevelFunc`...), mesma
estratégia de verificação de tipos em duas passadas — mas é uma
**implementação nova, em Rust, do zero**. Nada do código Lua é executado ou
transpilado; o Titan serve apenas como referência viva para conferir se o
comportamento bate.

O que **não** foi reaproveitado, e por quê, está em
[`docs/arquitetura.md`](docs/arquitetura.md) e em
[`docs/adr/0001-compilador-novo-em-rust.md`](docs/adr/0001-compilador-novo-em-rust.md).
Duas decisões da Fase 1 divergem deliberadamente do comportamento do Titan
original — o `for` numérico desaçucarado para `while`
([ADR 0004](docs/adr/0004-for-desacucarado-para-while.md)) e `and`/`or`
exigindo booleano estrito em vez de truthy/falsy
([ADR 0005](docs/adr/0005-and-or-boolean-estrito.md)). A Fase 2 soma mais
cinco: semântica de valor com `clone()` em vez de aliasing
([ADR 0006](docs/adr/0006-semantica-de-valor-clone-na-atribuicao.md)),
parâmetros compostos por `&mut` para preservar o idioma in-place
([ADR 0007](docs/adr/0007-parametros-compostos-por-mut.md)), indexação
checada no runtime com `T` em vez de `T?`
([ADR 0008](docs/adr/0008-indexacao-checada-e-variancia-invariante.md)),
records como `struct` nominal
([ADR 0009](docs/adr/0009-records-como-struct-rust-nominal.md)) e `string`
sempre `String`
([ADR 0010](docs/adr/0010-string-sempre-string.md)). A Fase 3 soma mais
cinco: `import data` como declaração de topo fixa, sem alias
([ADR 0011](docs/adr/0011-import-como-acucar-sintatico.md)), módulo como
`SymbolKind` em vez de `Type`
([ADR 0012](docs/adr/0012-modulo-como-symbolkind-nao-tipo.md)), tipo opaco
composto por herança de `is_composite`
([ADR 0013](docs/adr/0013-tipo-opaco-composto-por-heranca.md)), método
chamado com `.` em vez de `:`
([ADR 0014](docs/adr/0014-metodo-com-ponto-nao-dois-pontos.md)) e a API
`data.*` como contrato estável sobre um backend (Polars) trocável
([ADR 0015](docs/adr/0015-api-data-como-contrato-backend-trocavel.md)). A
Fase 4 soma mais cinco: acesso a texto por capability, não builtins nem
`s[i]` ([ADR 0016](docs/adr/0016-acesso-a-texto-por-capability.md)), `break`
sem `continue`
([ADR 0017](docs/adr/0017-break-sim-continue-nao.md) — a metade que recusava
`continue` foi superada na Fase 5 pelo
[ADR 0023](docs/adr/0023-continue-entra-com-o-incremento-no-topo.md)),
`titanc` exposto
como lib para o LSP reusar o pipeline
([ADR 0018](docs/adr/0018-titanc-lib-lsp-reusa-pipeline.md)), `tower-lsp`
com deps isoladas do `Cargo.toml` gerado
([ADR 0019](docs/adr/0019-lsp-tower-lsp-deps-isoladas.md)) e self-hosting
entregue por etapas, só o lexer nesta fase
([ADR 0020](docs/adr/0020-self-hosting-por-etapas.md)). A Fase 5 soma mais
quatro: bitwise exigindo `integer` estrito, sem coagir `float`
([ADR 0021](docs/adr/0021-bitwise-exige-integer-sem-coercao.md)), o `for`
numérico como `loop` com o incremento no topo — que supera o ADR 0004
([ADR 0022](docs/adr/0022-for-como-loop-com-incremento-no-topo.md)) —,
`continue` entrando na linguagem com o mesmo desenho de `break`
([ADR 0023](docs/adr/0023-continue-entra-com-o-incremento-no-topo.md)) e o
`for`-in como `for` nativo do Rust, com mutação do container durante a
iteração recusada pelo checker
([ADR 0024](docs/adr/0024-for-in-nativo-sem-mutacao-do-container.md)).

`titan/` e `lua/` (usado para checar comportamento de referência do Lua) são
**somente leitura** neste repositório — repositórios de terceiros, nunca
editados.

## O que já está implementado

Fase 0 (hello world) + Fase 1 (núcleo da linguagem) + Fase 2 (tipos
compostos) + Fase 3 (capability runtimes) + Fase 4 (self-hosting / LSP):

- `function`/`local function`, `local x [: T] = exp`, `return`.
- Operadores aritméticos `+ - * / % ^`, relacionais `== ~= < > <= >=`,
  lógicos `and or not`, unário `-`/`not`; `..` (concatenação, coage
  número→string).
- Controle de fluxo: `if`/`elseif`/`else`, `while`, `repeat`/`until`,
  `for` numérico (`for x = start, finish[, inc] do ... end`), `for`-in sobre
  array e map (`for x in v do ... end`, `for k, v in m do ... end` — Fase 5,
  T71, [ADR 0024](docs/adr/0024-for-in-nativo-sem-mutacao-do-container.md)),
  `break` ([ADR 0017](docs/adr/0017-break-sim-continue-nao.md)) e `continue`
  (Fase 5, T63 — o `for` passou a ser emitido como `loop` com o incremento no
  topo, [ADR 0022](docs/adr/0022-for-como-loop-com-incremento-no-topo.md) e
  [ADR 0023](docs/adr/0023-continue-entra-com-o-incremento-no-topo.md)).
- Atribuição single-target: `nome = exp` para local já declarada, incluindo
  `v[i] = x` e `p.campo = x`.
- Tipos compostos: `array` (`{T}`, literal, indexação `v[i]`, `#v`, mutação
  in-place por função via `&mut`), `record` (declaração `record Nome ... end`,
  literal exaustivo, leitura/escrita de campo `p.campo`) e `map`
  (`{K: V}`, `map_get`/`map_set` via indexação).
- `import data`/`import texto`/`import io` (declaração de topo, sem alias) e
  namespaces de módulo (`data.read_csv(...)`, `texto.byte(...)`,
  `io.ler_arquivo(...)`).
- Tipos opacos de capability (`data.DataFrame`) e métodos com ponto sobre
  eles (`df.soma("valor")`, açúcar da forma de módulo
  `data.soma(df, "valor")`).
- Capability runtime `titan-data`: leitura de CSV (`data.read_csv`),
  inspeção (`linhas`, `colunas`, `coluna_integer`/`coluna_float`) e
  agregação (`soma`, `media`, `minimo`, `maximo`) sobre Polars.
- Capability runtime `titan-texto`: acesso a texto por byte (`byte`, `sub`,
  `para_inteiro`, `de_inteiro`, `tamanho`)
  ([ADR 0016](docs/adr/0016-acesso-a-texto-por-capability.md)).
- Capability runtime `titan-io`: leitura de arquivo (`ler_arquivo`).
- `titan-lsp`: diagnósticos, hover, go-to-definition e autocomplete sobre o
  pipeline `lex → parse → check`, sem invocar o `cargo`
  ([ADR 0018](docs/adr/0018-titanc-lib-lsp-reusa-pipeline.md),
  [ADR 0019](docs/adr/0019-lsp-tower-lsp-deps-isoladas.md)), com extensão
  mínima para VS Code (`editors/vscode/`).
- `examples/lexer.titan`: lexer do Titan escrito em Titan, sobre `texto` e
  `io` — prova de self-hosting parcial
  ([ADR 0020](docs/adr/0020-self-hosting-por-etapas.md)).
- `selfhost/`: o pipeline `lexer → parser → checker` escrito em Titan, sobre
  tipos soma e módulos de usuário, validado contra o `titanc` por um teste de
  oráculo que compara tokens, forma da árvore e erros de tipo — veja "Como
  rodar o projeto da Fase 5" acima.
- Tipos opcionais (`T?`), com estreitamento por `if x ~= nil then`.
- Cast de tipo (`exp as T`) e o tipo `value` — veja a seção abaixo.
- `foreign function` (Fase 5, T73): chamada a funções C, com assinatura
  escrita em Titan — veja a seção abaixo
  ([ADR 0025](docs/adr/0025-foreign-function-com-assinatura-titan.md)).

### Cast `as` e o tipo `value`

`exp as T` converte entre números e de/para `value`. **Cast não é parsing**:
`"3" as integer` é erro de compilação, não a leitura do número dentro da
string.

```lua
local a: float = 1 as float        -- 1.0
local b: integer = 3.9 as integer  -- 3
local c: integer = -3.9 as integer -- -3
```

> **`as integer` trunca, `//` faz piso.** `-3.9 as integer` é **-3** (corta a
> parte fracionária, indo em direção a zero), enquanto `-7 // 2` é **-4**
> (arredonda para menos infinito, como no Lua). São duas operações
> diferentes, e a diferença só aparece com número negativo — é a pegadinha
> que vale ler duas vezes.

`value` é o topo do gradual typing: **qualquer** tipo sobe para ele, e a
subida **copia** (um `{integer}` convertido para `value` não fica aliasado ao
array de origem, seguindo o
[ADR 0006](docs/adr/0006-semantica-de-valor-clone-na-atribuicao.md)).

```lua
local v: value = 42 as value
local n: integer = v as integer    -- 42
```

A descida é checada **em tempo de execução** e só vai a tipo primitivo
(`boolean`, `integer`, `float`, `string`): se o `value` guardar outro tipo, o
programa aborta com mensagem em português e código 1, nunca com um panic do
Rust. Ela também **não converte nem formata** — um `integer` guardado não
desce como `float` nem como `"42"`; para isso, escreva os dois passos
(`v as integer as float`), e aí a conversão fica visível no fonte.

### `for`-in sobre array e map

`for x in v do ... end` percorre um `{T}`; `for k, v in m do ... end` percorre
um `{K: V}`, ligando chave e valor. `break` e `continue` valem lá dentro como
em qualquer outro laço.

```lua
local notas: {integer} = {7, 9, 10}
local soma: integer = 0
for n in notas do
    soma = soma + n
end

local idades: {string: integer} = {["ana"] = 30, ["bia"] = 25}
for nome, idade in idades do
    print(nome .. " tem " .. idade)
end
```

A variável do laço é uma **cópia** do elemento, coerente com a semântica de
valor do [ADR 0006](docs/adr/0006-semantica-de-valor-clone-na-atribuicao.md):
escrever nela não alcança o container.

> **A ordem de iteração de um map é não especificada.** `{K: V}` é um
> `HashMap` do Rust, que não garante ordem nenhuma — nem a de inserção, nem a
> das chaves, e nem a mesma ordem entre duas execuções do mesmo binário. Quem
> espera ordem de inserção (o reflexo de quem vem do Lua com tabelas
> pequenas) precisa ordenar as chaves antes de iterar. É a diferença
> observável que mais morde nesta construção.

**Mutar o container durante a iteração é erro de compilação**, em português,
antes de o `rustc` ver o programa
([ADR 0024](docs/adr/0024-for-in-nativo-sem-mutacao-do-container.md)). Contam
como mutação escrever no container (`v = ...`, `v[i] = ...`, `v.campo = ...`)
e passá-lo como argumento de função — porque parâmetro composto é `&mut`
([ADR 0007](docs/adr/0007-parametros-compostos-por-mut.md)).

```lua
for x in v do
    v[1] = 0   -- erro: não é possível modificar 'v' dentro do `for`-in
end            --       que itera sobre ele
```

Quem precisa mutar enquanto percorre escreve um `for` numérico sobre os
índices, que não passa por essa restrição.

### `foreign function` — a porta de FFI

`foreign function` declara uma função externa, com a assinatura escrita em
Titan. Não tem corpo, e por isso não tem `end`.

```lua
foreign function abs(n: integer): integer
foreign function strlen(s: string): integer

function main(args: {string}): integer
    print("abs(-7) = " .. abs(-7))       -- 7
    print("strlen = " .. strlen("titan")) -- 5
    return 0
end
```

O símbolo é resolvido pelo linker; a libc já vem linkada com a std, então
**nenhuma dependência nova entra no `Cargo.toml` gerado**. No Rust emitido,
cada declaração vira um bloco `unsafe extern "C"` e cada chamada sai envolta
em `unsafe` — o código gerado diz em voz alta o que a declaração assumiu.

A grafia do Titan original (`foreign import stdio "stdio.h"`) **não existe
aqui**: ela nomeia um header C e deixa as assinaturas implícitas, o que
exigiria parsear C — um gerador de bindings, que esta porta deliberadamente
não é ([ADR 0025](docs/adr/0025-foreign-function-com-assinatura-titan.md)).
Escrevê-la dá erro que aponta a grafia que existe.

> **Só escalares e `string` atravessam a fronteira.** `integer`, `float`,
> `boolean` e `string`; `nil` vale só como retorno (o `void` do C). `{T}`,
> `{K: V}`, `record`, `value` e `T?` são recusados pelo **checker**, com
> mensagem em português, antes de o `rustc` ver o `extern "C"` — o layout
> deles é escolhido pelo Rust, e nenhuma função C sabe lê-lo. Retorno
> múltiplo também: a ABI C devolve um valor só.

```lua
record Ponto
    x: integer
    y: integer
end

foreign function dist(p: Ponto): float
-- erro: o parâmetro 'p' de `foreign function dist` é Ponto, que não
--       atravessa a fronteira de FFI; só integer, float, boolean e
--       string atravessam.
```

`string` é a única que custa conversão: na ida vira `CString` (uma `string`
com byte zero no meio aborta em português — em C a string termina no primeiro
zero), e na volta vira `string` a partir do `char*`, com o ponteiro nulo
checado.

**A assinatura errada continua sendo erro de quem escreve.** O compilador
garante que os tipos declarados atravessam a fronteira; não garante que eles
são os tipos que a função externa realmente tem — declarar `foreign function
abs(n: float): float` para a `abs` da libc compila e produz lixo. É o mesmo
contrato de qualquer FFI, e é por isso que a declaração é explícita em vez de
gerada.

## Tipos soma: `enum` e `match`

Um `enum` declara um tipo com variantes, cada uma com zero ou mais campos
posicionais. Variante sem campo não leva parênteses, na declaração nem na
construção.

```lua
enum Exp
    ExpInteger(integer)
    ExpBinop(string, Exp, Exp)
end
```

**A recursão é o ponto.** `ExpBinop` carrega dois `Exp`, e é isso que faz o
tipo servir para uma AST — a do parser do Titan escrito em Titan, inclusive.
Em Rust esse tipo precisaria de indireção para ter tamanho finito; o
compilador a insere sozinho, e `Box` nunca aparece no fonte Titan
([ADR 0026](docs/adr/0026-enum-recursivo-com-box-na-emissao.md)). É a
diferença em relação a `record`, que segue rejeitando recursão: um `enum`
expressa a base do recursivo numa variante sem payload, um `record` não teria
como.

`match` desmonta o valor, como comando ou como expressão. Os braços ligam os
campos a nomes locais, que só existem dentro do braço, e `_` é o curinga.

```lua
function avalia(e: Exp): integer
    local r: integer = match e with
        ExpInteger(n) then
            n
        ExpBinop(op, l, d) then
            aplica(op, avalia(l), avalia(d))
    end
    return r
end
```

> **A exaustividade é conferida pelo checker, em português.** Um `match` que
> deixa variante de fora não compila, e a mensagem diz **quais** faltam — em
> vez de o `rustc` reclamar em inglês sobre o `match` gerado, que o usuário não
> escreveu. Braço duplicado, variante inexistente, variante de outro `enum` e
> aridade errada dão erros distintos, porque levam a correções distintas. Um
> `_` que vem **depois** de todas as variantes é **aviso**, não erro: o código
> está correto, e o `_` é justamente o que se quer se o `enum` crescer. Já um
> braço de variante depois do `_` é **erro** — ele jamais executa.

```lua
enum Cor
    Vermelho
    Verde
    Azul
end

match c with
    Vermelho then
        return 0
end
-- erro: `match` não cobre todas as variantes de 'Cor': falta(m) Verde, Azul.
--       Acrescente um braço para cada uma, ou um `_`.
```

Nomes de variante são **únicos no programa inteiro**: a construção
`ExpInteger(42)` se escreve igual a uma chamada de função e não diz de que
`enum` ela vem, então é o nome que decide. Pela mesma razão, uma variante não
pode se chamar como uma função já declarada.

Um valor de tipo soma tem semântica de valor como qualquer composto
([ADR 0006](docs/adr/0006-semantica-de-valor-clone-na-atribuicao.md)): `local
b = a` copia, e mutar o que um braço ligou não alcança o valor escrutinado.
Mas ele **não** é passado por `&mut` como array/map/record — vai por valor,
como um escalar. Duas coisas que um `enum` não faz: subir para `value` (o
`value` guarda primitiva, array, map, record e opcional — extraia o conteúdo
com `match` primeiro) e atravessar a fronteira de FFI.

## O que não está implementado ainda

Ficam para fases futuras (veja o roadmap no [`PRD.md`](PRD.md)):

- `local m = import "data"` (a forma do original).
- **O pipeline auto-hospedado cobre um subconjunto, não a linguagem inteira.**
  `selfhost/lexer.titan`, `parser.titan` e `checker.titan` leem e checam o que
  os `examples/*.titan` usam — funções, `local`, `return`, `if`/`while`/`for`,
  atribuição, chamadas, record, array e map —, e não as 4498 linhas de
  `crates/titanc/src/checker.rs`. Ficam de fora, entre outros, `enum`/`match`,
  `repeat`/`until`, `continue`, `foreign function` e float. O compilador que
  compila é o `titanc`; o pipeline em Titan é a prova de que a linguagem já
  comporta escrevê-lo.
- `titan-crypto`, `titan-ai` (fases 3b/3c).

Qualquer construção fora desse subconjunto é rejeitada pelo `checker` (ou
pelo `parser`, quando a sintaxe já é o problema) com uma mensagem de erro em
português, nunca com um panic.

## Estrutura do workspace

```
titan-rust/
├── crates/
│   ├── titanc/          # o compilador como lib + bin: lexer, parser, checker, codegen, driver, CLI
│   ├── titan-runtime/   # runtime mínimo (print, concat) chamado pelo Rust gerado
│   ├── titan-data/      # capability `data`: leitura de CSV e agregações sobre Polars
│   ├── titan-texto/     # capability `texto`: acesso a texto por byte
│   ├── titan-io/        # capability `io`: leitura de arquivo
│   └── titan-lsp/       # language server: diagnósticos, hover, go-to-definition, autocomplete
├── editors/
│   └── vscode/          # extensão mínima: realce de sintaxe + cliente do titan-lsp
├── examples/
│   ├── hello.titan
│   ├── nucleo.titan
│   ├── compostos.titan
│   ├── dados.titan
│   ├── lexer.titan      # registro histórico da Fase 4 — não é "consertado"
│   └── vendas.csv
├── selfhost/            # o titanc escrito em Titan: projeto multi-módulo
│   ├── titan.toml       # manifesto: nome → caminho de cada módulo
│   ├── ast.titan        # a AST do Titan em Titan, sobre tipos soma
│   ├── lexer.titan      # o lexer, no idioma da Fase 5
│   ├── parser.titan     # descida recursiva produzindo a AST acima
│   ├── checker.titan    # a análise semântica: symtab em pilha, duas passadas
│   └── main.titan       # amarra as três peças e imprime a AST tipada
├── docs/
│   ├── arquitetura.md
│   └── adr/
└── build/                # gerado pelo titanc, não versionado
```
