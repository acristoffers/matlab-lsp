/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use crate::extractors::full::scan_open;
use crate::handlers::notifications::handle_notification;
use crate::handlers::requests::handle_request;
use crate::threads::db::db_get_request_id;
use crate::types::{MessagePayload, SenderThread, ThreadMessage};
use anyhow::Result;
use crossbeam_channel::{Receiver, Sender};
use log::{error, info};
use lsp_server::{Message, Response};
use lsp_types::request::{Request, Shutdown};

pub fn start(
    lsp_sender: Sender<Message>,
    dispatcher_sender: Sender<ThreadMessage>,
    dispatcher_receiver: Receiver<ThreadMessage>,
) -> Result<()> {
    let mut exit_requested = false;
    loop {
        match dispatcher_receiver.recv()?.payload {
            MessagePayload::LspMessage(msg) => match msg {
                Message::Request(req) => {
                    if exit_requested {
                        let resp = Response::new_err(
                            req.id.clone(),
                            lsp_server::ErrorCode::InvalidRequest as i32,
                            "Shutdown already requested.".to_owned(),
                        );
                        lsp_sender.send(Message::Response(resp))?;
                    } else if req.method == Shutdown::METHOD {
                        info!("Shutdown request received.");
                        let resp = Response::new_ok(req.id, ());
                        let _ = lsp_sender.send(resp.into());
                        exit_requested = true;
                    } else if let Err(err) = handle_request(
                        lsp_sender.clone(),
                        dispatcher_sender.clone(),
                        dispatcher_receiver.clone(),
                        req,
                    ) {
                        error!("Error handling notification: {err}");
                    }
                }
                Message::Response(_) => {}
                Message::Notification(notification) => {
                    if let Err(err) = handle_notification(
                        lsp_sender.clone(),
                        dispatcher_sender.clone(),
                        dispatcher_receiver.clone(),
                        notification,
                    ) {
                        error!("Error handling notification: {err}");
                    }
                }
            },
            MessagePayload::Exit => break,
            MessagePayload::ScanOpen => {
                if let Some(id) = db_get_request_id(
                    &dispatcher_sender,
                    &dispatcher_receiver,
                    SenderThread::Handler,
                ) {
                    scan_open(
                        lsp_sender.clone(),
                        dispatcher_sender.clone(),
                        dispatcher_receiver.clone(),
                        id,
                    )?;
                }
            }
            _ => {}
        }
        dispatcher_sender.send(ThreadMessage {
            sender: SenderThread::Handler,
            payload: MessagePayload::Done,
        })?;
    }
    info!("Handler exited");
    Ok(())
}
