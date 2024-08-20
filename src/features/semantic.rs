/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::collections::HashMap;
use std::sync::Arc;

use crate::code_loc;
use crate::impls::range::PosToPoint;
use crate::types::{ParsedFile, Range, ReferenceTarget};
use anyhow::{anyhow, Result};
use lsp_types::{SemanticToken, SemanticTokenType};
use tree_sitter::{Node, Query, QueryCursor};

pub fn semantic_tokens(parsed_file: &Arc<ParsedFile>) -> Result<Vec<SemanticToken>> {
    let scm = include_str!("../queries/semantic.scm");
    let query = Query::new(&tree_sitter_matlab::language(), scm)?;
    let query_captures: HashMap<u32, String> = query
        .capture_names()
        .iter()
        .flat_map(|n| query.capture_index_for_name(n).map(|i| (i, n.to_string())))
        .collect();
    let mut cursor = QueryCursor::new();
    let tree = parsed_file.tree.clone();
    let node = tree.root_node();
    let captures: Vec<(String, Node)> = cursor
        .captures(&query, node, parsed_file.contents.as_bytes())
        .map(|(c, _)| c)
        .flat_map(|c| c.captures)
        .flat_map(|c| -> Result<(String, Node)> {
            let capture_name = query_captures
                .get(&c.index)
                .ok_or(code_loc!("Not capture for index."))?
                .clone();
            let node = c.node;
            Ok((capture_name, node))
        })
        .collect();
    semantic_tokens_impl(&captures, parsed_file)
}

fn semantic_tokens_impl(
    captures: &[(String, Node)],
    parsed_file: &Arc<ParsedFile>,
) -> Result<Vec<SemanticToken>> {
    let mut tokens = vec![];
    for (capture, node) in captures {
        let range: Range = node.range().into();
        let range: lsp_types::Range = range.into();
        match capture.as_str() {
            "number" => tokens.push(SemanticToken {
                delta_line: range.start.line,
                delta_start: range.start.character,
                length: range.end.character - range.start.character,
                token_type: token_id(SemanticTokenType::NUMBER),
                token_modifiers_bitset: 0,
            }),
            "comment" => tokens.push(SemanticToken {
                delta_line: range.start.line,
                delta_start: range.start.character,
                length: (node.byte_range().end - node.byte_range().start).try_into()?,
                token_type: token_id(SemanticTokenType::COMMENT),
                token_modifiers_bitset: 0,
            }),
            "string" => tokens.push(SemanticToken {
                delta_line: range.start.line,
                delta_start: range.start.character,
                length: range.end.character - range.start.character,
                token_type: token_id(SemanticTokenType::STRING),
                token_modifiers_bitset: 0,
            }),
            "operator" => tokens.push(SemanticToken {
                delta_line: range.start.line,
                delta_start: range.start.character,
                length: range.end.character - range.start.character,
                token_type: token_id(SemanticTokenType::OPERATOR),
                token_modifiers_bitset: 0,
            }),
            "keyword" => tokens.push(SemanticToken {
                delta_line: range.start.line,
                delta_start: range.start.character,
                length: range.end.character - range.start.character,
                token_type: token_id(SemanticTokenType::KEYWORD),
                token_modifiers_bitset: 0,
            }),
            "parameter" => tokens.push(SemanticToken {
                delta_line: range.start.line,
                delta_start: range.start.character,
                length: range.end.character - range.start.character,
                token_type: token_id(SemanticTokenType::PARAMETER),
                token_modifiers_bitset: 0,
            }),
            "function" => tokens.push(SemanticToken {
                delta_line: range.start.line,
                delta_start: range.start.character,
                length: range.end.character - range.start.character,
                token_type: token_id(SemanticTokenType::FUNCTION),
                token_modifiers_bitset: 0,
            }),
            "identifer" => {
                if let Some(token) = st_for_identifier(*node, parsed_file)? {
                    tokens.push(token);
                }
            }
            _ => {}
        }
    }
    Ok(deltalize_tokens(&tokens))
}

fn st_for_identifier(node: Node, parsed_file: &Arc<ParsedFile>) -> Result<Option<SemanticToken>> {
    let range: Range = node.range().into();
    let range: lsp_types::Range = range.into();
    let mut ttype = None;
    if node.utf8_text(parsed_file.contents.as_bytes())? == "end" {
        ttype = Some(SemanticTokenType::KEYWORD);
    }
    for reference in &parsed_file.workspace.references {
        if ttype.is_some() {
            break;
        }
        let r_ref = reference.borrow();
        if r_ref.loc.contains(range.start.to_point()) {
            ttype = match &r_ref.target {
                ReferenceTarget::Function(_) => Some(SemanticTokenType::FUNCTION),
                ReferenceTarget::Namespace(_) => Some(SemanticTokenType::NAMESPACE),
                ReferenceTarget::Script(_) => Some(SemanticTokenType::FUNCTION),
                ReferenceTarget::UnknownFunction => Some(SemanticTokenType::FUNCTION),
                ReferenceTarget::Variable(v) => {
                    if r_ref.name.contains('.') {
                        Some(SemanticTokenType::PROPERTY)
                    } else if v.borrow().is_parameter {
                        Some(SemanticTokenType::PARAMETER)
                    } else {
                        Some(SemanticTokenType::VARIABLE)
                    }
                }
                _ => {
                    if r_ref.name.contains('.') {
                        Some(SemanticTokenType::PROPERTY)
                    } else {
                        Some(SemanticTokenType::VARIABLE)
                    }
                }
            }
        }
    }
    for variable in &parsed_file.workspace.variables {
        if ttype.is_some() {
            break;
        }
        let v_ref = variable.borrow();
        if v_ref.loc.contains(range.start.to_point()) {
            ttype = if v_ref.name.contains('.') {
                Some(SemanticTokenType::PROPERTY)
            } else {
                Some(SemanticTokenType::VARIABLE)
            };
        }
    }
    if let Some(ttype) = ttype {
        Ok(Some(SemanticToken {
            delta_line: range.start.line,
            delta_start: range.start.character,
            length: range.end.character - range.start.character,
            token_type: token_id(ttype),
            token_modifiers_bitset: 0,
        }))
    } else {
        Ok(None)
    }
}

fn token_id(t: SemanticTokenType) -> u32 {
    let semantic_token_types = vec![
        SemanticTokenType::NAMESPACE,
        SemanticTokenType::TYPE,
        SemanticTokenType::CLASS,
        SemanticTokenType::ENUM,
        SemanticTokenType::INTERFACE,
        SemanticTokenType::STRUCT,
        SemanticTokenType::TYPE_PARAMETER,
        SemanticTokenType::PARAMETER,
        SemanticTokenType::VARIABLE,
        SemanticTokenType::PROPERTY,
        SemanticTokenType::ENUM_MEMBER,
        SemanticTokenType::EVENT,
        SemanticTokenType::FUNCTION,
        SemanticTokenType::METHOD,
        SemanticTokenType::MACRO,
        SemanticTokenType::KEYWORD,
        SemanticTokenType::MODIFIER,
        SemanticTokenType::COMMENT,
        SemanticTokenType::STRING,
        SemanticTokenType::NUMBER,
        SemanticTokenType::REGEXP,
        SemanticTokenType::OPERATOR,
    ];
    if let Some(i) = semantic_token_types.iter().position(|v| *v == t) {
        i.try_into().unwrap()
    } else {
        0
    }
}

fn deltalize_tokens(ts: &[SemanticToken]) -> Vec<SemanticToken> {
    if ts.is_empty() {
        return vec![];
    }
    let mut tokens = vec![*ts.first().unwrap()];
    for (i, token) in ts.iter().skip(1).enumerate() {
        let last = ts[i];
        let mut token = *token;
        token.delta_line -= last.delta_line;
        if token.delta_line == 0 {
            token.delta_start -= last.delta_start;
        }
        tokens.push(token);
    }
    tokens
}
