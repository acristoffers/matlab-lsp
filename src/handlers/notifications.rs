/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::sync::Arc;

use crate::extractors::symbols::extract_symbols;
use crate::threads::db::{
    db_delete_file_function, db_delete_parsed_file, db_get_parsed_file, db_set_parsed_file,
};
use crate::types::{MessagePayload, ParsedFile, Range, SenderThread, ThreadMessage};
use crate::utils::{read_to_string, request_semantic_tokens_refresh};

use anyhow::{anyhow, Result};
use crossbeam_channel::{Receiver, Sender};
use lsp_server::{ExtractError, Message, Notification};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, DidSaveTextDocument,
};
use lsp_types::{
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DidSaveTextDocumentParams,
};

pub fn handle_notification(
    lsp_sender: Sender<Message>,
    sender: Sender<ThreadMessage>,
    receiver: Receiver<ThreadMessage>,
    notification: Notification,
) -> Result<()> {
    let mut dispatcher = Dispatcher::new(lsp_sender, sender, receiver, notification);
    dispatcher
        .handle::<DidOpenTextDocument>(handle_text_document_did_open)
        .handle::<DidCloseTextDocument>(handle_text_document_did_close)
        .handle::<DidChangeTextDocument>(handle_text_document_did_change)
        .handle::<DidSaveTextDocument>(handle_text_document_did_save)
        .finish()?;
    Ok(())
}

struct Dispatcher {
    lsp_sender: Sender<Message>,
    sender: Sender<ThreadMessage>,
    receiver: Receiver<ThreadMessage>,
    notification: Notification,
    result: Option<Result<()>>,
}

type Callback<P> =
    fn(Sender<Message>, Sender<ThreadMessage>, Receiver<ThreadMessage>, P) -> Result<()>;

impl Dispatcher {
    fn new(
        lsp_sender: Sender<Message>,
        sender: Sender<ThreadMessage>,
        receiver: Receiver<ThreadMessage>,
        notification: Notification,
    ) -> Dispatcher {
        Dispatcher {
            lsp_sender,
            sender,
            receiver,
            notification,
            result: None,
        }
    }

    fn handle<N>(&mut self, function: Callback<N::Params>) -> &mut Self
    where
        N: lsp_types::notification::Notification,
        N::Params: serde::de::DeserializeOwned,
    {
        let result = match cast::<N>(self.notification.clone()) {
            Ok(params) => function(
                self.lsp_sender.clone(),
                self.sender.clone(),
                self.receiver.clone(),
                params,
            ),
            Err(err @ ExtractError::JsonError { .. }) => Err(anyhow!("JsonError: {err:?}")),
            Err(ExtractError::MethodMismatch(req)) => Err(anyhow!("MethodMismatch: {req:?}")),
        };
        if result.is_ok() || self.result.is_none() {
            self.result = Some(result);
        }
        self
    }

    fn finish(&mut self) -> Result<()> {
        self.result.take().unwrap_or(Ok(()))
    }
}

fn cast<N>(notification: Notification) -> Result<N::Params, ExtractError<Notification>>
where
    N: lsp_types::notification::Notification,
    N::Params: serde::de::DeserializeOwned,
{
    notification.extract(N::METHOD)
}

fn handle_text_document_did_open(
    lsp_sender: Sender<Message>,
    sender: Sender<ThreadMessage>,
    receiver: Receiver<ThreadMessage>,
    params: DidOpenTextDocumentParams,
) -> Result<()> {
    let path = params.text_document.uri.path().to_string();
    let contents = read_to_string(&mut params.text_document.text.as_bytes(), None)?.0;
    let mut file = ParsedFile::new(path.clone(), Some(contents))?;
    file.open = true;
    let file = extract_symbols(
        sender.clone(),
        receiver.clone(),
        SenderThread::Handler,
        Arc::new(file),
    )?;
    db_set_parsed_file(&sender, file, SenderThread::Handler)?;
    request_semantic_tokens_refresh(&lsp_sender, &sender, &receiver, SenderThread::Handler)?;
    Ok(())
}

fn handle_text_document_did_close(
    _lsp_sender: Sender<Message>,
    sender: Sender<ThreadMessage>,
    receiver: Receiver<ThreadMessage>,
    params: DidCloseTextDocumentParams,
) -> Result<()> {
    let path = params.text_document.uri.path().to_string();
    if let Ok(file) = ParsedFile::new(path.clone(), None) {
        let file = extract_symbols(
            sender.clone(),
            receiver.clone(),
            SenderThread::Handler,
            Arc::new(file),
        )?;
        let mut file = file.as_ref().clone();
        file.open = false;
        file.dump_contents();
        db_set_parsed_file(&sender, Arc::new(file), SenderThread::Handler)?;
    } else {
        db_delete_parsed_file(&sender, path.clone(), SenderThread::Handler)?;
        db_delete_file_function(&sender, path, SenderThread::Handler)?;
    }
    sender.send(ThreadMessage {
        sender: SenderThread::Handler,
        payload: MessagePayload::ScanWorkspace(vec![]),
    })?;
    Ok(())
}

fn handle_text_document_did_change(
    lsp_sender: Sender<Message>,
    sender: Sender<ThreadMessage>,
    receiver: Receiver<ThreadMessage>,
    params: DidChangeTextDocumentParams,
) -> Result<()> {
    let path = params.text_document.uri.path().to_string();
    let mut file =
        if let Some(file) = db_get_parsed_file(&sender, &receiver, path, SenderThread::Handler) {
            file.as_ref().clone()
        } else {
            return Ok(());
        };
    for change in params.content_changes {
        match change.range {
            Some(range) => {
                let range: Range = range.into();
                let ts_range = range.find_bytes(&file);
                let (start, mut end) = (ts_range.start_byte, ts_range.end_byte);
                end = end.min(file.contents.len().saturating_sub(1));
                if start >= end {
                    file.contents.insert_str(start, change.text.as_str());
                } else {
                    file.contents
                        .replace_range(start..end, change.text.as_str());
                }
            }
            None => file.contents = change.text,
        }
    }
    file.tree = ParsedFile::ts_parse(&file.contents)?;
    let file = extract_symbols(
        sender.clone(),
        receiver.clone(),
        SenderThread::Handler,
        Arc::new(file),
    )?;
    db_set_parsed_file(&sender, file, SenderThread::Handler)?;
    sender.send(ThreadMessage {
        sender: SenderThread::Handler,
        payload: crate::types::MessagePayload::ScanOpen,
    })?;
    request_semantic_tokens_refresh(&lsp_sender, &sender, &receiver, SenderThread::Handler)?;
    Ok(())
}

fn handle_text_document_did_save(
    lsp_sender: Sender<Message>,
    sender: Sender<ThreadMessage>,
    receiver: Receiver<ThreadMessage>,
    params: DidSaveTextDocumentParams,
) -> Result<()> {
    let path = params.text_document.uri.path().to_string();
    let mut file =
        if let Some(file) = db_get_parsed_file(&sender, &receiver, path, SenderThread::Handler) {
            file.as_ref().clone()
        } else {
            return Ok(());
        };
    if let Some(content) = params.text {
        file.contents = content;
    }
    file.tree = ParsedFile::ts_parse(&file.contents)?;
    let file = extract_symbols(
        sender.clone(),
        receiver.clone(),
        SenderThread::Handler,
        Arc::new(file),
    )?;
    db_set_parsed_file(&sender, file, SenderThread::Handler)?;
    sender.send(ThreadMessage {
        sender: SenderThread::Handler,
        payload: MessagePayload::ScanWorkspace(vec![]),
    })?;
    request_semantic_tokens_refresh(&lsp_sender, &sender, &receiver, SenderThread::Handler)?;
    Ok(())
}
