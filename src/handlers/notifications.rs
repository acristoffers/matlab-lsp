/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::sync::Arc;

use crate::parsed_file::ParsedFile;
use crate::utils::{lock_mutex, SessionStateArc};

use anyhow::{anyhow, Result};
use lsp_server::{ExtractError, Notification};
use lsp_types::notification::DidOpenTextDocument;
use lsp_types::DidOpenTextDocumentParams;

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
        .handle::<DidOpenTextDocument>(handle_did_open)
        .finish()
}

fn handle_did_open(state: SessionStateArc, params: DidOpenTextDocumentParams) -> Result<bool> {
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
