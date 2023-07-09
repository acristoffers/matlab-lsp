/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

mod analysis;
mod args;
mod formatter;
mod handlers;
mod parsed_file;
mod session_state;
mod types;
mod utils;

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use crate::types::Workspace;
use crate::utils::lock_mutex;

use self::session_state::SessionState;
use self::utils::SessionStateArc;
use args::{Arguments, Parser};
use crossbeam_channel::Receiver;
use handlers::{handle_notification, handle_request};

use anyhow::Result;
use log::{debug, error, info};
use lsp_server::{Connection, Message};
use lsp_types::ServerCapabilities;
use lsp_types::{
    OneOf, PositionEncodingKind, TextDocumentSyncCapability, TextDocumentSyncKind,
    TextDocumentSyncOptions,
};
use process_alive::Pid;
use simplelog::{CombinedLogger, Config, WriteLogger};

fn main() -> ExitCode {
    let arguments = Arguments::parse();
    if let Err(err) = configure_logger() {
        error!("Error initializing logger: {err}");
        return ExitCode::FAILURE;
    }
    info!("################################################################################");
    info!("###                                                                          ###");
    info!("###                             Starting Server                              ###");
    info!("###                                                                          ###");
    info!("################################################################################");
    info!("Starting server with arguments: {arguments:?}");
    let r = match start_server(arguments) {
        Ok(exit_code) => exit_code,
        Err(err) => {
            error!("Error: {err}");
            ExitCode::FAILURE
        }
    };
    info!("Quitting with code: {:?}", r);
    r
}

fn configure_logger() -> Result<()> {
    let xdg_dirs = xdg::BaseDirectories::with_prefix("matlab-lsp")?;
    let info_log_path = xdg_dirs.place_cache_file("matlab-lsp.log")?;
    let debug_log_path = xdg_dirs.place_cache_file("matlab-lsp-debug.log")?;
    let mut open_options = OpenOptions::new();
    open_options
        .append(true)
        .create(true)
        .read(false)
        .write(true);
    let info_log_file = open_options.open(info_log_path)?;
    let debug_log_file = open_options.open(debug_log_path)?;
    let info_logger = WriteLogger::new(log::LevelFilter::Info, Config::default(), info_log_file);
    let debug_logger = WriteLogger::new(log::LevelFilter::Debug, Config::default(), debug_log_file);
    CombinedLogger::init(vec![info_logger, debug_logger])?;
    Ok(())
}

fn start_server(arguments: Arguments) -> Result<ExitCode> {
    let (connection, _io_threads) = Connection::stdio();
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
        definition_provider: Some(OneOf::Left(true)),
        ..Default::default()
    })?;
    let initialization_params = connection.initialize(server_capabilities)?;
    let session_state = SessionState {
        client_requested_shutdown: false,
        path: if let Some(path) = arguments.path {
            path.split(':').map(String::from).collect()
        } else {
            vec![]
        },
        sender: connection.sender,
        workspace_params: serde_json::from_value(initialization_params)?,
        files: HashMap::new(),
        workspace: Workspace::default(),
    };
    let pid = session_state.workspace_params.process_id;
    let session_state: &'static mut SessionState = Box::leak(Box::new(session_state));
    let state_arc: SessionStateArc = Arc::new(Mutex::new(session_state));
    let handle = SessionState::start_worker(Arc::clone(&state_arc))?;
    let result = main_loop(Arc::clone(&state_arc), &connection.receiver, pid);
    debug!("Left main loop. Joining threads and shutting down.");
    // info!("files: {:?}", lock_mutex(&state_arc).unwrap().files);
    info!("workspace: {:?}", lock_mutex(&state_arc).unwrap().workspace);
    match handle.join() {
        Err(err) => {
            error!("Thread paniced? {:?}", err.downcast_ref::<String>());
        }
        Ok(Ok(_)) => info!("Thread joined."),
        Ok(Err(err)) => {
            error!("Thread returned error: {err}");
        }
    }
    result
}

fn main_loop(
    state: SessionStateArc,
    receiver: &Receiver<Message>,
    pid: Option<u32>,
) -> Result<ExitCode> {
    loop {
        if let Some(pid) = pid {
            let pid = Pid::from(pid);
            if let process_alive::State::Dead = process_alive::state(pid) {
                break;
            }
        }
        if let Ok(msg) = receiver.recv() {
            match process_message(Arc::clone(&state), &msg) {
                Ok(Some(error_code)) => return Ok(error_code),
                Ok(None) => {}
                Err(err) => error!("Error processing {msg:?}: {err:?}"),
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn process_message(state: SessionStateArc, msg: &Message) -> Result<Option<ExitCode>> {
    match msg {
        Message::Request(req) => handle_request(Arc::clone(&state), req),
        Message::Response(_resp) => Ok(None),
        Message::Notification(not) => handle_notification(Arc::clone(&state), not),
    }
}
