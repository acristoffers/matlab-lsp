/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::process::ExitCode;
use std::sync::Arc;

use crate::utils::{lock_mutex, SessionStateArc};

use anyhow::{anyhow, Context, Result};
use log::info;
use lsp_server::{ExtractError, Message, Request, RequestId, Response};
use lsp_types::request::{Formatting, Shutdown};
use lsp_types::{DocumentFormattingParams, Position, TextEdit};

struct Dispatcher<'a> {
    state: SessionStateArc,
    request: &'a Request,
    result: Option<Result<Option<ExitCode>>>,
}

impl Dispatcher<'_> {
    fn new(state: SessionStateArc, request: &Request) -> Dispatcher {
        Dispatcher {
            result: None,
            request,
            state,
        }
    }

    fn handle<R>(
        &mut self,
        function: fn(SessionStateArc, RequestId, R::Params) -> Result<Option<ExitCode>>,
    ) -> &mut Self
    where
        R: lsp_types::request::Request,
        R::Params: serde::de::DeserializeOwned,
    {
        let state = lock_mutex(&self.state).unwrap();
        if state.client_requested_shutdown {
            self.result = Some(Err(anyhow!("Got a request after a shutdown.")));
            let resp = Response::new_err(
                self.request.id.clone(),
                lsp_server::ErrorCode::InvalidRequest as i32,
                "Shutdown already requested.".to_owned(),
            );
            state.sender.send(Message::Response(resp)).unwrap();
            drop(state);
            return self;
        }
        drop(state);
        let result = match cast::<R>(self.request.clone()) {
            Ok((id, params)) => function(Arc::clone(&self.state), id, params),
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

fn cast<R>(request: Request) -> Result<(RequestId, R::Params), ExtractError<Request>>
where
    R: lsp_types::request::Request,
    R::Params: serde::de::DeserializeOwned,
{
    request.extract(R::METHOD)
}

pub fn handle_request(state: SessionStateArc, request: &Request) -> Result<Option<ExitCode>> {
    let mut dispatcher = Dispatcher::new(state, request);
    dispatcher
        .handle::<Formatting>(handle_formatting)
        .handle::<Shutdown>(handle_shutdown)
        .finish()
}

fn handle_formatting(
    state: SessionStateArc,
    id: RequestId,
    params: DocumentFormattingParams,
) -> Result<Option<ExitCode>> {
    info!("Formatting {}", params.text_document.uri.as_str());
    let mut state = lock_mutex(&state)?;
    let file = state
        .files
        .get_mut(&params.text_document.uri.as_str().to_string())
        .with_context(|| "No file parsed")?;
    let pos = file.tree.as_ref().unwrap().root_node().end_position();
    if let Some(code) = file.format() {
        let result = vec![TextEdit {
            range: lsp_types::Range {
                start: Position::new(0, 0),
                end: Position::new(pos.row.try_into()?, pos.column.try_into()?),
            },
            new_text: code,
        }];
        let result = serde_json::to_value(result).unwrap();
        let resp = Response {
            id,
            result: Some(result),
            error: None,
        };
        state.sender.send(Message::Response(resp))?;
    } else {
        return Err(anyhow!("Error formatting"));
    }
    Ok(None)
}

fn handle_shutdown(state: SessionStateArc, id: RequestId, _params: ()) -> Result<Option<ExitCode>> {
    info!("Received shutdown request.");
    let mut state = lock_mutex(&state)?;
    state.client_requested_shutdown = true;
    let resp = Response::new_ok(id, ());
    let _ = state.sender.send(resp.into());
    Ok(None)
}
