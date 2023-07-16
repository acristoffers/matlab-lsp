/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::path::Path;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use crate::analysis::defref;
use crate::parsed_file::{FileType, ParsedFile};
use crate::types::{Range, Workspace};
use crate::utils::{lock_mutex, read_to_string, rescan_file, SessionStateArc};

use anyhow::{anyhow, Result};
use itertools::Itertools;
use log::{debug, info};
use lsp_server::{ExtractError, Message, Notification, RequestId};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, DidSaveTextDocument, Exit,
};
use lsp_types::request::{Request, SemanticTokensRefresh};
use lsp_types::{
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DidSaveTextDocumentParams,
};

pub fn handle_notification(
    state: SessionStateArc,
    notification: &Notification,
) -> Result<Option<ExitCode>> {
    let mut dispatcher = Dispatcher::new(Arc::clone(&state), notification);
    dispatcher
        .handle::<DidOpenTextDocument>(handle_text_document_did_open)
        .handle::<DidCloseTextDocument>(handle_text_document_did_close)
        .handle::<DidChangeTextDocument>(handle_text_document_did_change)
        .handle::<DidSaveTextDocument>(handle_text_document_did_save)
        .handle::<Exit>(handle_exit)
        .finish()
}

struct Dispatcher<'a> {
    state: SessionStateArc,
    notification: &'a Notification,
    result: Option<Result<Option<ExitCode>>>,
}

impl Dispatcher<'_> {
    fn new(state: SessionStateArc, request: &Notification) -> Dispatcher {
        Dispatcher {
            result: None,
            notification: request,
            state,
        }
    }

    fn handle<N>(
        &mut self,
        function: fn(SessionStateArc, N::Params) -> Result<Option<ExitCode>>,
    ) -> &mut Self
    where
        N: lsp_types::notification::Notification,
        N::Params: serde::de::DeserializeOwned,
    {
        let result = match cast::<N>(self.notification.clone()) {
            Ok(params) => function(Arc::clone(&self.state), params),
            Err(err @ ExtractError::JsonError { .. }) => Err(anyhow!("JsonError: {err:?}")),
            Err(ExtractError::MethodMismatch(req)) => Err(anyhow!("MethodMismatch: {req:?}")),
        };
        if result.is_ok() || self.result.is_none() {
            self.result = Some(result);
        }
        self
    }

    fn finish(&mut self) -> Result<Option<ExitCode>> {
        let result = self.result.take();
        result.map_or_else(|| Ok(None), |x| x)
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
    state: SessionStateArc,
    params: DidOpenTextDocumentParams,
) -> Result<Option<ExitCode>> {
    info!(
        "documentText/didOpen: {}",
        params.text_document.uri.as_str()
    );
    let mut lock = lock_mutex(&state)?;
    let contents = read_to_string(&mut params.text_document.text.as_bytes(), None)?.0;
    let path = params.text_document.uri.path().to_string();
    if let Some(file) = lock.files.get(&path) {
        let mut lock_file = lock_mutex(file)?;
        lock_file.open = true;
        lock_file.contents = contents;
        drop(lock_file);
        let file = Arc::clone(file);
        rescan_file(&mut lock, file)?;
        return Ok(None);
    }
    let path_p = Path::new(&path);
    let mut scope = String::new();
    for segment in path_p.iter() {
        let segment = segment.to_string_lossy().to_string();
        if segment.starts_with('+') || segment.starts_with('@') {
            if !scope.is_empty() {
                scope += "/";
            }
            scope += segment.as_str();
        }
    }
    let file_name: String = if let Some(segs) = params.text_document.uri.path_segments() {
        if let Some(name) = segs
            .filter(|c| !c.is_empty())
            .flat_map(|c| c.strip_suffix(".m"))
            .last()
        {
            name.into()
        } else {
            "".into()
        }
    } else {
        "".into()
    };
    let mut parsed_file = ParsedFile {
        contents,
        path: params.text_document.uri.path().to_string(),
        name: file_name.clone(),
        file_type: FileType::MScript,
        in_classfolder: None,
        in_namespace: None,
        open: true,
        tree: None,
        workspace: Workspace::default(),
    };
    let key = parsed_file.path.clone();
    parsed_file.parse()?;
    let parsed_file = Arc::new(Mutex::new(parsed_file));
    let namespace = if let Some(segments) = params.text_document.uri.path_segments() {
        segments
            .map(|s| s.to_string())
            .flat_map(|s| s.strip_prefix(|f| f == '+' || f == '@').map(String::from))
            .join(".")
    } else {
        "".to_string()
    };
    defref::analyze(&lock, Arc::clone(&parsed_file))?;
    let mut file_lock = lock_mutex(&parsed_file)?;
    lock.files.insert(key.clone(), Arc::clone(&parsed_file));
    ParsedFile::define_type(
        &mut lock,
        Arc::clone(&parsed_file),
        &mut file_lock,
        namespace,
    )?;
    debug!("Inserted {key} into the store");
    lock.sender.send(Message::Request(lsp_server::Request {
        id: RequestId::from(lock.request_id),
        method: SemanticTokensRefresh::METHOD.to_string(),
        params: serde_json::to_value(())?,
    }))?;
    lock.request_id += 1;
    Ok(None)
}

fn handle_text_document_did_close(
    state: SessionStateArc,
    params: DidCloseTextDocumentParams,
) -> Result<Option<ExitCode>> {
    info!(
        "documentText/didClose: {}",
        params.text_document.uri.as_str()
    );
    let path = params.text_document.uri.path();
    if params.text_document.uri.scheme() == "file" && std::path::Path::new(path).exists() {
        let mut state_lock = lock_mutex(&state)?;
        if let Some(file) = state_lock.files.get(&path.to_string()) {
            let mut lock_file = lock_mutex(file)?;
            lock_file.open = false;
            drop(lock_file);
            let file = Arc::clone(file);
            rescan_file(&mut state_lock, file)?;
            state_lock.rescan_all_files = true;
        }
    } else {
        lock_mutex(&state)?
            .files
            .remove(params.text_document.uri.as_str());
    }
    Ok(None)
}

fn handle_text_document_did_change(
    state: SessionStateArc,
    params: DidChangeTextDocumentParams,
) -> Result<Option<ExitCode>> {
    info!(
        "documentText/didChange: {}",
        params.text_document.uri.as_str(),
    );
    let file_path = params.text_document.uri.path().to_string();
    let parsed_file = lock_mutex(&state)?
        .files
        .get(&file_path)
        .ok_or(anyhow!("No such file: {file_path}"))?
        .clone();
    for change in params.content_changes {
        debug!(
            "Appying change with range {} and contents {}",
            serde_json::to_string(&change.range)?,
            change.text
        );
        match change.range {
            Some(range) => {
                let range: Range = range.into();
                let mut parsed_file_lock = lock_mutex(&parsed_file)?;
                let ts_range = range.find_bytes(&parsed_file_lock);
                let (start, mut end) = (ts_range.start_byte, ts_range.end_byte);
                end = end.min(parsed_file_lock.contents.len().saturating_sub(1));
                if start >= end {
                    parsed_file_lock
                        .contents
                        .insert_str(start, change.text.as_str());
                } else {
                    debug!("Replacing from {start} to {end} with {}", change.text);
                    parsed_file_lock
                        .contents
                        .replace_range(start..end, change.text.as_str());
                }
            }
            None => lock_mutex(&parsed_file)?.contents = change.text,
        }
    }
    let mut lock = lock_mutex(&state)?;
    rescan_file(&mut lock, parsed_file)?;
    lock.sender.send(Message::Request(lsp_server::Request {
        id: RequestId::from(lock.request_id),
        method: SemanticTokensRefresh::METHOD.to_string(),
        params: serde_json::to_value(())?,
    }))?;
    lock.request_id += 1;
    lock.rescan_open_files = true;
    Ok(None)
}

fn handle_text_document_did_save(
    state: SessionStateArc,
    params: DidSaveTextDocumentParams,
) -> Result<Option<ExitCode>> {
    info!(
        "documentText/didSave: {}",
        params.text_document.uri.as_str(),
    );
    let file_path = params.text_document.uri.path().to_string();
    let mut lock = lock_mutex(&state)?;
    let parsed_file = lock
        .files
        .get(&file_path)
        .ok_or(anyhow!("No such file: {file_path}"))?
        .clone();
    if let Some(content) = params.text {
        let mut parsed_file_lock = lock_mutex(&parsed_file)?;
        parsed_file_lock.contents = content;
        drop(parsed_file_lock);
        rescan_file(&mut lock, parsed_file)?;
    }
    lock.sender.send(Message::Request(lsp_server::Request {
        id: RequestId::from(lock.request_id),
        method: SemanticTokensRefresh::METHOD.to_string(),
        params: serde_json::to_value(())?,
    }))?;
    lock.request_id += 1;
    lock.rescan_all_files = true;
    Ok(None)
}

fn handle_exit(state: SessionStateArc, _params: ()) -> Result<Option<ExitCode>> {
    info!("Got Exit notification.");
    if let Ok(state) = lock_mutex(&state) {
        if !state.client_requested_shutdown {
            return Ok(Some(ExitCode::from(1)));
        }
    }
    Ok(Some(ExitCode::SUCCESS))
}
