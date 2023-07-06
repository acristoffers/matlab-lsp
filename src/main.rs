/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

mod args;
mod formatter;
mod global_state;
mod parsed_code;
mod utils;

use std::collections::HashMap;

use self::global_state::GlobalState;
use self::parsed_code::ParsedCode;
use args::{Arguments, Parser};
use utils::*;

use anyhow::{anyhow, Context, Result};
use lsp_server::{Connection, ExtractError, Message, Response};
use lsp_types::{request::Formatting, ServerCapabilities};
use lsp_types::{
    DidOpenTextDocumentParams, OneOf, Position, PositionEncodingKind, TextDocumentSyncCapability,
    TextDocumentSyncKind, TextDocumentSyncOptions, TextEdit,
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
    let mut global_state = GlobalState {
        files: HashMap::new(),
        workspace: serde_json::from_value(params)?,
    };
    for msg in &connection.receiver {
        eprintln!("got msg: {msg:?}");
        let result = process_message(&connection, &msg, &mut global_state);
        if result.is_err() {
            eprintln!("Something went wrong when processing {msg:?}: {result:?}");
        }
    }
    Ok(())
}

fn process_message(
    connection: &Connection,
    msg: &Message,
    global_state: &mut GlobalState,
) -> Result<()> {
    match msg {
        Message::Request(req) => {
            if connection.handle_shutdown(req)? {
                eprintln!("lsp shutting down");
                return Ok(());
            }
            eprintln!("got request: {req:?}");
            match cast::<Formatting>(req.clone()) {
                Ok((id, params)) => {
                    eprintln!("got format request #{id}: {params:?}");
                    let file = global_state
                        .files
                        .get_mut(&params.text_document.uri.to_string())
                        .with_context(|| "No file parsed")?;
                    let pos = file.tree.as_ref().unwrap().root_node().end_position();
                    eprintln!("End position: {pos:?}");
                    if let Some(code) = file.format() {
                        eprintln!("File formatted!!! Sending back...");
                        let result = vec![TextEdit {
                            range: lsp_types::Range {
                                start: Position::new(0, 0),
                                end: Position::new(pos.row.try_into()?, pos.column.try_into()?),
                            },
                            new_text: code,
                        }];
                        let result = serde_json::to_value(result).unwrap();
                        let resp = Response {
                            id,
                            result: Some(result),
                            error: None,
                        };
                        eprintln!("Sending response {resp:?}");
                        connection.sender.send(Message::Response(resp))?;
                    } else {
                        eprintln!("Error formatting!");
                        return Err(anyhow!("Error formatting"));
                    }
                }
                Err(err @ ExtractError::JsonError { .. }) => panic!("{err:?}"),
                Err(ExtractError::MethodMismatch(_req)) => {}
            };
        }
        Message::Response(resp) => {
            eprintln!("got response: {resp:?}");
        }
        Message::Notification(not) => {
            eprintln!("got notification: {not:?}");
            match not.method.as_str() {
                "textDocument/didOpen" => {
                    let params: DidOpenTextDocumentParams =
                        serde_json::from_value(not.params.clone())?;
                    let mut parsed_code = ParsedCode {
                        file: params.text_document.uri,
                        contents: params.text_document.text,
                        tree: None,
                    };
                    parsed_code.parse()?;
                    global_state
                        .files
                        .insert(parsed_code.file.to_string(), parsed_code);
                }
                "textDocument/didClose" => {}
                _ => {}
            }
        }
    }
    Ok(())
}
