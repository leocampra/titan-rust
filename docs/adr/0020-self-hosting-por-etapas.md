# ADR 0020 — Self-hosting por etapas: lexer na Fase 4, parser/checker na Fase 5

## Status

Aceito.

## Contexto

O roadmap sempre listou a fase de self-hosting como "compilador escrito na
própria linguagem — complexidade muito alta", sem detalhar o que cabia numa
fase só. A investigação que abre a Fase 4 mediu as lacunas concretas entre o
que a linguagem tem e o que um `titanc` escrito em Titan exigiria:

| Lacuna | Onde é rejeitada | Consequência |
|---|---|---|
| Tipos soma (`enum`/`match`) | não existem (`types.rs`) | um `Exp` de 13 variantes (`ast.rs`) não tem representação |
| Módulos de usuário | `driver.rs` lê **um** arquivo | o compilador (4144 linhas só de `checker.rs`) teria de caber num único `.titan` |
| Ler arquivo | não havia capability de I/O | o compilador não alcançava o próprio fonte |
| Acesso a caractere de string | não havia | um lexer não avança sobre o fonte |
| `string` ↔ número | não havia `tonumber`/`tostring` | literal numérico não vira valor |
| `break`/`continue` | nem eram keywords | laço de scan vira flag booleana |
| Retornos múltiplos | não existem | `(token, pos)` vira record ou `&mut` |

Fechar todas as sete numa fase só significaria projetar tipos soma e módulos
de usuário — cada um do tamanho de uma fase inteira própria — sob pressão de
tempo, arriscando as duas ao mesmo tempo. A alternativa adotada foi recortar:
resolver as lacunas *baratas* (`texto`, `io`, `break`) e usar o resultado para
provar self-hosting **parcial** — um lexer, não o compilador inteiro — deixando
tipos soma, módulos de usuário e parser/checker auto-hospedados
explicitamente para depois.

## Decisão

A Fase 4 entrega self-hosting só do lexer: `examples/lexer.titan` (T56), um
lexer para o subconjunto de Titan usado em `examples/nucleo.titan`, escrito
em Titan, sobre as capabilities `texto`/`io` e o `break` novos desta mesma
fase. Tipos soma, módulos de usuário e parser/checker auto-hospedados ficam
para a Fase 5 ("self-hosting pleno").

O estilo resultante em `lexer.titan` é deliberadamente deselegante, e isso é
registrado no próprio arquivo, não escondido:

- `TokenKind` é `integer`, não tipo soma — sem variáveis de topo, as
  constantes viram funções sem argumento (`function TK_NAME(): integer
  return 1 end`).
- O estado da varredura (posição, linha, coluna) anda num `record Estado`
  passado por parâmetro, porque reatribuir um parâmetro escalar é proibido
  (`checker.rs:1293-1296`) e não há retorno múltiplo — ADR 0007
  (parâmetros compostos por `&mut`) trabalhando a favor.
- Tokens acumulam num `{Token}` via `res[#res + 1] = ...`, o idioma de
  append da Fase 2.

## Consequências

- A prova de self-hosting é real, mas parcial: `titanc examples/lexer.titan
  && ./lexer examples/nucleo.titan` tokeniza um programa Titan de verdade,
  mas o lexer não é o lexer *de produção* do `titanc` (esse continua em
  `lexer.rs`, Rust) — é uma demonstração de que a linguagem já é capaz de
  expressar essa classe de programa.
- A deselegância de `lexer.titan` (constantes-como-função, record de estado,
  tag inteira em vez de tipo soma) é a evidência empírica, não hipotética,
  que justifica o escopo da Fase 5: tipos soma e variáveis de topo deixam de
  ser "features desejáveis" abstratas e passam a ser "o que teria evitado
  isto", com um exemplo real como referência.
- Resistir à tentação de "consertar" a linguagem no meio da Fase 4 (risco 4
  do PRD.md) foi deliberado — adicionar tipo soma sob pressão para deixar
  `lexer.titan` mais bonito arriscaria repetir o erro de tentar caber duas
  fases numa.
- Parser e checker auto-hospedados (o resto do compilador escrito em Titan)
  permanecem fora de escopo até a Fase 5 ter tipos soma e módulos de
  usuário — sem os dois, um checker de 4144 linhas não tem onde morar nem
  como representar sua própria AST.
