/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::collections::HashMap;

use lsp_types::InitializeParams;

use crate::parsed_code::ParsedCode;

pub struct GlobalState {
    pub files: HashMap<String, ParsedCode>,
    pub workspace: InitializeParams,
}
