# Titan para VS Code

Extensão mínima (PRD.md, T51) que dá ao VS Code:

- realce de sintaxe para arquivos `.titan` (`syntaxes/titan.tmLanguage.json`);
- cliente de language server (`vscode-languageclient`) que conversa com o
  binário `titan-lsp` (`crates/titan-lsp`, T48/T49/T50) via stdio, trazendo
  diagnósticos, hover, go-to-definition e autocomplete para o editor.

Esta fase **não** publica a extensão no marketplace — é só o cliente que
prova que o LSP funciona de ponta a ponta num editor de verdade.

## Pré-requisito: compilar o `titan-lsp`

Da raiz do workspace Rust (`titan-rust/`):

```bash
cargo build --release
```

Isso produz `target/release/titan-lsp`. A extensão procura o binário
`titan-lsp` no `PATH` por padrão — adicione `target/release/` ao `PATH`, ou
rode `cargo install --path crates/titan-lsp`, ou aponte diretamente para o
binário na configuração `titan.serverPath` do VS Code (Settings → Extensions
→ Titan → Server Path), com o caminho absoluto para
`target/release/titan-lsp`.

## Como rodar em modo de desenvolvimento (F5)

1. Instale as dependências e compile o cliente TypeScript:

   ```bash
   cd editors/vscode
   npm install
   npm run compile
   ```

2. Abra a pasta `editors/vscode/` no VS Code.
3. Pressione **F5** (ou Run → Start Debugging). Isso abre uma nova janela
   ("Extension Development Host") com a extensão Titan carregada.
4. Nessa nova janela, abra o repositório `titan-rust/` (ou qualquer pasta
   com arquivos `.titan`) e abra `examples/nucleo.titan`.

## Critério de aceite

- Abrir `examples/nucleo.titan` mostra realce de sintaxe (palavras-chave,
  tipos, strings, números, comentários).
- Introduzir um erro de tipo (ex.: trocar `n: integer` por uma comparação
  com string) sublinha a linha com a mensagem de erro em português, vinda
  do `titan-lsp`.

## Estrutura

```
editors/vscode/
├── package.json                       # manifesto da extensão
├── language-configuration.json        # comentários, pares de colchetes, indentação
├── syntaxes/titan.tmLanguage.json     # gramática TextMate
├── src/extension.ts                   # cliente vscode-languageclient
└── .vscode/{launch,tasks}.json        # suporte a F5
```
