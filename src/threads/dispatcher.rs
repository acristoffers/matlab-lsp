/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use crate::args::Arguments;
use crate::types::{
    DBArgument, DBOperation, DBRequest, DBTarget, MessagePayload, SenderThread, State,
    ThreadMessage, Workspace,
};

use anyhow::Result;
use crossbeam_channel::{Receiver, Sender};
use log::debug;
use lsp_server::{Message, RequestId};
use lsp_types::notification::{Cancel, Notification};
use lsp_types::{CancelParams, InitializeParams, NumberOrString};

pub fn start(
    arguments: Arguments,
    init: InitializeParams,
    receiver: Receiver<ThreadMessage>,
    handler_sender: Sender<ThreadMessage>,
    bw_sender: Sender<ThreadMessage>,
) -> Result<()> {
    let mut state = State {
        lib_path: arguments
            .path
            .unwrap_or("".into())
            .split(':')
            .map(String::from)
            .collect(),
        ws_path: if let Some(ws) = init.workspace_folders {
            ws.iter().map(|w| w.uri.path().to_string()).collect()
        } else if let Some(path) = init.root_uri {
            vec![path.path().to_string()]
        } else {
            vec![]
        },
        requests_queue: VecDeque::new(),
        notifications_queue: VecDeque::new(),
        responses_queue: VecDeque::new(),
        handler_idle: true,
        bw_idle: false,
        parsed_files: HashMap::new(),
        workspace: Workspace::default(),
        request_id: 0,
        bw_queue: VecDeque::new(),
        handler_queue: VecDeque::new(),
    };
    bw_sender.send(ThreadMessage {
        sender: SenderThread::Dispatcher,
        payload: MessagePayload::ScanPath(state.lib_path.clone()),
    })?;
    state.bw_queue.push_back(ThreadMessage {
        sender: SenderThread::Dispatcher,
        payload: MessagePayload::ScanWorkspace(state.ws_path.clone()),
    });
    state.bw_queue.push_back(ThreadMessage {
        sender: SenderThread::Dispatcher,
        payload: MessagePayload::ScanWorkspace(state.ws_path.clone()),
    });
    loop {
        if state.handler_idle {
            state.handler_idle = false;
            if let Some(not) = state.notifications_queue.pop_front() {
                handler_sender.send(ThreadMessage {
                    sender: SenderThread::Main,
                    payload: MessagePayload::LspMessage(Message::Notification(not)),
                })?;
            } else if let Some(resp) = state.responses_queue.pop_front() {
                handler_sender.send(ThreadMessage {
                    sender: SenderThread::Main,
                    payload: MessagePayload::LspMessage(Message::Response(resp)),
                })?;
            } else if let Some(req) = state.requests_queue.pop_front() {
                handler_sender.send(ThreadMessage {
                    sender: SenderThread::Main,
                    payload: MessagePayload::LspMessage(Message::Request(req)),
                })?;
            } else if let Some(msg) = state.handler_queue.pop_front() {
                handler_sender.send(msg)?;
            } else {
                state.handler_idle = true;
            }
        }
        if state.bw_idle {
            if let Some(msg) = state.bw_queue.pop_front() {
                state.bw_idle = false;
                bw_sender.send(msg)?;
            }
        }
        let msg = receiver.recv()?;
        if let SenderThread::Main = msg.sender {
            match msg.payload {
                MessagePayload::LspMessage(msg) => match msg {
                    Message::Notification(not) if not.method == Cancel::METHOD => {
                        let params: CancelParams = serde_json::from_value(not.params)?;
                        let id = match params.id {
                            NumberOrString::Number(n) => n,
                            NumberOrString::String(s) => s.parse().unwrap_or(0),
                        };
                        let id = RequestId::from(id);
                        state.requests_queue.retain(|r| r.id != id);
                        continue;
                    }
                    Message::Notification(not) => state.notifications_queue.push_back(not),
                    Message::Request(req) => state.requests_queue.push_back(req),
                    Message::Response(resp) => state.responses_queue.push_back(resp),
                },
                MessagePayload::Exit => {
                    handler_sender.send(msg.clone())?;
                    bw_sender.send(msg.clone())?;
                    break;
                }
                _ => {}
            }
        } else {
            match msg.payload {
                MessagePayload::Done => {
                    if let SenderThread::Handler = msg.sender {
                        state.handler_idle = true;
                    } else if let SenderThread::BackgroundWorker = msg.sender {
                        state.bw_idle = true;
                    }
                }
                MessagePayload::DB(req) => match msg.sender {
                    SenderThread::Handler => {
                        handle_db_transaction(&mut state, handler_sender.clone(), req, true)?
                    }
                    SenderThread::BackgroundWorker => {
                        handle_db_transaction(&mut state, bw_sender.clone(), req, false)?
                    }
                    _ => {}
                },
                MessagePayload::InitPath((files, functions)) => {
                    for file in files {
                        state.parsed_files.insert(file.path.clone(), file);
                    }
                    for function in functions {
                        let key = function.package.clone() + "." + &function.name;
                        let key = key.strip_prefix('.').map(String::from).unwrap_or(key);
                        state.workspace.functions.insert(key, function);
                    }
                }
                MessagePayload::ScanWorkspace(_) => state.bw_queue.push_back(ThreadMessage {
                    sender: SenderThread::Dispatcher,
                    payload: MessagePayload::ScanWorkspace(state.ws_path.clone()),
                }),
                MessagePayload::ScanOpen => state.handler_queue.push_back(ThreadMessage {
                    sender: SenderThread::Dispatcher,
                    payload: MessagePayload::ScanOpen,
                }),
                _ => {}
            }
        }
    }
    Ok(())
}

fn handle_db_transaction(
    state: &mut State,
    sender: Sender<ThreadMessage>,
    req: DBRequest,
    from_handler: bool,
) -> Result<()> {
    let argument = match &req.operation {
        //////////////////////////////////////////////////////////////////////////////
        //                                                                          //
        //                                   Get                                    //
        //                                                                          //
        //////////////////////////////////////////////////////////////////////////////
        DBOperation::Get => match &req.target {
            DBTarget::ParsedFile => match req.argument {
                DBArgument::String(path) => match state.parsed_files.get(&path) {
                    Some(file) => DBArgument::ParsedFile(Arc::clone(file)),
                    None => DBArgument::NotFound,
                },
                _ => DBArgument::NotFound,
            },
            DBTarget::Package => match req.argument {
                DBArgument::String(pkg) => DBArgument::Packages(
                    state
                        .workspace
                        .packages
                        .iter()
                        .filter(|p| p.starts_with(&pkg))
                        .map(String::from)
                        .collect(),
                ),
                _ => DBArgument::NotFound,
            },
            DBTarget::FunctionDefinition => match req.argument {
                DBArgument::String(path) => match state.workspace.functions.get(&path) {
                    Some(file) => DBArgument::FunctionDefinition(Arc::clone(file)),
                    None => DBArgument::NotFound,
                },
                _ => DBArgument::NotFound,
            },
            DBTarget::RequestID => {
                let id = state.request_id;
                state.request_id += 1;
                DBArgument::Integer(id)
            }
            DBTarget::Script => match req.argument {
                DBArgument::String(name) => {
                    if let Some(file) = state
                        .parsed_files
                        .values()
                        .map(Arc::clone)
                        .find(|f| f.is_script && f.name == name)
                    {
                        DBArgument::ParsedFile(file)
                    } else {
                        DBArgument::NotFound
                    }
                }
                _ => DBArgument::NotFound,
            },
        },
        //////////////////////////////////////////////////////////////////////////////
        //                                                                          //
        //                                   Set                                    //
        //                                                                          //
        //////////////////////////////////////////////////////////////////////////////
        DBOperation::Set => match &req.target {
            DBTarget::ParsedFile => match req.argument {
                DBArgument::ParsedFile(file) => {
                    if let Some(stored) = state.parsed_files.get(&file.path) {
                        if (stored.open && !from_handler) || stored.timestamp > file.timestamp {
                            return Ok(());
                        }
                    }
                    debug!("Setting file {file:?}");
                    state.parsed_files.insert(file.path.clone(), file);
                    return Ok(());
                }
                _ => DBArgument::NotFound,
            },
            DBTarget::Package => match req.argument {
                DBArgument::Packages(pkgs) => {
                    state.workspace.packages.extend(pkgs);
                    return Ok(());
                }
                _ => DBArgument::NotFound,
            },
            DBTarget::FunctionDefinition => match req.argument {
                DBArgument::FunctionDefinition(func) => {
                    let name = format!("{}.{}", func.package, func.name);
                    let name = name.strip_prefix('.').map(String::from).unwrap_or(name);
                    state.workspace.functions.insert(name, func);
                    return Ok(());
                }
                _ => DBArgument::NotFound,
            },
            DBTarget::RequestID => DBArgument::NotFound,
            DBTarget::Script => DBArgument::NotFound,
        },
        //////////////////////////////////////////////////////////////////////////////
        //                                                                          //
        //                                  Delete                                  //
        //                                                                          //
        //////////////////////////////////////////////////////////////////////////////
        DBOperation::Delete => match &req.target {
            DBTarget::ParsedFile => match req.argument {
                DBArgument::String(path) => {
                    state.parsed_files.remove(&path);
                    return Ok(());
                }
                _ => DBArgument::NotFound,
            },
            DBTarget::Package => DBArgument::NotFound,
            DBTarget::Script => DBArgument::NotFound,
            DBTarget::FunctionDefinition => match req.argument {
                DBArgument::String(path) => {
                    state.workspace.functions.retain(|_, f| f.path != path);
                    return Ok(());
                }
                _ => DBArgument::NotFound,
            },
            DBTarget::RequestID => DBArgument::NotFound,
        },
        //////////////////////////////////////////////////////////////////////////////
        //                                                                          //
        //                                  Fetch                                   //
        //                                                                          //
        //////////////////////////////////////////////////////////////////////////////
        DBOperation::Fetch => match &req.target {
            DBTarget::ParsedFile => DBArgument::ParsedFiles(state.parsed_files.clone()),
            DBTarget::Package => DBArgument::NotFound,
            DBTarget::Script => DBArgument::ParsedFiles(
                state
                    .parsed_files
                    .iter()
                    .filter(|(_, f)| f.is_script)
                    .map(|(k, v)| (k.clone(), Arc::clone(v)))
                    .collect(),
            ),
            DBTarget::FunctionDefinition => {
                DBArgument::FunctionDefinitions(state.workspace.functions.clone())
            }
            DBTarget::RequestID => DBArgument::NotFound,
        },
    };
    sender.send(ThreadMessage {
        sender: SenderThread::Dispatcher,
        payload: MessagePayload::DB(DBRequest {
            operation: req.operation,
            target: req.target,
            argument,
        }),
    })?;
    Ok(())
}
