/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::collections::HashMap;
use std::process::ExitCode;
use std::sync::MutexGuard;

use crate::analysis::completion;
use crate::analysis::hover::hover_for_symbol;
use crate::analysis::references::find_references_to_symbol;
use crate::analysis::semantic::semantic_tokens;
use crate::code_loc;
use crate::session_state::SessionState;
use crate::types::{PointToPos, PosToPoint, Range};
use crate::utils::{lock_mutex, SessionStateArc};

use anyhow::{anyhow, Context, Result};
use log::{debug, info};
use lsp_server::{ExtractError, Message, Request, RequestId, Response};
use lsp_types::request::{
    Completion, DocumentHighlightRequest, FoldingRangeRequest, Formatting, GotoDefinition,
    HoverRequest, References, Rename, SemanticTokensFullRequest, Shutdown,
};
use lsp_types::{
    CompletionParams, DocumentFormattingParams, DocumentHighlight, DocumentHighlightParams,
    FoldingRange, FoldingRangeKind, FoldingRangeParams, GotoDefinitionParams,
    GotoDefinitionResponse, Hover, HoverContents, HoverParams, Location, MarkupKind, Position,
    ReferenceParams, RenameParams, SemanticTokens, SemanticTokensParams, TextEdit, Url,
    WorkspaceEdit,
};
use regex::Regex;
use tree_sitter::{Point, Query, QueryCursor};

pub fn handle_request(state: SessionStateArc, request: &Request) -> Result<Option<ExitCode>> {
    debug!("Handling a request.");
    let mut lock = lock_mutex(&state)?;
    if lock.client_requested_shutdown {
        let resp = Response::new_err(
            request.id.clone(),
            lsp_server::ErrorCode::InvalidRequest as i32,
            "Shutdown already requested.".to_owned(),
        );
        lock.sender.send(Message::Response(resp))?;
        return Err(anyhow!("Got a request after a shutdown."));
    }
    let mut dispatcher = Dispatcher::new(request);
    dispatcher
        .handle::<Formatting>(&mut lock, handle_formatting)?
        .handle::<GotoDefinition>(&mut lock, handle_goto_definition)?
        .handle::<References>(&mut lock, handle_references)?
        .handle::<Rename>(&mut lock, handle_rename)?
        .handle::<HoverRequest>(&mut lock, handle_hover)?
        .handle::<DocumentHighlightRequest>(&mut lock, handle_highlight)?
        .handle::<FoldingRangeRequest>(&mut lock, handle_folding)?
        .handle::<SemanticTokensFullRequest>(&mut lock, handle_semantic)?
        .handle::<Completion>(&mut lock, handle_completion)?
        .handle::<Shutdown>(&mut lock, handle_shutdown)?
        .finish()
}

struct Dispatcher<'a> {
    request: &'a Request,
    result: Option<Result<Option<ExitCode>>>,
}

type Callback<P> =
    fn(&mut MutexGuard<'_, &mut SessionState>, RequestId, P) -> Result<Option<ExitCode>>;

impl Dispatcher<'_> {
    fn new(request: &Request) -> Dispatcher {
        Dispatcher {
            request,
            result: None,
        }
    }

    fn handle<R>(
        &mut self,
        state: &mut MutexGuard<'_, &mut SessionState>,
        function: Callback<R::Params>,
    ) -> Result<&mut Self>
    where
        R: lsp_types::request::Request,
        R::Params: serde::de::DeserializeOwned,
    {
        match cast::<R>(self.request.clone()) {
            Ok((id, params)) => {
                self.result = Some(function(state, id, params));
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
    state: &mut MutexGuard<'_, &mut SessionState>,
    id: RequestId,
    params: DocumentFormattingParams,
) -> Result<Option<ExitCode>> {
    info!("Formatting {}", params.text_document.uri.as_str());
    let file = state
        .files
        .get_mut(&params.text_document.uri.path().to_string())
        .with_context(|| "No file parsed")?;
    let mut file = file.borrow_mut();
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
    state: &mut MutexGuard<'_, &mut SessionState>,
    id: RequestId,
    params: GotoDefinitionParams,
) -> Result<Option<ExitCode>> {
    let uri = params.text_document_position_params.text_document.uri;
    let file = uri.path();
    let loc = params.text_document_position_params.position;
    let loc = Point {
        row: loc.line.try_into()?,
        column: loc.character.try_into()?,
    };
    if let Some(file) = state.files.get(file) {
        let file = file.borrow_mut();
        debug!("Goto Definition for file {}", file.path);
        debug!(
            "File contains {} references",
            file.workspace.references.len()
        );
        let refs = file.workspace.references.clone();
        drop(file);
        for refs in &refs {
            let r = refs.borrow();
            if r.loc.contains(loc) {
                debug!("Point in range, matching.");
                let resp = match &r.target {
                    crate::types::ReferenceTarget::Class(cls) => {
                        let path = cls.borrow().parsed_file.borrow().path.clone();
                        let path = String::from("file://") + path.as_str();
                        Some(GotoDefinitionResponse::from(Location::new(
                            Url::parse(path.as_str())?,
                            Range::default().into(),
                        )))
                    }
                    crate::types::ReferenceTarget::Function(fun) => {
                        let path = fun.borrow().parsed_file.borrow().path.clone();
                        let path = String::from("file://") + path.as_str();
                        Some(GotoDefinitionResponse::from(Location::new(
                            Url::parse(path.as_str())?,
                            fun.borrow_mut().loc.into(),
                        )))
                    }
                    crate::types::ReferenceTarget::Variable(var) => {
                        Some(GotoDefinitionResponse::from(Location::new(
                            uri,
                            var.borrow_mut().loc.into(),
                        )))
                    }
                    crate::types::ReferenceTarget::Script(scr) => {
                        let path = scr.borrow().path.clone();
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
    state: &mut MutexGuard<'_, &mut SessionState>,
    id: RequestId,
    params: ReferenceParams,
) -> Result<Option<ExitCode>> {
    info!("Received textDocument/references.");
    let include_declaration = params.context.include_declaration;
    let path = params
        .text_document_position
        .text_document
        .uri
        .path()
        .to_string();
    let loc = params.text_document_position.position.to_point();
    if let Ok(rs) = find_references_to_symbol(state, path, loc, include_declaration) {
        let rs: Vec<&Location> = rs.iter().map(|(v, _)| v).collect();
        let result = serde_json::to_value(rs)?;
        let resp = Response::new_ok(id, result);
        let _ = state.sender.send(resp.into());
    } else {
        let resp = Response::new_err(id, 0, "Could not find file.".into());
        let _ = state.sender.send(resp.into());
    }
    Ok(None)
}

fn handle_rename(
    state: &mut MutexGuard<'_, &mut SessionState>,
    id: RequestId,
    params: RenameParams,
) -> Result<Option<ExitCode>> {
    info!("Received textDocument/references.");
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
        state.sender.send(Message::Response(resp))?;
        return Ok(None);
    }
    let references = find_references_to_symbol(state, path, loc, true)?;
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
    state.sender.send(Message::Response(resp))?;
    Ok(None)
}

fn handle_hover(
    state: &mut MutexGuard<'_, &mut SessionState>,
    id: RequestId,
    params: HoverParams,
) -> Result<Option<ExitCode>> {
    info!("Received textDocument/hover.");
    let path = params
        .text_document_position_params
        .text_document
        .uri
        .path()
        .to_string();
    let loc = params.text_document_position_params.position.to_point();
    if let Some((md, plain)) = hover_for_symbol(state, path, loc)? {
        if let Some(td) = &state.workspace_params.capabilities.text_document {
            if let Some(hover) = &td.hover {
                if let Some(cf) = &hover.content_format {
                    if cf.contains(&MarkupKind::Markdown) {
                        let response = Hover {
                            contents: HoverContents::Markup(md),
                            range: None,
                        };
                        let resp = Response::new_ok(id, response);
                        state.sender.send(Message::Response(resp))?;
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
        state.sender.send(Message::Response(resp))?;
    } else {
        let resp = Response::new_ok(id, ());
        state.sender.send(Message::Response(resp))?;
    }
    Ok(None)
}

fn handle_highlight(
    state: &mut MutexGuard<'_, &mut SessionState>,
    id: RequestId,
    params: DocumentHighlightParams,
) -> Result<Option<ExitCode>> {
    info!("Received textDocument/highlight.");
    let path = params
        .text_document_position_params
        .text_document
        .uri
        .path()
        .to_string();
    let loc = params.text_document_position_params.position.to_point();
    let locs = find_references_to_symbol(state, path.clone(), loc, true)?;
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
    state.sender.send(Message::Response(resp))?;
    Ok(None)
}

fn handle_folding(
    state: &mut MutexGuard<'_, &mut SessionState>,
    id: RequestId,
    params: FoldingRangeParams,
) -> Result<Option<ExitCode>> {
    info!("Received textDocument/foldingRange.");
    let path = params.text_document.uri.path().to_string();
    if let Some(file) = state.files.get(&path) {
        let pf_ref = file.borrow();
        if let Some(tree) = &pf_ref.tree {
            let root = tree.root_node();
            let scm = "(block) @block";
            let query = Query::new(tree_sitter_matlab::language(), scm)?;
            let mut cursor = QueryCursor::new();
            let mut resp = vec![];
            for node in cursor
                .captures(&query, root, pf_ref.contents.as_bytes())
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
            state.sender.send(Message::Response(resp))?;
            return Ok(None);
        }
    }
    let resp = Response::new_err(
        id,
        lsp_server::ErrorCode::InvalidParams as i32,
        "File was not yet parsed.".to_owned(),
    );
    state.sender.send(Message::Response(resp))?;
    Ok(None)
}

fn handle_semantic(
    state: &mut MutexGuard<'_, &mut SessionState>,
    id: RequestId,
    params: SemanticTokensParams,
) -> Result<Option<ExitCode>> {
    info!("Received textDocument/semanticTokens/full.");
    let path = params.text_document.uri.path().to_string();
    if let Some(file) = state.files.get(&path) {
        let parsed_file = file.borrow_mut();
        let response = semantic_tokens(&parsed_file)?;
        let sts = SemanticTokens {
            result_id: None,
            data: response,
        };
        let resp = Response::new_ok(id, sts);
        state.sender.send(Message::Response(resp))?;
    } else {
        let resp = Response::new_err(
            id,
            lsp_server::ErrorCode::InvalidParams as i32,
            "File not found.".to_owned(),
        );
        state.sender.send(Message::Response(resp))?;
    }
    Ok(None)
}

fn handle_completion(
    state: &mut MutexGuard<'_, &mut SessionState>,
    id: RequestId,
    params: CompletionParams,
) -> Result<Option<ExitCode>> {
    info!("Received textDocument/completion.");
    let path = params
        .text_document_position
        .text_document
        .uri
        .path()
        .to_string();
    if let Some(file) = state.files.get(&path) {
        let parsed_file = file.borrow_mut();
        let response =
            completion::complete(state, &parsed_file, params.text_document_position.position)?;
        let resp = Response::new_ok(id, response);
        state.sender.send(Message::Response(resp))?;
    } else {
        let resp = Response::new_err(
            id,
            lsp_server::ErrorCode::InvalidParams as i32,
            "File not found.".to_owned(),
        );
        state.sender.send(Message::Response(resp))?;
    }
    Ok(None)
}

fn handle_shutdown(
    state: &mut MutexGuard<'_, &mut SessionState>,
    id: RequestId,
    _params: (),
) -> Result<Option<ExitCode>> {
    info!("Received shutdown request.");
    state.client_requested_shutdown = true;
    state.rescan_all_files = false;
    state.rescan_open_files = false;
    let resp = Response::new_ok(id, ());
    let _ = state.sender.send(resp.into());
    Ok(None)
}
