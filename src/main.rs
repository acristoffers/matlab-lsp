/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

mod args;
mod extractors;
mod features;
mod handlers;
mod impls;
mod threads;
mod types;
mod utils;

use std::fs::OpenOptions;
use std::process::ExitCode;
use std::thread::{spawn, JoinHandle};

use args::{Arguments, Parser};
use threads::{background_worker, dispatcher, handler};
use types::{MessagePayload, SenderThread, ThreadMessage};

use anyhow::Result;
use crossbeam_channel::{Receiver, Sender};
use log::{debug, error, info};
use lsp_server::{Connection, Message};
use lsp_types::notification::{Exit, Notification};
use lsp_types::{
    CompletionOptions, FoldingRangeProviderCapability, HoverProviderCapability, InitializeParams,
    OneOf, PositionEncodingKind, SaveOptions, SemanticTokenType, SemanticTokensFullOptions,
    SemanticTokensLegend, SemanticTokensOptions, SemanticTokensServerCapabilities,
    ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind, TextDocumentSyncOptions,
    WorkDoneProgressOptions,
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
    let xdg_dirs = xdg::BaseDirectories::with_prefix("matlab-lsp");
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
    let server_capabilities = serde_json::to_value(server_capabilities())?;
    let initialization_params = connection.initialize(server_capabilities)?;
    let initialization_params: InitializeParams = serde_json::from_value(initialization_params)?;
    let pid = initialization_params.process_id;
    let (threads, sender) =
        start_threads(arguments, initialization_params, connection.sender.clone());
    let result = main_loop(sender, connection.receiver.clone(), pid);
    debug!("Left main loop. Joining threads and shutting down.");
    for handle in threads {
        match handle.join() {
            Err(err) => {
                error!("Thread paniced? {:?}", err.downcast_ref::<String>());
            }
            Ok(Ok(_)) => info!("Thread joined."),
            Ok(Err(err)) => {
                error!("Thread returned error: {err}");
            }
        }
    }
    result
}

fn server_capabilities() -> ServerCapabilities {
    let semantic_token_types = vec![
        SemanticTokenType::NAMESPACE,
        SemanticTokenType::TYPE,
        SemanticTokenType::CLASS,
        SemanticTokenType::ENUM,
        SemanticTokenType::INTERFACE,
        SemanticTokenType::STRUCT,
        SemanticTokenType::TYPE_PARAMETER,
        SemanticTokenType::PARAMETER,
        SemanticTokenType::VARIABLE,
        SemanticTokenType::PROPERTY,
        SemanticTokenType::ENUM_MEMBER,
        SemanticTokenType::EVENT,
        SemanticTokenType::FUNCTION,
        SemanticTokenType::METHOD,
        SemanticTokenType::MACRO,
        SemanticTokenType::KEYWORD,
        SemanticTokenType::MODIFIER,
        SemanticTokenType::COMMENT,
        SemanticTokenType::STRING,
        SemanticTokenType::NUMBER,
        SemanticTokenType::REGEXP,
        SemanticTokenType::OPERATOR,
    ];
    ServerCapabilities {
        position_encoding: Some(PositionEncodingKind::UTF8),
        text_document_sync: Some(TextDocumentSyncCapability::Options(
            TextDocumentSyncOptions {
                change: Some(TextDocumentSyncKind::INCREMENTAL),
                open_close: Some(true),
                will_save: Some(false),
                will_save_wait_until: Some(false),
                save: Some(
                    SaveOptions {
                        include_text: Some(true),
                    }
                    .into(),
                ),
            },
        )),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        completion_provider: Some(CompletionOptions {
            resolve_provider: Some(false),
            trigger_characters: Some(vec![".".to_string()]),
            all_commit_characters: None,
            work_done_progress_options: WorkDoneProgressOptions {
                work_done_progress: Some(false),
            },
            completion_item: None,
        }),
        definition_provider: Some(OneOf::Left(true)),
        references_provider: Some(OneOf::Left(true)),
        document_highlight_provider: Some(OneOf::Left(true)),
        document_formatting_provider: Some(lsp_types::OneOf::Left(true)),
        rename_provider: Some(OneOf::Left(true)),
        folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
        semantic_tokens_provider: Some(SemanticTokensServerCapabilities::SemanticTokensOptions(
            SemanticTokensOptions {
                work_done_progress_options: WorkDoneProgressOptions {
                    work_done_progress: Some(false),
                },
                legend: SemanticTokensLegend {
                    token_types: semantic_token_types,
                    token_modifiers: vec![],
                },
                range: None,
                full: Some(SemanticTokensFullOptions::Bool(true)),
            },
        )),
        ..Default::default()
    }
}

fn start_threads(
    arguments: Arguments,
    init: InitializeParams,
    lsp_sender: Sender<Message>,
) -> (Vec<JoinHandle<Result<()>>>, Sender<ThreadMessage>) {
    let mut handlers = vec![];
    let (dispatcher_sender, dispatcher_receiver) = crossbeam_channel::unbounded();
    let (handler_sender, handler_receiver) = crossbeam_channel::unbounded();
    let (bw_sender, bw_receiver) = crossbeam_channel::unbounded();
    let handler = spawn(move || -> Result<()> {
        dispatcher::start(
            arguments,
            init,
            dispatcher_receiver,
            handler_sender,
            bw_sender,
        )
    });
    handlers.push(handler);
    let ds_clone = dispatcher_sender.clone();
    let lsp_sender_clone = lsp_sender.clone();
    let handler = spawn(move || -> Result<()> {
        handler::start(lsp_sender_clone, ds_clone, handler_receiver)
    });
    handlers.push(handler);
    let ds_clone = dispatcher_sender.clone();
    let handler = spawn(move || -> Result<()> {
        background_worker::start(lsp_sender, ds_clone, bw_receiver)
    });
    handlers.push(handler);
    (handlers, dispatcher_sender)
}

fn main_loop(
    sender: Sender<ThreadMessage>,
    receiver: Receiver<Message>,
    pid: Option<u32>,
) -> Result<ExitCode> {
    loop {
        if let Some(pid) = pid {
            let pid = Pid::from(pid);
            if let process_alive::State::Dead = process_alive::state(pid) {
                info!("Editor is dead, leaving.");
                sender.send(ThreadMessage {
                    sender: SenderThread::Main,
                    payload: MessagePayload::Exit,
                })?;
                break;
            }
        }
        if let Ok(msg) = receiver.recv() {
            if let Message::Notification(not) = &msg {
                if not.method == Exit::METHOD {
                    sender.send(ThreadMessage {
                        sender: SenderThread::Main,
                        payload: MessagePayload::Exit,
                    })?;
                    break;
                }
            }
            sender.send(ThreadMessage {
                sender: SenderThread::Main,
                payload: MessagePayload::LspMessage(msg),
            })?;
        }
    }
    Ok(ExitCode::SUCCESS)
}
