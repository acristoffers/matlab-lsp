/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::analysis::defref;
use crate::parsed_file::{FunctionSignature, ParsedFile};
use crate::session_state::SessionState;
use crate::types::Range;

use anyhow::{anyhow, Context, Result};
use itertools::Itertools;
use log::debug;
use lsp_server::Message;
use lsp_types::notification::{Notification, Progress};
use lsp_types::{
    ProgressParams, ProgressParamsValue, WorkDoneProgress, WorkDoneProgressBegin,
    WorkDoneProgressEnd, WorkDoneProgressReport,
};
use tree_sitter::Node;

pub type SessionStateArc = Arc<Mutex<&'static mut SessionState>>;

////////////////////////////////////////////////////////////////////////////////
///                                                                          ///
///                          Better Error Handling                           ///
///                                                                          ///
////////////////////////////////////////////////////////////////////////////////

pub trait TraversingError<T> {
    fn err_at_loc(self, node: &Node) -> Result<T>;
}

impl<T> TraversingError<T> for Option<T> {
    fn err_at_loc(self, node: &Node) -> Result<T> {
        self.ok_or_else(|| {
            anyhow!(
                "Error accessing token around line {} col {}",
                node.range().start_point.row,
                node.range().start_point.column
            )
        })
    }
}

#[macro_export]
macro_rules! code_loc {
    () => {
        anyhow!(format!("{}:{}", file!(), line!()))
    };
    ($msg:expr) => {
        anyhow!(format!("{}:{} - {}", file!(), line!(), $msg))
    };
}

/// Locks a mutex, and adds an error message in case of error.
pub(crate) fn lock_mutex<T>(arc: &Arc<Mutex<T>>) -> Result<MutexGuard<'_, T>> {
    arc.lock().map_err(|_| code_loc!("Could not lock mutex."))
}

pub fn send_progress_begin<S: AsRef<str>, T: AsRef<str>>(
    state: &mut MutexGuard<'_, &mut SessionState>,
    id: i32,
    title: S,
    message: T,
) -> Result<()> {
    let wd_begin = WorkDoneProgress::Begin(WorkDoneProgressBegin {
        title: title.as_ref().into(),
        cancellable: Some(false),
        message: Some(message.as_ref().into()),
        percentage: Some(0),
    });
    send_notification(state, id, wd_begin)
}

pub fn send_progress_report<T: AsRef<str>>(
    state: &mut MutexGuard<'_, &mut SessionState>,
    id: i32,
    message: T,
    percentage: u32,
) -> Result<()> {
    let wd_begin = WorkDoneProgress::Report(WorkDoneProgressReport {
        cancellable: Some(false),
        message: Some(message.as_ref().into()),
        percentage: Some(percentage),
    });
    send_notification(state, id, wd_begin)
}

pub fn send_progress_end<T: AsRef<str>>(
    state: &mut MutexGuard<'_, &mut SessionState>,
    id: i32,
    message: T,
) -> Result<()> {
    let wd_begin = WorkDoneProgress::End(WorkDoneProgressEnd {
        message: Some(message.as_ref().into()),
    });
    send_notification(state, id, wd_begin)
}

pub fn send_notification(
    state: &mut MutexGuard<'_, &mut SessionState>,
    id: i32,
    progress: WorkDoneProgress,
) -> Result<()> {
    state
        .sender
        .send(Message::Notification(lsp_server::Notification {
            method: Progress::METHOD.to_string(),
            params: serde_json::to_value(ProgressParams {
                token: lsp_types::NumberOrString::Number(id),
                value: ProgressParamsValue::WorkDone(progress),
            })?,
        }))
        .context(code_loc!())
}

pub fn rescan_file(
    state: &mut MutexGuard<'_, &mut SessionState>,
    file: Arc<Mutex<ParsedFile>>,
) -> Result<()> {
    debug!("Rescaning file.");
    let mut file_lock = lock_mutex(&file)?;
    if !file_lock.open {
        file_lock.load_contents()?;
    }
    remove_references_to_file(state, &mut file_lock, Arc::clone(&file))?;
    file_lock.parse()?;
    let ns_path = if let Some(ns) = &file_lock.in_namespace {
        let ns_lock = lock_mutex(ns)?;
        ns_lock.path.clone()
    } else {
        "".into()
    };
    ParsedFile::define_type(state, Arc::clone(&file), &mut file_lock, ns_path)?;
    drop(file_lock);
    defref::analyze(state, Arc::clone(&file))?;
    let mut file_lock = lock_mutex(&file)?;
    if !file_lock.open {
        file_lock.dump_contents();
    }
    Ok(())
}

fn remove_references_to_file(
    state: &mut MutexGuard<'_, &mut SessionState>,
    file: &mut MutexGuard<'_, ParsedFile>,
    parsed_file: Arc<Mutex<ParsedFile>>,
) -> Result<()> {
    'out: for v1 in file.workspace.functions.values() {
        for (name, v2) in &state.workspace.functions.clone() {
            if Arc::ptr_eq(v1, v2) {
                state.workspace.functions.remove(name);
                break 'out;
            }
        }
    }
    'out: for v1 in file.workspace.classes.values() {
        for (name, v2) in &state.workspace.classes.clone() {
            if Arc::ptr_eq(v1, v2) {
                state.workspace.functions.remove(name);
                break 'out;
            }
        }
    }
    for (name, v1) in &state.workspace.scripts.clone() {
        if Arc::ptr_eq(v1, &parsed_file) {
            state.workspace.functions.remove(name);
            break;
        }
    }
    Ok(())
}

pub fn function_signature(
    parsed_file: &MutexGuard<'_, ParsedFile>,
    node: Node,
) -> Result<FunctionSignature> {
    debug!("Scanning signature.");
    debug!("File size: {}", parsed_file.contents.len());
    let (name, name_range) = if let Some(name) = node.child_by_field_name("name") {
        let name_range = name.range();
        let name = name.utf8_text(parsed_file.contents.as_bytes())?.to_string();
        debug!("Found name.");
        (name, name_range)
    } else {
        debug!("Could not find name.");
        return Err(anyhow!("Could not find function name"));
    };
    let mut sig_range: Range = node.range().into();
    sig_range.end = name_range.end_point;
    let mut cursor = node.walk();
    let mut argout: usize = 0;
    let mut vargout = false;
    let mut argout_names = vec![];
    if let Some(output) = node
        .named_children(&mut cursor)
        .find(|c| c.kind() == "function_output")
    {
        debug!("Function has output.");
        if let Some(args) = output.child(0) {
            if args.kind() == "identifier" {
                debug!("A single one.");
                argout = 1;
                argout_names.push(args.utf8_text(parsed_file.contents.as_bytes())?.into());
            } else {
                debug!("Multiple outputs.");
                argout = args.named_child_count();
                let mut cursor2 = args.walk();
                for arg_name in args
                    .named_children(&mut cursor2)
                    .filter(|c| c.kind() == "identifier")
                    .filter_map(|c| c.utf8_text(parsed_file.contents.as_bytes()).ok())
                    .map(String::from)
                {
                    if arg_name == "varargout" {
                        vargout = true;
                    } else {
                        argout_names.push(arg_name);
                    }
                }
                if vargout {
                    argout -= 1;
                }
            }
        }
    }
    let mut argin: usize = 0;
    let mut vargin = false;
    let mut argin_names = vec![];
    let mut vargin_names = vec![];
    if let Some(inputs) = node
        .named_children(&mut cursor)
        .find(|c| c.kind() == "function_arguments")
    {
        sig_range.end = inputs.end_position();
        argin = inputs.named_child_count();
        let mut cursor2 = node.walk();
        let mut cursor3 = node.walk();
        let mut cursor4 = node.walk();
        for arg_name in inputs
            .named_children(&mut cursor2)
            .filter_map(|c| c.utf8_text(parsed_file.contents.as_bytes()).ok())
            .map(String::from)
        {
            argin_names.push(arg_name);
        }
        let mut optional_arguments = HashMap::new();
        for argument in node
            .named_children(&mut cursor2)
            .filter(|c| c.kind() == "arguments_statement")
        {
            if let Some(attributes) = argument
                .named_children(&mut cursor3)
                .find(|c| c.kind() == "attributes")
            {
                if attributes
                    .named_children(&mut cursor4)
                    .filter_map(|c| c.utf8_text(parsed_file.contents.as_bytes()).ok())
                    .any(|c| c == "Output")
                {
                    continue;
                }
            }
            for property in argument
                .named_children(&mut cursor3)
                .filter_map(|c| c.child_by_field_name("name"))
                .filter(|c| c.kind() == "property_name")
            {
                let arg_name = property
                    .named_child(0)
                    .ok_or(anyhow!(code_loc!()))?
                    .utf8_text(parsed_file.contents.as_bytes())?
                    .to_string();
                argin_names.retain(|e| *e != arg_name);
                optional_arguments.insert(arg_name, ());
                let opt_arg_name = property
                    .named_child(1)
                    .ok_or(anyhow!(code_loc!()))?
                    .utf8_text(parsed_file.contents.as_bytes())?
                    .to_string();
                vargin_names.push(opt_arg_name);
            }
        }
        let vargin_count = optional_arguments.keys().count();
        vargin = vargin_count > 0;
        argin -= vargin_count;
    }
    let doc: String = node
        .named_children(&mut cursor)
        .skip_while(|n| n.kind() != "comment")
        .take(1)
        .flat_map(|n| n.utf8_text(parsed_file.contents.as_bytes()))
        .flat_map(|s| s.split('\n'))
        .map(|s| s.trim().to_string())
        .map(|s| s.strip_prefix('%').unwrap_or(s.as_str()).to_string())
        .join("\n");
    let function = FunctionSignature {
        name_range: name_range.into(),
        name,
        argin,
        argout,
        vargin,
        vargout,
        argout_names,
        argin_names,
        vargin_names,
        range: sig_range,
        documentation: doc,
    };
    Ok(function)
}

////////////////////////////////////////////////////////////////////////////////
///                                                                          ///
///           The functions below were taken from the helix editor           ///
///                                                                          ///
////////////////////////////////////////////////////////////////////////////////

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
