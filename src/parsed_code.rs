/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use crate::formatter::format;
use crate::utils::read_to_string;
use anyhow::{anyhow, Context, Result};
use lsp_types::Url;
use tree_sitter::Tree;

pub struct ParsedCode {
    pub file: Url,
    pub contents: String,
    pub tree: Option<Tree>,
}

impl ParsedCode {
    pub fn parse(&mut self) -> Result<()> {
        self.contents = read_to_string(&mut self.contents.as_bytes(), None)?.0;
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(tree_sitter_matlab::language())
            .with_context(|| "Could not set Tree-Sitter language")?;
        let tree = parser
            .parse(&self.contents, None)
            .ok_or_else(|| anyhow!("Could not parse file."))?;
        self.tree = Some(tree);
        eprintln!("Code parsed!!!");
        Ok(())
    }

    pub fn format(&mut self) -> Option<String> {
        if let Some(tree) = &self.tree {
            if tree.root_node().has_error() {
                return None;
            }
        } else {
            return None;
        }
        self.contents = format((self.contents.clone() + "\n").as_str()).ok()?;
        self.parse().expect("Parses formatted code.");
        Some(self.contents.clone())
    }
}
