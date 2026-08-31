# ADR 0019 — LSP sobre `tower-lsp`; deps do servidor nunca entram no `Cargo.toml` gerado

## Status

Aceito.

## Contexto

Duas decisões separadas precisavam de registro: qual biblioteca implementa o
protocolo LSP, e como impedir que as dependências desse servidor vazem para
os programas compilados pelo `titanc`.

**Escolha de biblioteca.** Implementar o protocolo LSP (JSON-RPC sobre
stdio, ciclo de vida de `initialize`, roteamento de notificação/requisição)
à mão era possível, mas reimplementaria protocolo já resolvido no
ecossistema Rust — o mesmo raciocínio que levou a Polars em
[ADR 0015](0015-api-data-como-contrato-backend-trocavel.md). `tower-lsp`
(`crates/titan-lsp/Cargo.toml`) foi escolhido: expõe o protocolo como um
trait (`LanguageServer`) sobre `tokio`, e lida com a serialização
`serde_json` das mensagens.

**Armadilha de posicionamento, descoberta em T48.** `ast::Loc` é 1-indexado e
conta colunas em **caracteres** Unicode (o lexer itera `.chars()`); o
protocolo LSP é 0-indexado. O PRD original cogitava anunciar
`positionEncoding: "utf-8"` para evitar conversão — mas o cliente real desta
fase, `vscode-languageclient`, só oferece `general.positionEncodings:
["utf-16"]` e **derruba a conexão** se o servidor responder qualquer coisa
diferente de `"utf-16"` (ou omitir o campo): `throw new Error("Unsupported
position encoding...")` em `client.js:835`. `"utf-8"` não era uma opção
disponível na prática, apesar de ser o que a spec do protocolo permite em
tese.

**Vazamento de dependências.** `titan-lsp` depende de `titanc`, `tower-lsp`,
`tokio` e `serde_json` — nenhuma dessas é algo que um programa `.titan`
compilado deveria herdar. `collect_deps` (`driver.rs:158-170`), que monta o
`Cargo.toml` gerado por programa, só itera `checker::imported_capabilities`
— `titan-lsp` não é uma capability, não tem entrada em `capabilities.rs`, e
portanto não tem como entrar nessa lista por engano.

## Decisão

`titan-lsp` (`crates/titan-lsp`) usa `tower-lsp` + `tokio` + `serde_json`,
como dependências normais do **workspace do compilador**
(`crates/titan-lsp/Cargo.toml`), nunca do `Cargo.toml` gerado por programa.

Sobre posicionamento: o servidor anuncia `positionEncoding: "utf-16"` de
verdade (`PositionEncodingKind::UTF16`, `main.rs:85`) e converte
explicitamente char→UTF-16 em `loc_to_position`/`position_to_loc`
(`position.rs`), em vez de assumir `line - 1` / `col - 1` direto. Acentos do
português (`ç`, `ã`, `é`...) são todos do BMP — 1 char, 1 unidade UTF-16 —
então não expõem o erro por si só; um caractere fora do BMP (emoji) ocupa 2
unidades UTF-16 e exige a conversão de verdade, coberta por teste unitário
em `position.rs`.

Sobre isolamento de dependências: nenhuma mudança de código é necessária —
`collect_deps` já não tem caminho para incluir `titan-lsp`, porque a função
só enxerga capabilities importadas. A garantia é fechada por um teste (T57,
`integration.rs:131`) que confere que o `Cargo.toml` gerado **não** contém
`"titan-lsp"`.

## Consequências

- O risco de posicionamento (risco 2 do PRD.md, Fase 4) era "silencioso" —
  só um `.titan` com acento em posição específica exporia o bug. Resolvido em
  T48/T49 com teste explícito de caractere fora do BMP, não deixado para o
  cliente descobrir.
- A escolha de `"utf-16"` sobre `"utf-8"` foi forçada pelo cliente
  disponível (`vscode-languageclient`), não por preferência — se um cliente
  LSP diferente for adotado no futuro e aceitar `"utf-8"`/`"utf-32"`, a
  conversão em `position.rs` pode ser simplificada ou removida, mas isso
  exigiria revisitar esta decisão, não presumi-la.
- O isolamento de dependências não depende de disciplina do desenvolvedor —
  é estrutural (`collect_deps` só olha `imported_capabilities`) e testado
  (T57). Adicionar uma nova dependência ao LSP no futuro não corre risco de
  vazar para programas compilados, mesmo sem revisão manual.
- `titan-lsp` nunca invoca `cargo` (decisão 6 do PRD.md, Fase 4,
  [ADR 0018](0018-titanc-lib-lsp-reusa-pipeline.md)) — reforça por que suas
  dependências não têm relação nenhuma com o que um programa `.titan`
  precisa em tempo de execução.
