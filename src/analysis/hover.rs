/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::sync::{Arc, MutexGuard};

use anyhow::Result;
use atomic_refcell::{AtomicRefCell, AtomicRefMut};
use itertools::Itertools;
use log::debug;
use lsp_types::{MarkupContent, MarkupKind};
use tree_sitter::Point;

use crate::parsed_file::ParsedFile;
use crate::session_state::SessionState;
use crate::types::{FunctionDefinition, VariableDefinition};

use super::defref::parent_of_kind;

pub fn hover_for_symbol(
    state: &MutexGuard<'_, &mut SessionState>,
    file: String,
    loc: Point,
) -> Result<Option<(MarkupContent, MarkupContent)>> {
    if let Some(file) = state.files.get(&file) {
        let pf_ref = file.borrow_mut();
        for reference in &pf_ref.workspace.references {
            let r_ref = reference.borrow();
            if r_ref.loc.contains(loc) {
                match &r_ref.target {
                    crate::types::ReferenceTarget::Class(cl) => {
                        return hover_simple_info(format!("Class: {}", cl.borrow().name));
                    }
                    crate::types::ReferenceTarget::ClassFolder(cf) => {
                        return hover_simple_info(format!("Class Folder: {}", cf.borrow().name));
                    }
                    crate::types::ReferenceTarget::Function(function) => {
                        let function = Arc::clone(function);
                        drop(r_ref);
                        drop(pf_ref);
                        return hover_function(function);
                    }
                    crate::types::ReferenceTarget::Namespace(ns) => {
                        return hover_simple_info(format!("Namespace: {}", ns.borrow().name));
                    }
                    crate::types::ReferenceTarget::Script(s) => {
                        return hover_simple_info(format!("Script: {}", s.borrow().name));
                    }
                    crate::types::ReferenceTarget::UnknownVariable => {
                        return hover_simple_info("Unknown variable.".into())
                    }
                    crate::types::ReferenceTarget::UnknownFunction => {
                        return hover_simple_info("Unknown function".into())
                    }
                    crate::types::ReferenceTarget::Variable(v) => {
                        return hover_variable(&pf_ref, Arc::clone(v))
                    }
                }
            }
        }
    }
    Ok(None)
}

fn hover_variable(
    parsed_file: &AtomicRefMut<'_, ParsedFile>,
    variable: Arc<AtomicRefCell<VariableDefinition>>,
) -> Result<Option<(MarkupContent, MarkupContent)>> {
    debug!("Hovering a variable.");
    let vd_ref = variable.borrow();
    if let Some(tree) = &parsed_file.tree {
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
    }
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
