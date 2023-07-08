/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::process::ExitCode;
use std::sync::Arc;

use crate::parsed_file::ParsedFile;
use crate::utils::{lock_mutex, range_to_bytes, SessionStateArc};

use anyhow::{anyhow, Result};
use log::{debug, info};
use lsp_server::{ExtractError, Notification};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, Exit,
};
use lsp_types::{
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
};

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
            Ok(params) => Some(function(Arc::clone(&self.state), params)),
            Err(err @ ExtractError::JsonError { .. }) => Some(Err(anyhow!("JsonError: {err:?}"))),
            Err(ExtractError::MethodMismatch(req)) => Some(Err(anyhow!("MethodMismatch: {req:?}"))),
        };
        self.result = result;
        self
    }

    fn finish(&mut self) -> Result<Option<ExitCode>> {
        let result = self.result.take();
        result.map_or_else(|| Ok(None), |x| x)
    }
}

fn cast<N>(not: Notification) -> Result<N::Params, ExtractError<Notification>>
where
    N: lsp_types::notification::Notification,
    N::Params: serde::de::DeserializeOwned,
{
    not.extract(N::METHOD)
}

pub fn handle_notification(
    state: SessionStateArc,
    notification: &Notification,
) -> Result<Option<ExitCode>> {
    let mut dispatcher = Dispatcher::new(Arc::clone(&state), notification);
    dispatcher
        .handle::<DidOpenTextDocument>(handle_text_document_did_open)
        .handle::<DidCloseTextDocument>(handle_text_document_did_close)
        .handle::<DidChangeTextDocument>(handle_text_document_did_change)
        .handle::<Exit>(handle_exit)
        .finish()
}

fn handle_text_document_did_open(
    state: SessionStateArc,
    params: DidOpenTextDocumentParams,
) -> Result<Option<ExitCode>> {
    info!(
        "documentText/didOpen: {}",
        params.text_document.uri.as_str()
    );
    let mut parsed_code = ParsedFile {
        file: params.text_document.uri,
        contents: params.text_document.text,
        tree: None,
        open: true,
    };
    parsed_code.parse()?;
    lock_mutex(&state)?
        .files
        .insert(parsed_code.file.as_str().into(), parsed_code);
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
    if params.text_document.uri.scheme() == "file" {
        let path = params.text_document.uri.path();
        let parsed_file = ParsedFile::parse_file(path.into())?;
        let mut state = lock_mutex(&state)?;
        if let Some(file) = state.files.get_mut(path) {
            *file = parsed_file;
        } else {
            state
                .files
                .insert(parsed_file.file.as_str().into(), parsed_file);
        }
    } else {
        let mut state = lock_mutex(&state)?;
        state.files.remove(params.text_document.uri.as_str());
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
    let file_name = params.text_document.uri.as_str().to_string();
    let mut state = lock_mutex(&state)?;
    let parsed_file = state
        .files
        .get_mut(&file_name)
        .ok_or(anyhow!("No such file: {file_name}"))?;
    for change in params.content_changes {
        debug!(
            "Appying change with range {} and contents {}",
            serde_json::to_string(&change.range)?,
            change.text
        );
        match change.range {
            Some(range) => {
                let (start, mut end) = range_to_bytes(range, parsed_file)?;
                end = end.min(parsed_file.contents.len() - 1);
                if start == end {
                    parsed_file.contents.insert_str(start, change.text.as_str());
                } else {
                    eprintln!("Replacing from {start} to {end} with {}", change.text);
                    parsed_file
                        .contents
                        .replace_range(start..end, change.text.as_str());
                }
                parsed_file.parse()?;
            }
            None => parsed_file.contents = change.text,
        }
    }
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
