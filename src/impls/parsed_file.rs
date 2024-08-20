/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::time::Instant;

use crate::features::formatter::format;
use crate::types::{ParsedFile, Workspace};
use crate::utils::read_to_string;

use anyhow::{anyhow, Context, Result};
use log::error;
use tree_sitter::Tree;

impl ParsedFile {
    pub fn new(path: String, contents: Option<String>) -> Result<ParsedFile> {
        let contents = if let Some(contents) = contents {
            contents
        } else {
            let mut file = std::fs::File::open(&path)?;
            read_to_string(&mut file, None)?.0 + "\n"
        };
        Ok(ParsedFile {
            tree: ParsedFile::ts_parse(&contents)?,
            contents,
            name: path
                .split('/')
                .last()
                .unwrap_or("")
                .strip_suffix(".m")
                .unwrap_or("")
                .into(),
            path,
            open: false,
            timestamp: Instant::now(),
            package: String::new(),
            is_script: true,
            workspace: Workspace::default(),
        })
    }

    pub fn ts_parse(contents: &String) -> Result<Tree> {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_matlab::language())
            .with_context(|| "Could not set Tree-Sitter language")?;
        parser
            .parse(contents, None)
            .ok_or_else(|| anyhow!("Could not parse file."))
    }

    pub fn load_contents(&mut self) -> Result<()> {
        if !self.open {
            let mut file = std::fs::File::open(self.path.clone())?;
            self.contents = read_to_string(&mut file, None)?.0;
        }
        Ok(())
    }

    pub fn dump_contents(&mut self) {
        if !self.open {
            self.contents = "".into();
        }
    }

    pub fn format(&mut self) -> Option<String> {
        let tree = self.tree.clone();
        if tree.root_node().has_error() {
            error!("Cannot format, has errors.");
            return None;
        }
        if let Err(err) = self.load_contents() {
            error!("Error loading contents: {err}");
            return None;
        }
        let result = format((self.contents.clone() + "\n").as_str()).ok();
        self.dump_contents();
        result
    }
}
