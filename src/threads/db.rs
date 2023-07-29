/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use crossbeam_channel::{Receiver, Sender};
use log::debug;

use crate::types::{
    DBArgument, DBOperation, DBRequest, DBTarget, FunctionDefinition, MessagePayload, ParsedFile,
    SenderThread, ThreadMessage,
};

pub fn db_get_parsed_file(
    sender: &Sender<ThreadMessage>,
    receiver: &Receiver<ThreadMessage>,
    path: String,
    sender_thread: SenderThread,
) -> Option<Arc<ParsedFile>> {
    if sender
        .send(ThreadMessage {
            sender: sender_thread,
            payload: MessagePayload::DB(DBRequest {
                operation: DBOperation::Get,
                target: DBTarget::ParsedFile,
                argument: DBArgument::String(path.to_string()),
            }),
        })
        .is_ok()
    {
        if let Ok(response) = receiver.recv() {
            if let MessagePayload::DB(response) = response.payload {
                if let DBArgument::ParsedFile(file) = response.argument {
                    return Some(file);
                }
            }
        }
    }
    None
}

pub fn db_set_parsed_file(
    sender: &Sender<ThreadMessage>,
    file: Arc<ParsedFile>,
    sender_thread: SenderThread,
) -> Result<()> {
    sender.send(ThreadMessage {
        sender: sender_thread,
        payload: MessagePayload::DB(DBRequest {
            operation: DBOperation::Set,
            target: DBTarget::ParsedFile,
            argument: DBArgument::ParsedFile(file),
        }),
    })?;
    Ok(())
}

pub fn db_delete_parsed_file(
    sender: &Sender<ThreadMessage>,
    path: String,
    sender_thread: SenderThread,
) -> Result<()> {
    sender.send(ThreadMessage {
        sender: sender_thread,
        payload: MessagePayload::DB(DBRequest {
            operation: DBOperation::Delete,
            target: DBTarget::ParsedFile,
            argument: DBArgument::String(path),
        }),
    })?;
    Ok(())
}

pub fn db_fetch_parsed_files(
    sender: &Sender<ThreadMessage>,
    receiver: &Receiver<ThreadMessage>,
    sender_thread: SenderThread,
) -> Option<HashMap<String, Arc<ParsedFile>>> {
    if sender
        .send(ThreadMessage {
            sender: sender_thread,
            payload: MessagePayload::DB(DBRequest {
                operation: DBOperation::Fetch,
                target: DBTarget::ParsedFile,
                argument: DBArgument::NotFound,
            }),
        })
        .is_ok()
    {
        if let Ok(response) = receiver.recv() {
            if let MessagePayload::DB(response) = response.payload {
                if let DBArgument::ParsedFiles(fs) = response.argument {
                    return Some(fs);
                }
            }
        }
    }
    None
}

pub fn db_get_script(
    sender: &Sender<ThreadMessage>,
    receiver: &Receiver<ThreadMessage>,
    name: String,
    sender_thread: SenderThread,
) -> Option<Arc<ParsedFile>> {
    if sender
        .send(ThreadMessage {
            sender: sender_thread,
            payload: MessagePayload::DB(DBRequest {
                operation: DBOperation::Get,
                target: DBTarget::Script,
                argument: DBArgument::String(name.to_string()),
            }),
        })
        .is_ok()
    {
        if let Ok(response) = receiver.recv() {
            debug!("Got response.");
            if let MessagePayload::DB(response) = response.payload {
                if let DBArgument::ParsedFile(file) = response.argument {
                    return Some(file);
                }
            }
        }
    }
    None
}

pub fn db_fetch_script(
    sender: &Sender<ThreadMessage>,
    receiver: &Receiver<ThreadMessage>,
    sender_thread: SenderThread,
) -> Vec<Arc<ParsedFile>> {
    if sender
        .send(ThreadMessage {
            sender: sender_thread,
            payload: MessagePayload::DB(DBRequest {
                operation: DBOperation::Fetch,
                target: DBTarget::Script,
                argument: DBArgument::NotFound,
            }),
        })
        .is_ok()
    {
        if let Ok(response) = receiver.recv() {
            if let MessagePayload::DB(response) = response.payload {
                if let DBArgument::ParsedFiles(fs) = response.argument {
                    return fs.values().map(Arc::clone).collect();
                }
            }
        }
    }
    vec![]
}

pub fn db_get_function(
    sender: &Sender<ThreadMessage>,
    receiver: &Receiver<ThreadMessage>,
    name: String,
    sender_thread: SenderThread,
) -> Option<Arc<FunctionDefinition>> {
    if sender
        .send(ThreadMessage {
            sender: sender_thread,
            payload: MessagePayload::DB(DBRequest {
                operation: DBOperation::Get,
                target: DBTarget::FunctionDefinition,
                argument: DBArgument::String(name.to_string()),
            }),
        })
        .is_ok()
    {
        if let Ok(response) = receiver.recv() {
            if let MessagePayload::DB(response) = response.payload {
                if let DBArgument::FunctionDefinition(func) = response.argument {
                    return Some(func);
                }
            }
        }
    }
    None
}

pub fn db_set_function(
    sender: &Sender<ThreadMessage>,
    function: Arc<FunctionDefinition>,
    sender_thread: SenderThread,
) -> Result<()> {
    sender.send(ThreadMessage {
        sender: sender_thread,
        payload: MessagePayload::DB(DBRequest {
            operation: DBOperation::Set,
            target: DBTarget::FunctionDefinition,
            argument: DBArgument::FunctionDefinition(function),
        }),
    })?;
    Ok(())
}

pub fn db_delete_file_function(
    sender: &Sender<ThreadMessage>,
    path: String,
    sender_thread: SenderThread,
) -> Result<()> {
    sender.send(ThreadMessage {
        sender: sender_thread,
        payload: MessagePayload::DB(DBRequest {
            operation: DBOperation::Delete,
            target: DBTarget::FunctionDefinition,
            argument: DBArgument::String(path),
        }),
    })?;
    Ok(())
}

pub fn db_fetch_functions(
    sender: &Sender<ThreadMessage>,
    receiver: &Receiver<ThreadMessage>,
    sender_thread: SenderThread,
) -> Option<HashMap<String, Arc<FunctionDefinition>>> {
    if sender
        .send(ThreadMessage {
            sender: sender_thread,
            payload: MessagePayload::DB(DBRequest {
                operation: DBOperation::Fetch,
                target: DBTarget::FunctionDefinition,
                argument: DBArgument::NotFound,
            }),
        })
        .is_ok()
    {
        if let Ok(response) = receiver.recv() {
            if let MessagePayload::DB(response) = response.payload {
                if let DBArgument::FunctionDefinitions(fs) = response.argument {
                    return Some(fs);
                }
            }
        }
    }
    None
}

pub fn db_get_package(
    sender: &Sender<ThreadMessage>,
    receiver: &Receiver<ThreadMessage>,
    name: String,
    sender_thread: SenderThread,
) -> Vec<String> {
    if sender
        .send(ThreadMessage {
            sender: sender_thread,
            payload: MessagePayload::DB(DBRequest {
                operation: DBOperation::Get,
                target: DBTarget::Package,
                argument: DBArgument::String(name),
            }),
        })
        .is_ok()
    {
        if let Ok(response) = receiver.recv() {
            if let MessagePayload::DB(response) = response.payload {
                if let DBArgument::Packages(pkg) = response.argument {
                    return pkg;
                }
            }
        }
    }
    vec![]
}

pub fn db_set_packages(
    sender: &Sender<ThreadMessage>,
    packages: Vec<String>,
    sender_thread: SenderThread,
) -> Result<()> {
    sender.send(ThreadMessage {
        sender: sender_thread,
        payload: MessagePayload::DB(DBRequest {
            operation: DBOperation::Set,
            target: DBTarget::Package,
            argument: DBArgument::Packages(packages),
        }),
    })?;
    Ok(())
}

pub fn db_get_request_id(
    sender: &Sender<ThreadMessage>,
    receiver: &Receiver<ThreadMessage>,
    sender_thread: SenderThread,
) -> Option<i32> {
    if sender
        .send(ThreadMessage {
            sender: sender_thread,
            payload: MessagePayload::DB(DBRequest {
                operation: DBOperation::Get,
                target: DBTarget::RequestID,
                argument: DBArgument::NotFound,
            }),
        })
        .is_ok()
    {
        if let Ok(response) = receiver.recv() {
            if let MessagePayload::DB(response) = response.payload {
                if let DBArgument::Integer(id) = response.argument {
                    return Some(id);
                }
            }
        }
    }
    None
}
