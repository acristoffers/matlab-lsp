/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::sync::Arc;

use anyhow::Result;
use atomic_refcell::AtomicRefCell;
use crossbeam_channel::{Receiver, Sender};
use itertools::Itertools;
use log::debug;
use lsp_types::{MarkupContent, MarkupKind};
use tree_sitter::Point;

use crate::extractors::symbols::parent_of_kind;
use crate::threads::db::db_get_parsed_file;
use crate::types::{
    FunctionDefinition, ParsedFile, SenderThread, ThreadMessage, VariableDefinition,
};

pub fn hover_for_symbol(
    sender: Sender<ThreadMessage>,
    receiver: Receiver<ThreadMessage>,
    file: String,
    loc: Point,
) -> Result<Option<(MarkupContent, MarkupContent)>> {
    if let Some(file) = db_get_parsed_file(&sender, &receiver, file.clone(), SenderThread::Handler)
    {
        for reference in &file.workspace.references {
            let r_ref = reference.borrow();
            if r_ref.loc.contains(loc) {
                match &r_ref.target {
                    crate::types::ReferenceTarget::Function(function) => {
                        return hover_function(function.clone());
                    }
                    crate::types::ReferenceTarget::Namespace(ns) => {
                        return hover_simple_info(format!("Namespace: {}", ns));
                    }
                    crate::types::ReferenceTarget::Script(s) => {
                        return hover_simple_info(format!("Script: {}", s));
                    }
                    crate::types::ReferenceTarget::UnknownVariable => {
                        return hover_simple_info("Unknown variable.".into())
                    }
                    crate::types::ReferenceTarget::UnknownFunction => {
                        return hover_simple_info("Unknown function".into())
                    }
                    crate::types::ReferenceTarget::Variable(v) => {
                        return hover_variable(&file, v.clone())
                    }
                }
            }
        }
    }
    Ok(None)
}

fn hover_variable(
    parsed_file: &Arc<ParsedFile>,
    variable: Arc<AtomicRefCell<VariableDefinition>>,
) -> Result<Option<(MarkupContent, MarkupContent)>> {
    debug!("Hovering a variable.");
    let vd_ref = variable.borrow();
    let tree = parsed_file.tree.clone();
    debug!("Checking for node at {}", vd_ref.loc);
    if let Some(node) = tree
        .root_node()
        .named_descendant_for_point_range(vd_ref.loc.start, vd_ref.loc.end)
    {
        if let Some(parent) = parent_of_kind("assignment", node) {
            let code = parent.utf8_text(parsed_file.contents.as_bytes())?;
            let md = MarkupContent {
                kind: MarkupKind::Markdown,
                value: format!(
                    "Line {}:\n```matlab\n{code}\n```",
                    parent.start_position().row + 1
                ),
            };
            let plain = MarkupContent {
                kind: MarkupKind::PlainText,
                value: code.to_string(),
            };
            return Ok(Some((md, plain)));
        } else if let Some(parent) = node.parent() {
            let code = parent.utf8_text(parsed_file.contents.as_bytes())?;
            let md = MarkupContent {
                kind: MarkupKind::Markdown,
                value: format!(
                    "Line {}:\n```matlab\n{code}\n```",
                    parent.start_position().row + 1
                ),
            };
            let plain = MarkupContent {
                kind: MarkupKind::PlainText,
                value: code.to_string(),
            };
            return Ok(Some((md, plain)));
        }
    }
    debug!("No node found at point.");
    Ok(None)
}

fn hover_function(
    function: Arc<AtomicRefCell<FunctionDefinition>>,
) -> Result<Option<(MarkupContent, MarkupContent)>> {
    debug!("Hovering a function.");
    let sig = &function.borrow().signature;
    let mut fsig = "function ".to_string();
    if !sig.argout_names.is_empty() {
        if sig.argout_names.len() == 1 {
            fsig += sig.argout_names.first().unwrap();
        } else {
            fsig += format!("[{}]", sig.argout_names.iter().join(", ")).as_str();
        }
        fsig += " = ";
    }
    fsig += sig.name.as_str();
    fsig += format!("({})", sig.argin_names.iter().join(", ")).as_str();
    let md = MarkupContent {
        kind: MarkupKind::Markdown,
        value: format!("```matlab\n{}\n```\n---\n{}", fsig, sig.documentation),
    };
    let plain = MarkupContent {
        kind: MarkupKind::PlainText,
        value: format!("{}\n\n{}", fsig, sig.documentation),
    };
    Ok(Some((md, plain)))
}

fn hover_simple_info(info: String) -> Result<Option<(MarkupContent, MarkupContent)>> {
    let md = MarkupContent {
        kind: MarkupKind::Markdown,
        value: info.clone(),
    };
    let plain = MarkupContent {
        kind: MarkupKind::PlainText,
        value: info,
    };
    Ok(Some((md, plain)))
}
