/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::process::ExitCode;
use std::sync::Arc;

use crate::code_loc;
use crate::types::Range;
use crate::utils::{lock_mutex, SessionStateArc};

use anyhow::{anyhow, Context, Result};
use log::{debug, info};
use lsp_server::{ExtractError, Message, Request, RequestId, Response};
use lsp_types::request::{Formatting, GotoDefinition, Shutdown};
use lsp_types::{
    DocumentFormattingParams, GotoDefinitionParams, GotoDefinitionResponse, Location, Position,
    TextEdit, Url,
};
use tree_sitter::Point;

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
    ) -> Result<&mut Self>
    where
        R: lsp_types::request::Request,
        R::Params: serde::de::DeserializeOwned,
    {
        let state = lock_mutex(&self.state)?;
        if state.client_requested_shutdown {
            self.result = Some(Err(anyhow!("Got a request after a shutdown.")));
            let resp = Response::new_err(
                self.request.id.clone(),
                lsp_server::ErrorCode::InvalidRequest as i32,
                "Shutdown already requested.".to_owned(),
            );
            state.sender.send(Message::Response(resp))?;
            drop(state);
            return Ok(self);
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
        Ok(self)
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
    debug!("Handling a request.");
    let mut dispatcher = Dispatcher::new(state, request);
    dispatcher
        .handle::<Formatting>(handle_formatting)?
        .handle::<GotoDefinition>(handle_goto_definition)?
        .handle::<Shutdown>(handle_shutdown)?
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
        .get_mut(&params.text_document.uri.path().to_string())
        .with_context(|| "No file parsed")?;
    let mut file = lock_mutex(file)?;
    let pos = file
        .tree
        .as_ref()
        .ok_or(code_loc!())?
        .root_node()
        .end_position();
    if let Some(code) = file.format() {
        let result = vec![TextEdit {
            range: lsp_types::Range {
                start: Position::new(0, 0),
                end: Position::new(pos.row.try_into()?, pos.column.try_into()?),
            },
            new_text: code,
        }];
        let result = serde_json::to_value(result)?;
        let resp = Response {
            id,
            result: Some(result),
            error: None,
        };
        drop(file);
        state.sender.send(Message::Response(resp))?;
    } else {
        return Err(anyhow!("Error formatting"));
    }
    Ok(None)
}

fn handle_goto_definition(
    state: SessionStateArc,
    id: RequestId,
    params: GotoDefinitionParams,
) -> Result<Option<ExitCode>> {
    let state = lock_mutex(&state)?;
    let uri = params.text_document_position_params.text_document.uri;
    let file = uri.path();
    let loc = params.text_document_position_params.position;
    let loc = Point {
        row: loc.line.try_into()?,
        column: loc.character.try_into()?,
    };
    if let Some(file) = state.files.get(file) {
        let file = lock_mutex(file)?;
        debug!("Goto Definition for file {}", file.path);
        debug!(
            "File contains {} references",
            file.workspace.references.len()
        );
        let refs = file.workspace.references.clone();
        drop(file);
        for refs in &refs {
            let r = lock_mutex(refs)?;
            if r.loc.contains(loc) {
                debug!("Point in range, matching.");
                let resp = match &r.target {
                    crate::types::ReferenceTarget::Class(cls) => {
                        let path = lock_mutex(&lock_mutex(cls)?.parsed_file)?.path.clone();
                        let path = String::from("file://") + path.as_str();
                        Some(GotoDefinitionResponse::from(Location::new(
                            Url::parse(path.as_str())?,
                            Range::default().into(),
                        )))
                    }
                    crate::types::ReferenceTarget::Function(fun) => {
                        let path = lock_mutex(&lock_mutex(fun)?.parsed_file)?.path.clone();
                        let path = String::from("file://") + path.as_str();
                        Some(GotoDefinitionResponse::from(Location::new(
                            Url::parse(path.as_str())?,
                            lock_mutex(fun)?.loc.into(),
                        )))
                    }
                    crate::types::ReferenceTarget::Variable(var) => {
                        Some(GotoDefinitionResponse::from(Location::new(
                            uri,
                            lock_mutex(var)?.loc.into(),
                        )))
                    }
                    crate::types::ReferenceTarget::Script(scr) => {
                        let path = lock_mutex(scr)?.path.clone();
                        let path = String::from("file://") + path.as_str();
                        Some(GotoDefinitionResponse::from(Location::new(
                            Url::parse(path.as_str())?,
                            Range::default().into(),
                        )))
                    }
                    crate::types::ReferenceTarget::Namespace(_) => None,
                    crate::types::ReferenceTarget::ClassFolder(_) => None,
                    crate::types::ReferenceTarget::UnknownVariable => None,
                    crate::types::ReferenceTarget::UnknownFunction => None,
                };
                if let Some(resp) = resp {
                    let resp = Response::new_ok(id, resp);
                    state.sender.send(Message::Response(resp))?;
                } else {
                    debug!("Matched, got None");
                    let resp = Response::new_ok(id, ());
                    let _ = state.sender.send(resp.into());
                }
                return Ok(None);
            }
        }
        debug!("Point not in range.");
        let resp = Response::new_ok(id, ());
        let _ = state.sender.send(resp.into());
    } else {
        let resp = Response::new_err(id, 0, "Could not find file.".into());
        let _ = state.sender.send(resp.into());
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
