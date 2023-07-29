/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use anyhow::{anyhow, Context, Result};
use crossbeam_channel::{Receiver, Sender};
use lsp_server::{Message, RequestId};
use lsp_types::notification::{Notification, Progress};
use lsp_types::request::{Request, SemanticTokensRefresh};
use lsp_types::{
    ProgressParams, ProgressParamsValue, WorkDoneProgress, WorkDoneProgressBegin,
    WorkDoneProgressEnd, WorkDoneProgressReport,
};
use tree_sitter::Node;

use crate::threads::db::db_get_request_id;
use crate::types::{SenderThread, ThreadMessage};

pub fn request_semantic_tokens_refresh(
    lsp_sender: &Sender<Message>,
    sender: &Sender<ThreadMessage>,
    receiver: &Receiver<ThreadMessage>,
    thread: SenderThread,
) -> Result<()> {
    if let Some(request_id) = db_get_request_id(sender, receiver, thread) {
        lsp_sender.send(Message::Request(lsp_server::Request {
            id: RequestId::from(request_id),
            method: SemanticTokensRefresh::METHOD.to_string(),
            params: serde_json::to_value(())?,
        }))?;
    }
    Ok(())
}

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

//////////////////////////////////////////////////////////////////////////////
//                                                                          //
//                          Progress Notification                           //
//                                                                          //
//////////////////////////////////////////////////////////////////////////////

pub fn send_progress_begin<S: AsRef<str>, T: AsRef<str>>(
    lsp_sender: Sender<Message>,
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
    send_notification(lsp_sender, id, wd_begin)
}

pub fn send_progress_report<T: AsRef<str>>(
    lsp_sender: Sender<Message>,
    id: i32,
    message: T,
    percentage: u32,
) -> Result<()> {
    let wd_begin = WorkDoneProgress::Report(WorkDoneProgressReport {
        cancellable: Some(false),
        message: Some(message.as_ref().into()),
        percentage: Some(percentage),
    });
    send_notification(lsp_sender, id, wd_begin)
}

pub fn send_progress_end<T: AsRef<str>>(
    lsp_sender: Sender<Message>,
    id: i32,
    message: T,
) -> Result<()> {
    let wd_begin = WorkDoneProgress::End(WorkDoneProgressEnd {
        message: Some(message.as_ref().into()),
    });
    send_notification(lsp_sender, id, wd_begin)
}

pub fn send_notification(
    lsp_sender: Sender<Message>,
    id: i32,
    progress: WorkDoneProgress,
) -> Result<()> {
    lsp_sender
        .send(Message::Notification(lsp_server::Notification {
            method: Progress::METHOD.to_string(),
            params: serde_json::to_value(ProgressParams {
                token: lsp_types::NumberOrString::Number(id),
                value: ProgressParamsValue::WorkDone(progress),
            })?,
        }))
        .context(code_loc!())
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
