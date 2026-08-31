//! Language server do Titan (PRD.md, T48/T49/T50): abre um `.titan` e mostra
//! o erro sublinhado, o tipo sob o cursor, salta para a declaração e
//! completa símbolos, membros de módulo e métodos.
//!
//! Roda `lex → parse → check` sobre o buffer **em memória** a cada
//! `didOpen`/`didChange`; nunca invoca o `cargo` (PRD.md, decisão 6 da Fase
//! 4) — diferente de `driver::compile`, que gera Rust e builda de verdade.
//!
//! **Conversão de posição:** `ast::Loc` é 1-indexado e conta colunas em
//! caracteres Unicode (o lexer itera `.chars()`, não bytes); o LSP é
//! 0-indexado e usa UTF-16 por padrão. A spec permite anunciar
//! `positionEncoding: "utf-32"` em `initialize` para evitar essa conversão,
//! mas **o cliente real desta fase não aceita** — `vscode-languageclient`
//! só oferece `general.positionEncodings: ["utf-16"]` e derruba a conexão
//! (`throw new Error("Unsupported position encoding...")`,
//! `client.js:835`) se o servidor responder qualquer coisa diferente de
//! `"utf-16"` (ou omitir o campo). Por isso o servidor anuncia
//! `positionEncoding: "utf-16"` de verdade e converte char→UTF-16 em
//! `loc_to_position`/`position_to_loc` (`analysis.rs`), em vez de
//! `line - 1` / `col - 1` direto. Acentos do português (`ç`, `ã`, `é`...)
//! são todos do BMP — 1 char, 1 unidade UTF-16 — então não expõem o erro; um
//! caractere fora do BMP (ex. emoji, coberto em teste unitário) exigiria 2
//! unidades UTF-16 e a conversão direta erraria a posição a partir dali.
//!
//! `checker::check` devolve `Vec<CheckError>` — todos os erros de tipo saem
//! numa publicação só. `LexError`/`ParseError` param no primeiro erro; é uma
//! limitação aceita nesta fase (ver PRD.md, T48).
//!
//! **Hover, go-to-definition (T49) e autocomplete (T50)** precisam do texto
//! do buffer numa posição arbitrária, não só no momento de
//! `didOpen`/`didChange` — por isso `Backend` guarda o último texto de cada
//! documento em `documents`. Hover/go-to-definition só respondem quando o
//! buffer atual tipa sem erro (ver `analysis.rs`); autocomplete remenda o
//! buffer antes de analisar, porque o texto no momento de completar quase
//! nunca tipa (ver `completion.rs`).

mod analysis;
mod completion;
mod diagnostics;
mod position;

use std::collections::HashMap;

use tokio::sync::Mutex;
use tower_lsp::jsonrpc::Result as RpcResult;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

use diagnostics::compute_diagnostics;

struct Backend {
    client: Client,
    /// Último texto conhecido de cada documento aberto, por URI — hover e
    /// go-to-definition (T49) reanalisam a partir daqui, já que os pedidos
    /// do LSP para esses métodos carregam só a posição, não o texto.
    documents: Mutex<HashMap<Url, String>>,
}

impl Backend {
    fn new(client: Client) -> Self {
        Backend {
            client,
            documents: Mutex::new(HashMap::new()),
        }
    }

    async fn publish_for(&self, uri: Url, text: &str) {
        let diagnostics = compute_diagnostics(text);
        self.client
            .publish_diagnostics(uri, diagnostics, None)
            .await;
    }

    async fn set_document(&self, uri: Url, text: String) {
        self.publish_for(uri.clone(), &text).await;
        self.documents.lock().await.insert(uri, text);
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _params: InitializeParams) -> RpcResult<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                position_encoding: Some(PositionEncodingKind::UTF16),
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                definition_provider: Some(OneOf::Left(true)),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec![".".to_string()]),
                    ..CompletionOptions::default()
                }),
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
        self.set_document(params.text_document.uri, params.text_document.text)
            .await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        // Sincronização FULL (anunciada em `initialize`): a última mudança
        // sempre carrega o texto completo do documento.
        if let Some(change) = params.content_changes.into_iter().next_back() {
            self.set_document(params.text_document.uri, change.text)
                .await;
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        self.documents.lock().await.remove(&params.text_document.uri);
        self.client
            .publish_diagnostics(params.text_document.uri, Vec::new(), None)
            .await;
    }

    async fn hover(&self, params: HoverParams) -> RpcResult<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        // Clona o texto e solta o lock antes de analisar: o mutex só
        // protege o `HashMap`, não o trabalho de lex/parse/check, que não
        // precisa serializar requisições de documentos diferentes.
        let Some(text) = self.documents.lock().await.get(&uri).cloned() else {
            return Ok(None);
        };
        let Some(checked) = analysis::analyze(&text) else {
            return Ok(None);
        };
        Ok(analysis::hover_at(&checked, &text, position))
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> RpcResult<Option<GotoDefinitionResponse>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let Some(text) = self.documents.lock().await.get(&uri).cloned() else {
            return Ok(None);
        };
        let Some(checked) = analysis::analyze(&text) else {
            return Ok(None);
        };
        Ok(
            analysis::definition_at(&checked, &text, uri, position)
                .map(GotoDefinitionResponse::Scalar),
        )
    }

    async fn completion(&self, params: CompletionParams) -> RpcResult<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let Some(text) = self.documents.lock().await.get(&uri).cloned() else {
            return Ok(None);
        };
        let items = completion::complete(&text, position);
        Ok(Some(CompletionResponse::Array(items)))
    }
}

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}
