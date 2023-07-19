/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::sync::MutexGuard;

use crate::parsed_file::ParsedFile;
use crate::session_state::SessionState;
use crate::types::Range;
use anyhow::Result;
use atomic_refcell::AtomicRefMut;
use lsp_server::Message;
use lsp_types::notification::{Notification, PublishDiagnostics};
use lsp_types::{Diagnostic, DiagnosticSeverity, PublishDiagnosticsParams, Url};
use tree_sitter::{Query, QueryCursor};

pub fn diagnotiscs(
    state: &mut MutexGuard<'_, &mut SessionState>,
    parsed_file: &AtomicRefMut<'_, ParsedFile>,
) -> Result<()> {
    let mut diagnostics = vec![];
    if let Some(tree) = &parsed_file.tree {
        let root = tree.root_node();
        if root.has_error() {
            let scm = "(ERROR) @error";
            let query = Query::new(tree_sitter_matlab::language(), scm)?;
            let mut cursor = QueryCursor::new();
            for node in cursor
                .captures(&query, root, parsed_file.contents.as_bytes())
                .map(|(c, _)| c)
                .flat_map(|c| c.captures)
                .map(|c| c.node)
            {
                let range: lsp_types::Range = Range::from(node.range()).into();
                let diagnotisc = Diagnostic {
                    range,
                    severity: Some(DiagnosticSeverity::ERROR),
                    code: None,
                    code_description: None,
                    source: Some("tree-sitter".into()),
                    message: "There is a syntax error somewhere here...".into(),
                    related_information: None,
                    tags: None,
                    data: None,
                };
                diagnostics.push(diagnotisc);
            }
        }
    }
    for reference in &parsed_file.workspace.references {
        let ref_ref = reference.borrow();
        if ref_ref.name.contains('.') {
            continue;
        }
        if let crate::types::ReferenceTarget::UnknownVariable = ref_ref.target {
            let diagnotisc = Diagnostic {
                range: ref_ref.loc.into(),
                severity: Some(DiagnosticSeverity::WARNING),
                code: None,
                code_description: None,
                source: Some("matlab-lsp".into()),
                message: "This variable seems to be undefined".into(),
                related_information: None,
                tags: None,
                data: None,
            };
            diagnostics.push(diagnotisc);
        }
    }
    let path = String::from("file://") + parsed_file.path.as_str();
    let uri = Url::parse(&path)?;
    let diagnostic_params = PublishDiagnosticsParams {
        uri,
        diagnostics,
        version: None,
    };
    let notification = Message::Notification(lsp_server::Notification {
        method: PublishDiagnostics::METHOD.to_string(),
        params: serde_json::to_value(diagnostic_params)?,
    });
    state.sender.send(notification)?;
    Ok(())
}
