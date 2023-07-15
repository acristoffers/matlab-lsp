/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::Result;
use itertools::Itertools;
use log::debug;
use lsp_types::{MarkupContent, MarkupKind};
use tree_sitter::Point;

use crate::parsed_file::ParsedFile;
use crate::session_state::SessionState;
use crate::types::{FunctionDefinition, VariableDefinition};
use crate::utils::lock_mutex;

use super::defref::parent_of_kind;

pub fn hover_for_symbol(
    state: &MutexGuard<'_, &mut SessionState>,
    file: String,
    loc: Point,
) -> Result<Option<(MarkupContent, MarkupContent)>> {
    if let Some(file) = state.files.get(&file) {
        let lock = lock_mutex(file)?;
        for reference in &lock.workspace.references {
            let r_lock = lock_mutex(reference)?;
            if r_lock.loc.contains(loc) {
                match &r_lock.target {
                    crate::types::ReferenceTarget::Class(cl) => {
                        let lock = lock_mutex(cl)?;
                        return hover_simple_info(format!("Class: {}", lock.name));
                    }
                    crate::types::ReferenceTarget::ClassFolder(cf) => {
                        let lock = lock_mutex(cf)?;
                        return hover_simple_info(format!("Class Folder: {}", lock.name));
                    }
                    crate::types::ReferenceTarget::Function(function) => {
                        let function = Arc::clone(function);
                        drop(r_lock);
                        drop(lock);
                        return hover_function(function);
                    }
                    crate::types::ReferenceTarget::Namespace(ns) => {
                        let lock = lock_mutex(ns)?;
                        return hover_simple_info(format!("Namespace: {}", lock.name));
                    }
                    crate::types::ReferenceTarget::Script(s) => {
                        let lock = lock_mutex(s)?;
                        return hover_simple_info(format!("Script: {}", lock.name));
                    }
                    crate::types::ReferenceTarget::UnknownVariable => {
                        return hover_simple_info("Unknown variable.".into())
                    }
                    crate::types::ReferenceTarget::UnknownFunction => {
                        return hover_simple_info("Unknown function".into())
                    }
                    crate::types::ReferenceTarget::Variable(v) => {
                        return hover_variable(&lock, Arc::clone(v))
                    }
                }
            }
        }
    }
    Ok(None)
}

fn hover_variable(
    parsed_file: &MutexGuard<'_, ParsedFile>,
    variable: Arc<Mutex<VariableDefinition>>,
) -> Result<Option<(MarkupContent, MarkupContent)>> {
    debug!("Hovering a variable.");
    let lock = lock_mutex(&variable)?;
    if let Some(tree) = &parsed_file.tree {
        debug!("Checking for node at {}", lock.loc);
        if let Some(node) = tree
            .root_node()
            .named_descendant_for_point_range(lock.loc.start, lock.loc.end)
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
    }
    Ok(None)
}

fn hover_function(
    function: Arc<Mutex<FunctionDefinition>>,
) -> Result<Option<(MarkupContent, MarkupContent)>> {
    debug!("Hovering a function.");
    let lock = lock_mutex(&function)?;
    let sig = &lock.signature;
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
