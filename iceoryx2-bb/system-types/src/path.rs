// Copyright (c) 2023 Contributors to the Eclipse Foundation
//
// See the NOTICE file(s) distributed with this work for additional
// information regarding copyright ownership.
//
// This program and the accompanying materials are made available under the
// terms of the Apache Software License 2.0 which is available at
// https://www.apache.org/licenses/LICENSE-2.0, or the MIT license
// which is available at https://opensource.org/licenses/MIT.
//
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Relocatable (inter-process shared memory compatible) [`SemanticString`] implementation for
//! [`Path`]. All modification operations ensure that never an
//! invalid file or path name can be generated. All strings have a fixed size so that the maximum
//! path or file name length the system supports can be stored.
//!
//! # Example
//!
//! ```
//! use iceoryx2_bb_container::semantic_string::SemanticString;
//! use iceoryx2_bb_system_types::path::*;
//!
//! let name = Path::new(b"some/path/../bla/some_file.txt");
//!
//! let invalid_name = Path::new(b"/contains/illegal/\0/zero");
//! assert!(invalid_name.is_err());
//! ```

pub use iceoryx2_bb_container::semantic_string::SemanticString;

use core::hash::{Hash, Hasher};
use iceoryx2_bb_container::byte_string::FixedSizeByteString;
use iceoryx2_bb_container::semantic_string;

use iceoryx2_bb_log::fail;
use iceoryx2_pal_configuration::{FILENAME_LENGTH, PATH_SEPARATOR, ROOT};

use crate::file_path::FilePath;
use iceoryx2_bb_container::semantic_string::SemanticStringError;

const PATH_LENGTH: usize = iceoryx2_pal_configuration::PATH_LENGTH;

semantic_string! {
  name: Path,
  capacity: PATH_LENGTH,
  invalid_content: |_: &[u8]| {
    false
  },
  invalid_characters: |string: &[u8]| {
    for value in string {
        match value {
            // linux & windows
            0 => return true,
            // windows only
            1..=31 => return true,
            b'<' => return true,
            b'>' => return true,
            b'"' => return true,
            b'|' => return true,
            b'?' => return true,
            b'*' => return true,
            _ => (),
        }
    }

    false
  },
  normalize: |this: &Path| {
      let mut raw_path = [0u8; PATH_LENGTH];

      let mut previous_char_is_path_separator = false;
      let mut n = 0;
      for i in 0..this.value.len() {
          if i + 1 == this.value.len() && this.value[i] == PATH_SEPARATOR {
              break;
          }

          if !(previous_char_is_path_separator && this.value[i] == PATH_SEPARATOR) {
              raw_path[n] = this.value[i];
              n += 1;
          }

          previous_char_is_path_separator = this.value[i] == PATH_SEPARATOR
      }

      Path::new(&raw_path[0..n]).expect("A normalized path from a path shall be always valid.")
  }
}

impl Path {
    /// Adds a new file or directory entry to the path. It adds it in a fashion that a slash is
    /// added when the path does not end with a slash - except when it is empty.
    pub fn add_path_entry(
        &mut self,
        entry: &FixedSizeByteString<FILENAME_LENGTH>,
    ) -> Result<(), SemanticStringError> {
        let msg = format!("Unable to add entry \"{}\" to path since it would exceed the maximum supported path length of {} or the entry contains invalid symbols.",
            entry, PATH_LENGTH);
        if !self.is_empty()
            && self.as_bytes()[self.len() - 1] != iceoryx2_pal_configuration::PATH_SEPARATOR
        {
            fail!(from self, when self.push(iceoryx2_pal_configuration::PATH_SEPARATOR),
                "{}", msg);
        }

        fail!(from self, when self.push_bytes(entry.as_bytes()),
            "{}", msg);

        Ok(())
    }

    pub fn is_absolute(&self) -> bool {
        #[cfg(not(target_os = "windows"))]
        {
            if self.as_bytes().is_empty() {
                return false;
            }

            self.as_bytes()[0] == PATH_SEPARATOR
        }
        #[cfg(target_os = "windows")]
        {
            if self.len() < 3 {
                return false;
            }

            let has_drive_letter = (b'a' <= self.as_bytes()[0] && self.as_bytes()[0] <= b'z')
                || (b'A' <= self.as_bytes()[0] && self.as_bytes()[0] <= b'Z');

            has_drive_letter && self.as_bytes()[1] == b':' && (self.as_bytes()[2] == PATH_SEPARATOR)
        }
    }

    pub fn new_root_path() -> Path {
        Path::new(ROOT).expect("the root path is always valid")
    }

    pub fn new_empty() -> Path {
        Path::new(b"").expect("the empty path is always valid")
    }

    pub fn new_normalized(value: &[u8]) -> Result<Path, SemanticStringError> {
        Ok(Path::new(value)?.normalize())
    }

    pub fn entries(&self) -> Vec<FixedSizeByteString<FILENAME_LENGTH>> {
        let skip_size = if cfg!(target_os = "windows") && self.is_absolute() {
            // skip drive letter like C:\ since the path is absolute
            1
        } else {
            0
        };

        self.as_bytes()
            .split(|c| *c == PATH_SEPARATOR)
            .skip(skip_size)
            .filter(|entry| !entry.is_empty())
            .map(|entry| unsafe { FixedSizeByteString::new_unchecked(entry) })
            .collect()
    }
}

impl From<FilePath> for Path {
    fn from(value: FilePath) -> Self {
        unsafe { Path::new_unchecked(value.as_bytes()) }
    }
}
