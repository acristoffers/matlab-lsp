/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::sync::MutexGuard;

use atomic_refcell::AtomicRefMut;
use itertools::Itertools;
use lsp_types::{
    CompletionItem, CompletionItemKind, InsertTextFormat, MarkupContent, MarkupKind, Position,
};
use tree_sitter::Point;

use crate::analysis::defref::parent_of_kind;
use crate::parsed_file::ParsedFile;
use crate::session_state::SessionState;
use crate::types::{PosToPoint, Range, ReferenceTarget};
use anyhow::Result;

pub fn complete(
    state: &MutexGuard<'_, &mut SessionState>,
    pf_mr: &AtomicRefMut<'_, ParsedFile>,
    pos: Position,
) -> Result<Vec<CompletionItem>> {
    let mut result = vec![];
    let point = pos.to_point();
    let identifier = identifier(pf_mr, point);
    result.extend(variable_completions(pf_mr, &identifier, point));
    result.extend(function_completions(state, pf_mr, &identifier));
    result.extend(namespace_completions(state, &identifier));
    result.extend(classfolder_completions(state, &identifier));
    result.extend(class_completions(state, &identifier));
    result.extend(script_completions(state, &identifier));
    result.extend(reference_completions(pf_mr, &identifier, point));
    result.sort_by(|a, b| a.label.cmp(&b.label));
    result.dedup_by(|a, b| a.label == b.label);
    Ok(result)
}

fn identifier(pf_mr: &AtomicRefMut<'_, ParsedFile>, pos: Point) -> String {
    let mut range = Range {
        start: pos,
        end: pos,
    };
    range.start.column = 0;
    let line_range = range.find_bytes(pf_mr);
    let line = &pf_mr.contents[line_range.start_byte..line_range.end_byte];
    let line: String = line
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit() || c.is_ascii_alphabetic() || c.eq(&'_') || c.eq(&'.'))
        .collect();
    line.chars().rev().collect()
}

fn variable_completions(
    pf_mr: &AtomicRefMut<'_, ParsedFile>,
    text: &str,
    point: Point,
) -> Vec<CompletionItem> {
    let mut completions = vec![];
    for var in &pf_mr.workspace.variables {
        let var_ref = var.borrow();
        if var_ref.loc.start.row >= point.row || var_ref.cleared > 0 && var_ref.cleared < point.row
        {
            continue;
        }
        if var_ref.name.starts_with(text) {
            let mut code = String::new();
            if let Some(tree) = &pf_mr.tree {
                if let Some(node) = tree
                    .root_node()
                    .named_descendant_for_point_range(var_ref.loc.start, var_ref.loc.start)
                {
                    if let Some(parent) = parent_of_kind("assignment", node) {
                        if let Ok(text) = parent.utf8_text(pf_mr.contents.as_bytes()) {
                            code = text.to_string();
                        }
                    }
                }
            }
            let completion = CompletionItem {
                label: var_ref.name.clone(),
                label_details: None,
                kind: Some(if var_ref.name.contains('.') {
                    CompletionItemKind::FIELD
                } else {
                    CompletionItemKind::VARIABLE
                }),
                documentation: Some(lsp_types::Documentation::MarkupContent(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: format!("Line {}:\n```matlab\n{code}\n```", var_ref.loc.start.row),
                })),
                deprecated: Some(false),
                preselect: Some(false),
                ..CompletionItem::default()
            };
            completions.push(completion);
        }
    }
    completions
}

fn reference_completions(
    pf_mr: &AtomicRefMut<'_, ParsedFile>,
    text: &str,
    point: Point,
) -> Vec<CompletionItem> {
    let mut completions = vec![];
    for var in &pf_mr.workspace.references {
        let var = var.borrow();
        if let ReferenceTarget::Variable(def) = &var.target {
            let def = def.borrow();
            if def.loc.start.row > point.row || def.cleared > 0 && def.cleared < point.row {
                continue;
            }
        }
        if var.name.starts_with(text) {
            let completion = CompletionItem {
                label: var.name.clone(),
                label_details: None,
                kind: Some(if var.name.contains('.') {
                    CompletionItemKind::FIELD
                } else {
                    CompletionItemKind::VARIABLE
                }),
                deprecated: Some(false),
                preselect: Some(false),
                ..CompletionItem::default()
            };
            completions.push(completion);
        }
    }
    completions
}

fn namespace_completions(
    state: &MutexGuard<'_, &mut SessionState>,
    text: &str,
) -> Vec<CompletionItem> {
    let mut completions = vec![];
    for name in state.workspace.namespaces.keys() {
        if name.starts_with(text) {
            let completion = CompletionItem {
                label: name.clone(),
                label_details: None,
                kind: Some(CompletionItemKind::MODULE),
                deprecated: Some(false),
                preselect: Some(false),
                ..CompletionItem::default()
            };
            completions.push(completion);
        }
    }
    completions
}

fn classfolder_completions(
    state: &MutexGuard<'_, &mut SessionState>,
    text: &str,
) -> Vec<CompletionItem> {
    let mut completions = vec![];
    for name in state.workspace.functions.keys() {
        if name.starts_with(text) {
            let completion = CompletionItem {
                label: name.clone(),
                label_details: None,
                kind: Some(CompletionItemKind::CLASS),
                deprecated: Some(false),
                preselect: Some(false),
                ..CompletionItem::default()
            };
            completions.push(completion);
        }
    }
    completions
}

fn function_completions(
    state: &MutexGuard<'_, &mut SessionState>,
    pf_mr: &AtomicRefMut<'_, ParsedFile>,
    text: &str,
) -> Vec<CompletionItem> {
    let mut completions = vec![];
    let functions = state
        .workspace
        .functions
        .iter()
        .chain(pf_mr.workspace.functions.iter());
    for (name, function) in functions {
        if name.starts_with(text) {
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
            let function = function.borrow();
            let insert_text = format!(
                "{}({})",
                function.name,
                function
                    .signature
                    .argin_names
                    .iter()
                    .enumerate()
                    .map(|(i, v)| format!("${{{}:{v}}}", i + 1))
                    .join(", ")
            );
            let completion = CompletionItem {
                label: name.clone(),
                label_details: None,
                insert_text: Some(insert_text),
                insert_text_format: Some(InsertTextFormat::SNIPPET),
                documentation: Some(lsp_types::Documentation::MarkupContent(md)),
                kind: Some(CompletionItemKind::FUNCTION),
                deprecated: Some(false),
                preselect: Some(false),
                ..CompletionItem::default()
            };
            completions.push(completion);
        }
    }
    completions
}

fn class_completions(state: &MutexGuard<'_, &mut SessionState>, text: &str) -> Vec<CompletionItem> {
    let mut completions = vec![];
    for name in state.workspace.classes.keys() {
        if name.starts_with(text) {
            let completion = CompletionItem {
                label: name.clone(),
                label_details: None,
                kind: Some(CompletionItemKind::CLASS),
                deprecated: Some(false),
                preselect: Some(false),
                ..CompletionItem::default()
            };
            completions.push(completion);
        }
    }
    completions
}

fn script_completions(
    state: &MutexGuard<'_, &mut SessionState>,
    text: &str,
) -> Vec<CompletionItem> {
    let mut completions = vec![];
    for name in state.workspace.scripts.keys() {
        if name.starts_with(text) {
            let completion = CompletionItem {
                label: name.clone(),
                label_details: None,
                kind: Some(CompletionItemKind::FILE),
                deprecated: Some(false),
                preselect: Some(false),
                ..CompletionItem::default()
            };
            completions.push(completion);
        }
    }
    completions
}
