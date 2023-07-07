/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::sync::{Arc, Mutex, MutexGuard};

use crate::parsed_file::ParsedFile;
use crate::session_state::SessionState;

use anyhow::{anyhow, Result};
use lsp_types::Range;
use tree_sitter::Point;

pub type SessionStateArc = Arc<Mutex<&'static mut SessionState>>;

/// Transforms a Range into bytes
pub(crate) fn range_to_bytes(range: Range, parsed_file: &ParsedFile) -> Result<(usize, usize)> {
    let start = Point::new(
        range.start.line.try_into()?,
        range.start.character.try_into()?,
    );
    let end = Point::new(range.end.line.try_into()?, range.end.character.try_into()?);
    let mut byte = 0;
    let mut row = 0;
    let mut col = 0;
    let mut start_byte = 0;
    let mut end_byte = 0;
    let mut chars = parsed_file.contents.chars();
    if let Some(tree) = &parsed_file.tree {
        if let Some(node) = tree.root_node().descendant_for_point_range(start, end) {
            byte = node.start_byte();
            row = node.start_position().row;
            col = node.start_position().column;
            chars = parsed_file.contents[byte..].chars();
        }
    }
    loop {
        if row == start.row && col == start.column {
            start_byte = byte;
        }
        if row == end.row && col == end.column {
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
    Ok((start_byte, end_byte))
}

/// Locks a mutex, and adds an error message in case of error.
pub(crate) fn lock_mutex<T>(arc: &Arc<Mutex<T>>) -> Result<MutexGuard<'_, T>> {
    arc.lock().map_err(|_| anyhow!("Could not lock mutex."))
}

// The functions below were taken from the helix editor

/// Reads the first chunk from a Reader into the given buffer
/// and detects the encoding.
///
/// By default, the encoding of the text is auto-detected by
/// `encoding_rs` for_bom, and if it fails, from `chardetng`
/// crate which requires sample data from the reader.
/// As a manual override to this auto-detection is possible, the
/// same data is read into `buf` to ensure symmetry in the upcoming
/// loop.
fn read_and_detect_encoding<R: std::io::Read + ?Sized>(
    reader: &mut R,
    encoding: Option<&'static encoding_rs::Encoding>,
    buf: &mut [u8],
) -> Result<(
    &'static encoding_rs::Encoding,
    bool,
    encoding_rs::Decoder,
    usize,
)> {
    let read = reader.read(buf)?;
    let is_empty = read == 0;
    let (encoding, has_bom) = encoding
        .map(|encoding| (encoding, false))
        .or_else(|| {
            encoding_rs::Encoding::for_bom(buf).map(|(encoding, _bom_size)| (encoding, true))
        })
        .unwrap_or_else(|| {
            let mut encoding_detector = chardetng::EncodingDetector::new();
            encoding_detector.feed(buf, is_empty);
            (encoding_detector.guess(None, true), false)
        });
    let decoder = encoding.new_decoder();

    Ok((encoding, has_bom, decoder, read))
}

pub(crate) fn read_to_string<R: std::io::Read + ?Sized>(
    reader: &mut R,
    encoding: Option<&'static encoding_rs::Encoding>,
) -> Result<(String, &'static encoding_rs::Encoding, bool)> {
    let mut buf = [0u8; 0x2000];

    let (encoding, has_bom, mut decoder, read) =
        read_and_detect_encoding(reader, encoding, &mut buf)?;

    let mut slice = &buf[..read];
    let mut is_empty = read == 0;
    let mut buf_string = String::with_capacity(buf.len());

    loop {
        let mut total_read = 0usize;

        loop {
            let (result, read, ..) =
                decoder.decode_to_string(&slice[total_read..], &mut buf_string, is_empty);

            total_read += read;

            match result {
                encoding_rs::CoderResult::InputEmpty => {
                    debug_assert_eq!(slice.len(), total_read);
                    break;
                }
                encoding_rs::CoderResult::OutputFull => {
                    debug_assert!(slice.len() > total_read);
                    buf_string.reserve(buf.len())
                }
            }
        }

        if is_empty {
            debug_assert_eq!(reader.read(&mut buf)?, 0);
            break;
        }

        let read = reader.read(&mut buf)?;
        slice = &buf[..read];
        is_empty = read == 0;
    }
    Ok((buf_string, encoding, has_bom))
}
