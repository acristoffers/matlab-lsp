/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::collections::HashMap;

use crate::features::completion::complete;
use crate::features::hover::hover_for_symbol;
use crate::features::references::find_references_to_symbol;
use crate::features::semantic::semantic_tokens;
use crate::impls::range::{PointToPos, PosToPoint};
use crate::threads::db::db_get_parsed_file;
use crate::types::{Range, SenderThread, ThreadMessage};

use anyhow::{anyhow, Result};
use crossbeam_channel::{Receiver, Sender};
use log::{debug, info};
use lsp_server::{ExtractError, Message, Request, RequestId, Response};
use lsp_types::request::{
    Completion, DocumentHighlightRequest, FoldingRangeRequest, Formatting, GotoDefinition,
    HoverRequest, References, Rename, SemanticTokensFullRequest,
};
use lsp_types::{
    CompletionParams, DocumentFormattingParams, DocumentHighlight, DocumentHighlightParams,
    FoldingRange, FoldingRangeKind, FoldingRangeParams, GotoDefinitionParams,
    GotoDefinitionResponse, Hover, HoverContents, HoverParams, Location, Position, ReferenceParams,
    RenameParams, SemanticTokens, SemanticTokensParams, TextEdit, Url, WorkspaceEdit,
};
use regex::Regex;
use tree_sitter::{Point, Query, QueryCursor};

pub fn handle_request(
    lsp_sender: Sender<Message>,
    sender: Sender<ThreadMessage>,
    receiver: Receiver<ThreadMessage>,
    request: Request,
) -> Result<()> {
    let mut dispatcher = Dispatcher::new(lsp_sender, sender, receiver, request);
    dispatcher
        .handle::<Formatting>(handle_formatting)
        .handle::<GotoDefinition>(handle_goto_definition)
        .handle::<References>(handle_references)
        .handle::<Rename>(handle_rename)
        .handle::<HoverRequest>(handle_hover)
        .handle::<DocumentHighlightRequest>(handle_highlight)
        .handle::<FoldingRangeRequest>(handle_folding)
        .handle::<SemanticTokensFullRequest>(handle_semantic)
        .handle::<Completion>(handle_completion)
        .finish()
}

struct Dispatcher {
    lsp_sender: Sender<Message>,
    sender: Sender<ThreadMessage>,
    receiver: Receiver<ThreadMessage>,
    request: Request,
    result: Option<Result<()>>,
}

type Callback<P> =
    fn(Sender<Message>, Sender<ThreadMessage>, Receiver<ThreadMessage>, RequestId, P) -> Result<()>;

impl Dispatcher {
    fn new(
        lsp_sender: Sender<Message>,
        sender: Sender<ThreadMessage>,
        receiver: Receiver<ThreadMessage>,
        request: Request,
    ) -> Dispatcher {
        Dispatcher {
            lsp_sender,
            sender,
            receiver,
            request,
            result: None,
        }
    }

    fn handle<R>(&mut self, function: Callback<R::Params>) -> &mut Self
    where
        R: lsp_types::request::Request,
        R::Params: serde::de::DeserializeOwned,
    {
        let result = match cast::<R>(self.request.clone()) {
            Ok((id, params)) => function(
                self.lsp_sender.clone(),
                self.sender.clone(),
                self.receiver.clone(),
                id,
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

fn cast<R>(request: Request) -> Result<(RequestId, R::Params), ExtractError<Request>>
where
    R: lsp_types::request::Request,
    R::Params: serde::de::DeserializeOwned,
{
    request.extract(R::METHOD)
}

fn handle_formatting(
    lsp_sender: Sender<Message>,
    sender: Sender<ThreadMessage>,
    receiver: Receiver<ThreadMessage>,
    id: RequestId,
    params: DocumentFormattingParams,
) -> Result<()> {
    info!("Formatting {}", params.text_document.uri.as_str());
    let path = params.text_document.uri.path();
    let mut file = if let Some(file) =
        db_get_parsed_file(&sender, &receiver, path.to_string(), SenderThread::Handler)
    {
        file.as_ref().clone()
    } else {
        return Ok(());
    };
    let pos = file.tree.root_node().end_position();
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
        lsp_sender.send(Message::Response(resp))?;
    } else {
        return Err(anyhow!("Error formatting"));
    }
    Ok(())
}

fn handle_goto_definition(
    lsp_sender: Sender<Message>,
    sender: Sender<ThreadMessage>,
    receiver: Receiver<ThreadMessage>,
    id: RequestId,
    params: GotoDefinitionParams,
) -> Result<()> {
    let uri = params.text_document_position_params.text_document.uri;
    let path = uri.path().to_string();
    let loc = params.text_document_position_params.position;
    let loc = Point {
        row: loc.line.try_into()?,
        column: loc.character.try_into()?,
    };
    if let Some(file) = db_get_parsed_file(&sender, &receiver, path, SenderThread::Handler) {
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
                    crate::types::ReferenceTarget::Function(fun) => {
                        let path = fun.borrow().path.clone();
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
                    crate::types::ReferenceTarget::Script(path) => {
                        let path = String::from("file://") + path.as_str();
                        Some(GotoDefinitionResponse::from(Location::new(
                            Url::parse(path.as_str())?,
                            Range::default().into(),
                        )))
                    }
                    crate::types::ReferenceTarget::Namespace(_) => None,
                    crate::types::ReferenceTarget::UnknownVariable => None,
                    crate::types::ReferenceTarget::UnknownFunction => None,
                };
                if let Some(resp) = resp {
                    let resp = Response::new_ok(id, resp);
                    lsp_sender.send(Message::Response(resp))?;
                } else {
                    debug!("Matched, got None");
                    let resp = Response::new_ok(id, ());
                    let _ = lsp_sender.send(resp.into());
                }
                return Ok(());
            }
        }
        debug!("Point not in range.");
        let resp = Response::new_ok(id, ());
        let _ = lsp_sender.send(resp.into());
    } else {
        let resp = Response::new_err(id, 0, "Could not find file.".into());
        let _ = lsp_sender.send(resp.into());
    }
    Ok(())
}

fn handle_references(
    lsp_sender: Sender<Message>,
    sender: Sender<ThreadMessage>,
    receiver: Receiver<ThreadMessage>,
    id: RequestId,
    params: ReferenceParams,
) -> Result<()> {
    info!("Received textDocument/references.");
    let include_declaration = params.context.include_declaration;
    let path = params
        .text_document_position
        .text_document
        .uri
        .path()
        .to_string();
    let loc = params.text_document_position.position.to_point();
    if let Ok(rs) = find_references_to_symbol(
        sender.clone(),
        receiver.clone(),
        path,
        loc,
        include_declaration,
    ) {
        let rs: Vec<&Location> = rs.iter().map(|(v, _)| v).collect();
        let result = serde_json::to_value(rs)?;
        let resp = Response::new_ok(id, result);
        let _ = lsp_sender.send(resp.into());
    } else {
        let resp = Response::new_err(id, 0, "Could not find file.".into());
        let _ = lsp_sender.send(resp.into());
    }
    Ok(())
}

fn handle_rename(
    lsp_sender: Sender<Message>,
    sender: Sender<ThreadMessage>,
    receiver: Receiver<ThreadMessage>,
    id: RequestId,
    params: RenameParams,
) -> Result<()> {
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
        lsp_sender.send(Message::Response(resp))?;
        return Ok(());
    }
    let references = find_references_to_symbol(sender.clone(), receiver.clone(), path, loc, true)?;
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
    lsp_sender.send(Message::Response(resp))?;
    Ok(())
}

fn handle_hover(
    lsp_sender: Sender<Message>,
    sender: Sender<ThreadMessage>,
    receiver: Receiver<ThreadMessage>,
    id: RequestId,
    params: HoverParams,
) -> Result<()> {
    info!("Received textDocument/hover.");
    let path = params
        .text_document_position_params
        .text_document
        .uri
        .path()
        .to_string();
    let loc = params.text_document_position_params.position.to_point();
    if let Some((md, _)) = hover_for_symbol(sender.clone(), receiver.clone(), path, loc)? {
        let response = Hover {
            contents: HoverContents::Markup(md),
            range: None,
        };
        let resp = Response::new_ok(id, response);
        lsp_sender.send(Message::Response(resp))?;
        return Ok(());
    } else {
        let resp = Response::new_ok(id, ());
        lsp_sender.send(Message::Response(resp))?;
    }
    Ok(())
}

fn handle_highlight(
    lsp_sender: Sender<Message>,
    sender: Sender<ThreadMessage>,
    receiver: Receiver<ThreadMessage>,
    id: RequestId,
    params: DocumentHighlightParams,
) -> Result<()> {
    info!("Received textDocument/highlight.");
    let path = params
        .text_document_position_params
        .text_document
        .uri
        .path()
        .to_string();
    let loc = params.text_document_position_params.position.to_point();
    let locs =
        find_references_to_symbol(sender.clone(), receiver.clone(), path.clone(), loc, true)?;
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
    lsp_sender.send(Message::Response(resp))?;
    Ok(())
}

fn handle_folding(
    lsp_sender: Sender<Message>,
    sender: Sender<ThreadMessage>,
    receiver: Receiver<ThreadMessage>,
    id: RequestId,
    params: FoldingRangeParams,
) -> Result<()> {
    info!("Received textDocument/foldingRange.");
    let path = params.text_document.uri.path().to_string();
    if let Some(file) = db_get_parsed_file(&sender, &receiver, path, SenderThread::Handler) {
        let tree = file.tree.clone();
        let root = tree.root_node();
        let scm = "(block) @block";
        let query = Query::new(&tree_sitter_matlab::language(), scm)?;
        let mut cursor = QueryCursor::new();
        let mut resp = vec![];
        for node in cursor
            .captures(&query, root, file.contents.as_bytes())
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
        lsp_sender.send(Message::Response(resp))?;
        return Ok(());
    }
    let resp = Response::new_err(
        id,
        lsp_server::ErrorCode::InvalidParams as i32,
        "File was not yet parsed.".to_owned(),
    );
    lsp_sender.send(Message::Response(resp))?;
    Ok(())
}

fn handle_semantic(
    lsp_sender: Sender<Message>,
    sender: Sender<ThreadMessage>,
    receiver: Receiver<ThreadMessage>,
    id: RequestId,
    params: SemanticTokensParams,
) -> Result<()> {
    info!("Received textDocument/semanticTokens/full.");
    let path = params.text_document.uri.path().to_string();
    if let Some(file) = db_get_parsed_file(&sender, &receiver, path, SenderThread::Handler) {
        let response = semantic_tokens(&file)?;
        let sts = SemanticTokens {
            result_id: None,
            data: response,
        };
        let resp = Response::new_ok(id, sts);
        lsp_sender.send(Message::Response(resp))?;
    } else {
        let resp = Response::new_err(
            id,
            lsp_server::ErrorCode::InvalidParams as i32,
            "File not found.".to_owned(),
        );
        lsp_sender.send(Message::Response(resp))?;
    }
    Ok(())
}

fn handle_completion(
    lsp_sender: Sender<Message>,
    sender: Sender<ThreadMessage>,
    receiver: Receiver<ThreadMessage>,
    id: RequestId,
    params: CompletionParams,
) -> Result<()> {
    info!("Received textDocument/completion.");
    let path = params
        .text_document_position
        .text_document
        .uri
        .path()
        .to_string();
    if let Some(file) = db_get_parsed_file(&sender, &receiver, path, SenderThread::Handler) {
        let response = complete(
            sender.clone(),
            receiver.clone(),
            file,
            params.text_document_position.position,
        )?;
        let resp = Response::new_ok(id, response);
        lsp_sender.send(Message::Response(resp))?;
    } else {
        let resp = Response::new_err(
            id,
            lsp_server::ErrorCode::InvalidParams as i32,
            "File not found.".to_owned(),
        );
        lsp_sender.send(Message::Response(resp))?;
    }
    Ok(())
}
