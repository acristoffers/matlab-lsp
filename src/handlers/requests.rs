/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::sync::Arc;

use crate::utils::{lock_mutex, SessionStateArc};

use anyhow::{anyhow, Context, Result};
use lsp_server::{ExtractError, Message, Request, RequestId, Response};
use lsp_types::request::{Formatting, Shutdown};
use lsp_types::{DocumentFormattingParams, Position, TextEdit};

struct Dispatcher<'a> {
    state: SessionStateArc,
    request: &'a Request,
    result: Option<Result<bool>>,
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
        function: fn(SessionStateArc, RequestId, R::Params) -> Result<bool>,
    ) -> &mut Self
    where
        R: lsp_types::request::Request,
        R::Params: serde::de::DeserializeOwned,
    {
        let result = match cast::<R>(self.request.clone()) {
            Ok((id, params)) => Some(function(Arc::clone(&self.state), id, params)),
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

fn cast<R>(req: Request) -> Result<(RequestId, R::Params), ExtractError<Request>>
where
    R: lsp_types::request::Request,
    R::Params: serde::de::DeserializeOwned,
{
    req.extract(R::METHOD)
}

pub fn handle_request(state: SessionStateArc, request: &Request) -> Result<bool> {
    let mut dispatcher = Dispatcher::new(state, request);
    dispatcher
        .handle::<Shutdown>(handle_shutdown)
        .handle::<Formatting>(handle_formatting)
        .finish()
}

fn handle_formatting(
    state: SessionStateArc,
    id: RequestId,
    params: DocumentFormattingParams,
) -> Result<bool> {
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
    Ok(false)
}

fn handle_shutdown(state: SessionStateArc, id: RequestId, _params: ()) -> Result<bool> {
    let state = lock_mutex(&state)?;
    let resp = Response::new_ok(id.clone(), ());
    let _ = state.sender.send(resp.into());
    Ok(true)
}
