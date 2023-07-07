/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

mod args;
mod formatter;
mod handlers;
mod parsed_file;
mod session_state;
mod utils;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use self::session_state::SessionState;
use self::utils::SessionStateArc;
use args::{Arguments, Parser};
use crossbeam_channel::Receiver;
use handlers::{handle_notification, handle_request};

use anyhow::Result;
use lsp_server::{Connection, Message};
use lsp_types::ServerCapabilities;
use lsp_types::{
    OneOf, PositionEncodingKind, TextDocumentSyncCapability, TextDocumentSyncKind,
    TextDocumentSyncOptions,
};

fn main() -> Result<()> {
    let arguments = Arguments::parse();
    start_server(arguments)?;
    Ok(())
}

fn start_server(arguments: Arguments) -> Result<()> {
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

    let session_state = SessionState {
        files: HashMap::new(),
        workspace: serde_json::from_value(initialization_params)?,
        sender: connection.sender,
        path: if let Some(path) = arguments.path {
            path.split(':').map(|p| p.to_string()).collect()
        } else {
            vec![]
        },
    };
    let session_state: &'static mut SessionState = Box::leak(Box::new(session_state));
    let state_arc: SessionStateArc = Arc::new(Mutex::new(session_state));

    let handles = SessionState::parse_path_async(Arc::clone(&state_arc))?;
    main_loop(Arc::clone(&state_arc), &connection.receiver)?;

    io_threads.join()?;

    for handle in handles {
        let _ = handle.join();
    }

    Ok(())
}

fn main_loop(state: SessionStateArc, receiver: &Receiver<Message>) -> Result<()> {
    for msg in receiver {
        match process_message(Arc::clone(&state), &msg) {
            Ok(true) => break,
            Ok(false) => continue,
            Err(err) => eprintln!("Error processing {msg:?}: {err:?}"),
        }
    }
    Ok(())
}

fn process_message(state: SessionStateArc, msg: &Message) -> Result<bool> {
    match msg {
        Message::Request(req) => handle_request(Arc::clone(&state), req),
        Message::Response(resp) => {
            eprintln!("got response: {resp:?}");
            Ok(false)
        }
        Message::Notification(not) => handle_notification(Arc::clone(&state), not),
    }
}
