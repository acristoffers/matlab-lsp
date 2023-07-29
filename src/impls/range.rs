/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::fmt::Display;

use crate::code_loc;
use crate::types::{ParsedFile, Range};
use anyhow::{anyhow, Context};
use lsp_types::Position;
use tree_sitter::Point;

impl From<tree_sitter::Range> for Range {
    fn from(value: tree_sitter::Range) -> Self {
        Range {
            start: value.start_point,
            end: value.end_point,
        }
    }
}

impl From<Range> for tree_sitter::Range {
    fn from(value: Range) -> Self {
        tree_sitter::Range {
            start_byte: 0,
            end_byte: 0,
            start_point: value.start,
            end_point: value.end,
        }
    }
}

impl From<lsp_types::Range> for Range {
    fn from(value: lsp_types::Range) -> Self {
        Range {
            start: value.start.to_point(),
            end: value.end.to_point(),
        }
    }
}

impl From<Range> for lsp_types::Range {
    fn from(value: Range) -> Self {
        lsp_types::Range {
            start: value.start.to_position(),
            end: value.end.to_position(),
        }
    }
}

impl Display for Range {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Range {{({}, {}), ({}, {})}}",
            self.start.row, self.start.column, self.end.row, self.end.column
        )
    }
}

impl Range {
    pub fn fully_contains(&self, other: Range) -> bool {
        self.contains(other.start) && self.contains(other.end)
    }

    pub fn contains(&self, other: Point) -> bool {
        self.start.row < other.row && other.row < self.end.row
            || (self.start.row == other.row || self.end.row == other.row)
                && self.start.column <= other.column
                && other.column <= self.end.column
    }

    pub fn find_bytes(&self, parsed_file: &ParsedFile) -> tree_sitter::Range {
        let mut byte = 0;
        let mut row = 0;
        let mut col = 0;
        let mut start_byte = 0;
        let mut end_byte = 0;
        if parsed_file.contents.is_empty() {
            return Range::default().into();
        }
        let contents = parsed_file
            .contents
            .replace("\r\n", "\n")
            .replace('\r', "\n");
        let mut chars = contents.chars();
        loop {
            if row == self.start.row && col == self.start.column {
                start_byte = byte;
            }
            if row == self.end.row && col == self.end.column {
                end_byte = byte;
                break;
            }
            if let Some(c) = chars.next() {
                byte += c.len_utf8();
                col += 1;
                if c == '\n' {
                    row += 1;
                    col = 0;
                }
            } else {
                break;
            }
        }
        let mut tree_range: tree_sitter::Range = self.to_owned().into();
        tree_range.start_byte = start_byte;
        tree_range.end_byte = end_byte;
        tree_range
    }
}

pub trait PosToPoint {
    fn to_point(&self) -> Point;
}

impl PosToPoint for Position {
    fn to_point(&self) -> Point {
        Point {
            row: self
                .line
                .try_into()
                .context(code_loc!("Error converting number."))
                .unwrap(),
            column: self
                .character
                .try_into()
                .context(code_loc!("Error converting number."))
                .unwrap(),
        }
    }
}

pub trait PointToPos {
    fn to_position(&self) -> Position;
}

impl PointToPos for Point {
    fn to_position(&self) -> Position {
        Position::new(
            self.row
                .try_into()
                .context(code_loc!("Error converting number."))
                .unwrap(),
            self.column
                .try_into()
                .context(code_loc!("Error converting number."))
                .unwrap(),
        )
    }
}
