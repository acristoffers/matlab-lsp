/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::collections::HashMap;
use std::process::ExitCode;
use std::sync::Arc;

use crate::analysis::hover::hover_for_symbol;
use crate::analysis::references::find_references_to_symbol;
use crate::analysis::semantic::semantic_tokens;
use crate::code_loc;
use crate::types::{PointToPos, PosToPoint, Range};
use crate::utils::{lock_mutex, SessionStateArc};

use anyhow::{anyhow, Context, Result};
use log::{debug, info};
use lsp_server::{ExtractError, Message, Request, RequestId, Response};
use lsp_types::request::{
    DocumentHighlightRequest, FoldingRangeRequest, Formatting, GotoDefinition, HoverRequest,
    References, Rename, SemanticTokensFullRequest, Shutdown,
};
use lsp_types::{
    DocumentFormattingParams, DocumentHighlight, DocumentHighlightParams, FoldingRange,
    FoldingRangeKind, FoldingRangeParams, GotoDefinitionParams, GotoDefinitionResponse, Hover,
    HoverContents, HoverParams, Location, MarkupKind, Position, ReferenceParams, RenameParams,
    SemanticTokens, SemanticTokensParams, TextEdit, Url, WorkspaceEdit,
};
use regex::Regex;
use tree_sitter::{Point, Query, QueryCursor};

pub fn handle_request(state: SessionStateArc, request: &Request) -> Result<Option<ExitCode>> {
    debug!("Handling a request.");
    let mut dispatcher = Dispatcher::new(state, request);
    dispatcher
        .handle::<Formatting>(handle_formatting)?
        .handle::<GotoDefinition>(handle_goto_definition)?
        .handle::<References>(handle_references)?
        .handle::<Rename>(handle_rename)?
        .handle::<HoverRequest>(handle_hover)?
        .handle::<DocumentHighlightRequest>(handle_highlight)?
        .handle::<FoldingRangeRequest>(handle_folding)?
        .handle::<SemanticTokensFullRequest>(handle_semantic)?
        .handle::<Shutdown>(handle_shutdown)?
        .finish()
}

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
        match cast::<R>(self.request.clone()) {
            Ok((id, params)) => {
                self.result = Some(function(Arc::clone(&self.state), id, params));
            }
            Err(err @ ExtractError::JsonError { .. }) => {
                self.result = Some(Err(anyhow!("JsonError: {err:?}")));
            }
            Err(ExtractError::MethodMismatch(req)) => {
                if self.result.is_none() {
                    self.result = Some(Err(anyhow!("MethodMismatch: {req:?}")));
                }
            }
        };
        Ok(self)
    }

    fn finish(&mut self) -> Result<Option<ExitCode>> {
        let result = self.result.take();
        result.unwrap_or(Ok(None))
    }
}

fn cast<R>(request: Request) -> Result<(RequestId, R::Params), ExtractError<Request>>
where
    R: lsp_types::request::Request,
    R::Params: serde::de::DeserializeOwned,
{
    request.extract(R::METHOD)
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

fn handle_references(
    state: SessionStateArc,
    id: RequestId,
    params: ReferenceParams,
) -> Result<Option<ExitCode>> {
    info!("Received textDocument/references.");
    let include_declaration = params.context.include_declaration;
    let lock = lock_mutex(&state)?;
    let path = params
        .text_document_position
        .text_document
        .uri
        .path()
        .to_string();
    let loc = params.text_document_position.position.to_point();
    if let Ok(rs) = find_references_to_symbol(&lock, path, loc, include_declaration) {
        let rs: Vec<&Location> = rs.iter().map(|(v, _)| v).collect();
        let result = serde_json::to_value(rs)?;
        let resp = Response::new_ok(id, result);
        let _ = lock.sender.send(resp.into());
    } else {
        let resp = Response::new_err(id, 0, "Could not find file.".into());
        let _ = lock.sender.send(resp.into());
    }
    Ok(None)
}

fn handle_rename(
    state: SessionStateArc,
    id: RequestId,
    params: RenameParams,
) -> Result<Option<ExitCode>> {
    info!("Received textDocument/references.");
    let lock = lock_mutex(&state)?;
    let path = params
        .text_document_position
        .text_document
        .uri
        .path()
        .to_string();
    let loc = params.text_document_position.position.to_point();
    let new_name = params.new_name;
    let regex = Regex::new(r"^[a-zA-Z_][a-zA-Z_0-9]*$")?;
    if !regex.is_match(&new_name) {
        let resp = Response::new_err(
            id,
            lsp_server::ErrorCode::InvalidParams as i32,
            "The name is not a valid identifier.".to_owned(),
        );
        lock.sender.send(Message::Response(resp))?;
        return Ok(None);
    }
    let references = find_references_to_symbol(&lock, path, loc, true)?;
    let mut ws_edit: HashMap<Url, Vec<TextEdit>> = HashMap::new();
    for (reference, _) in references {
        let uri = reference.uri;
        let text_edit = TextEdit {
            range: reference.range,
            new_text: new_name.clone(),
        };
        ws_edit
            .entry(uri)
            .and_modify(|v| v.push(text_edit.clone()))
            .or_insert(vec![text_edit]);
    }
    let ws_edit = WorkspaceEdit::new(ws_edit);
    let resp = Response::new_ok(id, ws_edit);
    lock.sender.send(Message::Response(resp))?;
    Ok(None)
}

fn handle_hover(
    state: SessionStateArc,
    id: RequestId,
    params: HoverParams,
) -> Result<Option<ExitCode>> {
    info!("Received textDocument/hover.");
    let lock = lock_mutex(&state)?;
    let path = params
        .text_document_position_params
        .text_document
        .uri
        .path()
        .to_string();
    let loc = params.text_document_position_params.position.to_point();
    if let Some((md, plain)) = hover_for_symbol(&lock, path, loc)? {
        if let Some(td) = &lock.workspace_params.capabilities.text_document {
            if let Some(hover) = &td.hover {
                if let Some(cf) = &hover.content_format {
                    if cf.contains(&MarkupKind::Markdown) {
                        let response = Hover {
                            contents: HoverContents::Markup(md),
                            range: None,
                        };
                        let resp = Response::new_ok(id, response);
                        lock.sender.send(Message::Response(resp))?;
                        return Ok(None);
                    }
                }
            }
        }
        let response = Hover {
            contents: HoverContents::Markup(plain),
            range: None,
        };
        let resp = Response::new_ok(id, response);
        lock.sender.send(Message::Response(resp))?;
    } else {
        let resp = Response::new_ok(id, ());
        lock.sender.send(Message::Response(resp))?;
    }
    Ok(None)
}

fn handle_highlight(
    state: SessionStateArc,
    id: RequestId,
    params: DocumentHighlightParams,
) -> Result<Option<ExitCode>> {
    info!("Received textDocument/highlight.");
    let lock = lock_mutex(&state)?;
    let path = params
        .text_document_position_params
        .text_document
        .uri
        .path()
        .to_string();
    let loc = params.text_document_position_params.position.to_point();
    let locs = find_references_to_symbol(&lock, path.clone(), loc, true)?;
    let mut response = vec![];
    for (location, kind) in locs {
        if location.uri.path() == path {
            let dh = DocumentHighlight {
                range: location.range,
                kind: Some(kind),
            };
            response.push(dh);
        }
    }
    let resp = Response::new_ok(id, response);
    lock.sender.send(Message::Response(resp))?;
    Ok(None)
}

fn handle_folding(
    state: SessionStateArc,
    id: RequestId,
    params: FoldingRangeParams,
) -> Result<Option<ExitCode>> {
    info!("Received textDocument/foldingRange.");
    let lock = lock_mutex(&state)?;
    let path = params.text_document.uri.path().to_string();
    if let Some(file) = lock.files.get(&path) {
        let file_lock = lock_mutex(file)?;
        if let Some(tree) = &file_lock.tree {
            let root = tree.root_node();
            let scm = "(block) @block";
            let query = Query::new(tree_sitter_matlab::language(), scm)?;
            let mut cursor = QueryCursor::new();
            let mut resp = vec![];
            for node in cursor
                .captures(&query, root, file_lock.contents.as_bytes())
                .map(|(c, _)| c)
                .flat_map(|c| c.captures)
                .map(|c| c.node)
            {
                let fold = FoldingRange {
                    start_line: node.start_position().to_position().line,
                    start_character: None,
                    end_line: node.end_position().to_position().line,
                    end_character: None,
                    kind: Some(FoldingRangeKind::Region),
                    collapsed_text: None,
                };
                resp.push(fold);
            }
            let resp = Response::new_ok(id, resp);
            lock.sender.send(Message::Response(resp))?;
            return Ok(None);
        }
    }
    let resp = Response::new_err(
        id,
        lsp_server::ErrorCode::InvalidParams as i32,
        "File was not yet parsed.".to_owned(),
    );
    lock.sender.send(Message::Response(resp))?;
    Ok(None)
}

fn handle_semantic(
    state: SessionStateArc,
    id: RequestId,
    params: SemanticTokensParams,
) -> Result<Option<ExitCode>> {
    info!("Received textDocument/semanticTokens/full.");
    let lock = lock_mutex(&state)?;
    let path = params.text_document.uri.path().to_string();
    if let Some(file) = lock.files.get(&path) {
        let parsed_file = lock_mutex(file)?;
        let response = semantic_tokens(&parsed_file)?;
        let sts = SemanticTokens {
            result_id: None,
            data: response,
        };
        let resp = Response::new_ok(id, sts);
        lock.sender.send(Message::Response(resp))?;
    } else {
        let resp = Response::new_err(
            id,
            lsp_server::ErrorCode::InvalidParams as i32,
            "File not found.".to_owned(),
        );
        lock.sender.send(Message::Response(resp))?;
    }
    Ok(None)
}

fn handle_shutdown(state: SessionStateArc, id: RequestId, _params: ()) -> Result<Option<ExitCode>> {
    info!("Received shutdown request.");
    let mut state = lock_mutex(&state)?;
    state.client_requested_shutdown = true;
    state.rescan_all_files = false;
    state.rescan_open_files = false;
    let resp = Response::new_ok(id, ());
    let _ = state.sender.send(resp.into());
    Ok(None)
}
