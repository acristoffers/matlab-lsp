/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::sync::Arc;

use crate::parsed_file::ParsedFile;
use crate::utils::{lock_mutex, range_to_bytes, SessionStateArc};

use anyhow::{anyhow, Result};
use lsp_server::{ExtractError, Notification};
use lsp_types::notification::{DidChangeTextDocument, DidOpenTextDocument};
use lsp_types::{DidChangeTextDocumentParams, DidOpenTextDocumentParams};

struct Dispatcher<'a> {
    state: SessionStateArc,
    notification: &'a Notification,
    result: Option<Result<bool>>,
}

impl Dispatcher<'_> {
    fn new(state: SessionStateArc, request: &Notification) -> Dispatcher {
        Dispatcher {
            result: None,
            notification: request,
            state,
        }
    }

    fn handle<N>(&mut self, function: fn(SessionStateArc, N::Params) -> Result<bool>) -> &mut Self
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

    fn finish(&mut self) -> Result<bool> {
        let result = self.result.take();
        result.map_or_else(|| Ok(false), |x| x)
    }
}

fn cast<N>(not: Notification) -> Result<N::Params, ExtractError<Notification>>
where
    N: lsp_types::notification::Notification,
    N::Params: serde::de::DeserializeOwned,
{
    not.extract(N::METHOD)
}

pub fn handle_notification(state: SessionStateArc, notification: &Notification) -> Result<bool> {
    let mut dispatcher = Dispatcher::new(Arc::clone(&state), notification);
    dispatcher
        .handle::<DidOpenTextDocument>(handle_text_document_did_open)
        .handle::<DidChangeTextDocument>(handle_text_document_did_change)
        .finish()
}

fn handle_text_document_did_open(
    state: SessionStateArc,
    params: DidOpenTextDocumentParams,
) -> Result<bool> {
    let mut parsed_code = ParsedFile {
        file: params.text_document.uri,
        contents: params.text_document.text,
        tree: None,
    };
    parsed_code.parse()?;
    lock_mutex(&state)?
        .files
        .insert(parsed_code.file.as_str().into(), parsed_code);
    Ok(false)
}

fn handle_text_document_did_change(
    state: SessionStateArc,
    params: DidChangeTextDocumentParams,
) -> Result<bool> {
    let file_name = params.text_document.uri.as_str().to_string();
    let mut state = lock_mutex(&state)?;
    let parsed_file = state
        .files
        .get_mut(&file_name)
        .ok_or(anyhow!("No such file: {file_name}"))?;
    for change in params.content_changes {
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
    Ok(false)
}
