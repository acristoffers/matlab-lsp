/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

mod args;
mod utils;

use args::{Arguments, Parser};
use utils::*;

use anyhow::Result;
use lsp_server::{Connection, ExtractError, Message, Response};
use lsp_types::{
    request::Formatting, GotoDefinitionResponse, InitializeParams, ServerCapabilities,
};
use lsp_types::{
    OneOf, PositionEncodingKind, TextDocumentSyncCapability, TextDocumentSyncKind,
    TextDocumentSyncOptions,
};

fn main() -> Result<()> {
    let _ = Arguments::parse();
    start_server()?;
    Ok(())
}

fn start_server() -> Result<()> {
    eprintln!("Starting server...");
    let (connection, io_threads) = Connection::stdio();

    let server_capabilities = serde_json::to_value(ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Options(
            TextDocumentSyncOptions {
                change: Some(TextDocumentSyncKind::INCREMENTAL),
                open_close: Some(true),
                will_save: Some(false),
                will_save_wait_until: Some(false),
                save: Some(lsp_types::TextDocumentSyncSaveOptions::Supported(false)),
            },
        )),
        position_encoding: Some(PositionEncodingKind::UTF8),
        document_formatting_provider: Some(OneOf::Left(true)),
        ..Default::default()
    })?;

    let initialization_params = connection.initialize(server_capabilities)?;

    main_loop(connection, initialization_params)?;

    io_threads.join()?;

    Ok(())
}

fn main_loop(connection: Connection, params: serde_json::Value) -> Result<()> {
    let _params: InitializeParams = serde_json::from_value(params)?;
    for msg in &connection.receiver {
        eprintln!("got msg: {msg:?}");
        match msg {
            Message::Request(req) => {
                if connection.handle_shutdown(&req)? {
                    eprintln!("lsp shutting down");
                    return Ok(());
                }
                eprintln!("got request: {req:?}");
                // match cast::<Formatting>(req) {
                //     Ok((id, params)) => {
                //         eprintln!("got format request #{id}: {params:?}");
                //         let result = Some(GotoDefinitionResponse::Array(Vec::new()));
                //         let result = serde_json::to_value(&result).unwrap();
                //         let resp = Response {
                //             id,
                //             result: Some(result),
                //             error: None,
                //         };
                //         connection.sender.send(Message::Response(resp))?;
                //         continue;
                //     }
                //     Err(err @ ExtractError::JsonError { .. }) => panic!("{err:?}"),
                //     Err(ExtractError::MethodMismatch(req)) => req,
                // };
            }
            Message::Response(resp) => {
                eprintln!("got response: {resp:?}");
            }
            Message::Notification(not) => {
                eprintln!("got notification: {not:?}");
            }
        }
    }
    Ok(())
}
