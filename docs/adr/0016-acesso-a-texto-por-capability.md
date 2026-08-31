# ADR 0016 — Acesso a texto por capability `texto`, não builtins globais nem `s[i]`

## Status

Aceito.

## Contexto

Um lexer escrito em Titan (T56) precisa andar sobre o fonte: ler um byte numa
posição, fatiar um trecho, converter entre `string` e `integer`. Três formas
de expor isso foram consideradas antes de começar a Fase 4:

1. **Builtins globais** (`byte(s, i)`, no molde de `#s`/`print`). Rejeitada
   porque `builtins.rs` é uma tabela **plana**, sem namespace — cada nome novo
   compete pelo mesmo espaço que `print`, `tostring` e futuras capabilities, e
   o precedente de `titan-data` (Fase 3) já tinha estabelecido `import` como o
   mecanismo para superfície de tamanho médio.
2. **Indexação direta, `s[i]`**. Rejeitada por ser assimétrica: leitura faria
   sentido (`s[i]` devolvendo um byte), mas escrita não (`string` é imutável
   por valor, [ADR 0010](0010-string-sempre-string.md)) — e ainda deixaria
   `sub`/`para_inteiro`/`de_inteiro` sem um lugar natural, já que não são
   indexação.
3. **Capability `texto`**, no molde exato de `titan-data`: um crate novo
   (`titan-texto`) com entrada em `capabilities.rs`, sem mudança em checker ou
   codegen.

## Decisão

Acesso a texto entra como capability `texto` (`import texto`), com cinco
funções de módulo (`crates/titan-texto/src/lib.rs`): `byte(s, i)`,
`sub(s, i, j)`, `para_inteiro(s)`, `de_inteiro(n)` e `tamanho(s)`. Todas
1-indexadas, coerentes com a indexação de array já existente
([ADR 0008](0008-indexacao-checada-e-variancia-invariante.md)), e operam sobre
**bytes** — coerente com `#s` (`titan-runtime/src/lib.rs:134-136`, que já
conta bytes), não "caracteres". Cada função segue o par `_checked -> Result<_,
String>` mais wrapper que aborta com mensagem em português, o mesmo molde de
`titan-data` ([ADR 0015](0015-api-data-como-contrato-backend-trocavel.md)).

O mecanismo de capability (`import`, namespace de módulo, `capabilities.rs`
como fonte única de verdade) já existia desde a Fase 3 — `texto` é a segunda
capability a usá-lo, não uma extensão do mecanismo.

## Consequências

- `texto` é a prova de que o mecanismo de capability da Fase 3 generaliza:
  entrar uma segunda capability não exigiu tocar checker nem codegen, só
  `capabilities.rs` mais o crate novo.
- Limitação documentada, não escondida: o fonte é assumido ASCII. Uma string
  com acento (UTF-8 multi-byte) tem comportamento *definido* — os bytes são
  indexados individualmente — mas não corresponde a "caracteres". Um lexer
  sobre fonte em português (como o próprio `titan-rust`) precisaria de
  indexação por *char*, fora de escopo desta fase.
- `s[i]` continua fora da linguagem — não é reconsiderado por `texto` existir;
  a decisão 2 acima permanece válida enquanto `string` for imutável por valor.
- `texto` é puro (nenhuma chamada de sistema); I/O foi deliberadamente
  separado numa capability à parte, `io` (`crates/titan-io`) — decisão 3 do
  PRD.md, Fase 4: `texto` nunca toca o sistema, `io` sim.
