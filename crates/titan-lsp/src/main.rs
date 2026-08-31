//! Language server do Titan (PRD.md, T48): abre um `.titan` e mostra o erro
//! sublinhado.
//!
//! Roda `lex → parse → check` sobre o buffer **em memória** a cada
//! `didOpen`/`didChange`; nunca invoca o `cargo` (PRD.md, decisão 6 da Fase
//! 4) — diferente de `driver::compile`, que gera Rust e builda de verdade.
//!
//! **Conversão de posição:** `ast::Loc` é 1-indexado e conta colunas em
//! bytes; o LSP é 0-indexado e usa UTF-16 por padrão. Em vez de converter
//! byte→UTF-16 a cada posição, o servidor anuncia `positionEncoding: "utf-8"`
//! na resposta de `initialize` (permitido pela spec desde a 3.17, negociado
//! via `general.positionEncodings` do cliente) — daí a conversão é só
//! `line - 1` / `col - 1`. Sem isso, um `.titan` com acento (o projeto
//! inteiro escreve em português) exporia posições erradas silenciosamente.
//!
//! `checker::check` devolve `Vec<CheckError>` — todos os erros de tipo saem
//! numa publicação só. `LexError`/`ParseError` param no primeiro erro; é uma
//! limitação aceita nesta fase (ver PRD.md, T48).

mod diagnostics;

use tower_lsp::jsonrpc::Result as RpcResult;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

use diagnostics::compute_diagnostics;

struct Backend {
    client: Client,
}

impl Backend {
    fn new(client: Client) -> Self {
        Backend { client }
    }

    async fn publish_for(&self, uri: Url, text: &str) {
        let diagnostics = compute_diagnostics(text);
        self.client
            .publish_diagnostics(uri, diagnostics, None)
            .await;
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _params: InitializeParams) -> RpcResult<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                position_encoding: Some(PositionEncodingKind::UTF8),
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                ..ServerCapabilities::default()
            },
            server_info: Some(ServerInfo {
                name: "titan-lsp".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
        })
    }

    async fn initialized(&self, _params: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "titan-lsp inicializado")
            .await;
    }

    async fn shutdown(&self) -> RpcResult<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        self.publish_for(params.text_document.uri, &params.text_document.text)
            .await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        // Sincronização FULL (anunciada em `initialize`): a última mudança
        // sempre carrega o texto completo do documento.
        if let Some(change) = params.content_changes.into_iter().next_back() {
            self.publish_for(params.text_document.uri, &change.text)
                .await;
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        self.client
            .publish_diagnostics(params.text_document.uri, Vec::new(), None)
            .await;
    }
}

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}
