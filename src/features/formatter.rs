/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use anyhow::Result;
use log::debug;
use matlab_beautifier::beautify;
use matlab_beautifier::Arguments;

pub fn format(code: &str) -> Result<String> {
    let mut arguments = Arguments {
        files: vec![],
        sparse_math: false,
        sparse_add: true,
        inplace: true,
    };
    debug!("Calling beautifier code.");
    beautify(code, &mut arguments)
}
