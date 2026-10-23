//! The persisted form of a [`PluginState`]: `PHST`, a little-endian `u32` format version, the
//! little-endian `u64` length of the rest, so that a truncated file is refused, then sections of a
//! 4-byte tag, a little-endian `u64` length and that many bytes. Version 1 has an
//! identity section (`IDEN`: a format byte, then the class ID, name, vendor and version, each a
//! `u64` length and UTF-8 text), and either the VST3 component (`COMP`) with the controller
//! (`CONT`) when there is one, or the single blob of the other formats (`BLOB`). The structure
//! follows a VST3 `.vstpreset`, whose container cannot hold the identity text or other formats.
//!
//! The format is plughost's own and stays readable: a later version reads every earlier one.

use super::{MAX_STATE_BYTES, PluginState};
use crate::InputError;
use crate::plugin::{PluginFormat, PluginId};

const MAGIC: &[u8; 4] = b"PHST";
const VERSION: u32 = 1;
const IDENTITY: &[u8; 4] = b"IDEN";
const COMPONENT: &[u8; 4] = b"COMP";
const CONTROLLER: &[u8; 4] = b"CONT";
const BLOB: &[u8; 4] = b"BLOB";
const HEADER: usize = 16;

/// The format byte of the identity section. These values never change meaning.
fn format_tag(format: PluginFormat) -> u8 {
    match format {
        PluginFormat::Vst3 => 1,
        PluginFormat::AudioUnit => 2,
        PluginFormat::Clap => 3,
    }
}

fn tag_format(tag: u8) -> Option<PluginFormat> {
    match tag {
        1 => Some(PluginFormat::Vst3),
        2 => Some(PluginFormat::AudioUnit),
        3 => Some(PluginFormat::Clap),
        _ => None,
    }
}

pub(super) fn encode(state: &PluginState) -> Vec<u8> {
    let mut identity = vec![format_tag(state.id.format())];
    for text in [
        state.id.class_id().as_str(),
        &state.name,
        &state.vendor,
        &state.version,
    ] {
        put(&mut identity, text.as_bytes());
    }
    let mut bytes = Vec::with_capacity(
        64 + identity.len() + state.component.len() + state.controller.as_ref().map_or(0, Vec::len),
    );
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&VERSION.to_le_bytes());
    bytes.extend_from_slice(&[0; 8]);
    section(&mut bytes, IDENTITY, &identity);
    if state.id.format() == PluginFormat::Vst3 {
        section(&mut bytes, COMPONENT, &state.component);
        if let Some(controller) = &state.controller {
            section(&mut bytes, CONTROLLER, controller);
        }
    } else {
        section(&mut bytes, BLOB, &state.component);
    }
    let rest = (bytes.len() - HEADER) as u64;
    bytes[8..HEADER].copy_from_slice(&rest.to_le_bytes());
    bytes
}

fn put(bytes: &mut Vec<u8>, data: &[u8]) {
    bytes.extend_from_slice(&(data.len() as u64).to_le_bytes());
    bytes.extend_from_slice(data);
}

fn section(bytes: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]) {
    bytes.extend_from_slice(tag);
    put(bytes, data);
}

/// Reads borrowed fields from the front of the input.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], InputError> {
        if count > self.0.len() {
            return Err(InputError::StateBytes);
        }
        let (taken, rest) = self.0.split_at(count);
        self.0 = rest;
        Ok(taken)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], InputError> {
        let mut array = [0; N];
        array.copy_from_slice(self.take(N)?);
        Ok(array)
    }

    /// A length-prefixed field, refused over `MAX_STATE_BYTES` before it is read.
    fn field(&mut self) -> Result<&'a [u8], InputError> {
        let length = u64::from_le_bytes(self.array()?);
        if length > MAX_STATE_BYTES as u64 {
            return Err(InputError::StateSize);
        }
        self.take(length as usize)
    }

    fn text(&mut self) -> Result<String, InputError> {
        let bytes = self.field()?;
        String::from_utf8(bytes.to_vec()).map_err(|_| InputError::StateBytes)
    }
}

pub(super) fn decode(bytes: &[u8]) -> Result<PluginState, InputError> {
    let mut reader = Reader(bytes);
    if reader.array::<4>()? != *MAGIC
        || u32::from_le_bytes(reader.array()?) != VERSION
        || u64::from_le_bytes(reader.array()?) != reader.0.len() as u64
    {
        return Err(InputError::StateBytes);
    }
    let (mut identity, mut component, mut controller, mut blob) = (None, None, None, None);
    let mut payload = 0usize;
    while !reader.0.is_empty() {
        let tag = reader.array::<4>()?;
        let data = reader.field()?;
        let slot = match &tag {
            IDENTITY => &mut identity,
            COMPONENT => &mut component,
            CONTROLLER => &mut controller,
            BLOB => &mut blob,
            _ => return Err(InputError::StateBytes),
        };
        if slot.replace(data).is_some() {
            return Err(InputError::StateBytes);
        }
        if tag != *IDENTITY {
            payload += data.len();
            if payload > MAX_STATE_BYTES {
                return Err(InputError::StateSize);
            }
        }
    }
    let mut identity = Reader(identity.ok_or(InputError::StateBytes)?);
    let format = tag_format(identity.array::<1>()?[0]).ok_or(InputError::StateBytes)?;
    let class_id = identity.text()?;
    let (name, vendor, version) = (identity.text()?, identity.text()?, identity.text()?);
    if !identity.0.is_empty() {
        return Err(InputError::StateBytes);
    }
    let (component, controller) = match (format, component, controller, blob) {
        (PluginFormat::Vst3, Some(component), controller, None) => (component, controller),
        (format, None, None, Some(blob)) if format != PluginFormat::Vst3 => (blob, None),
        _ => return Err(InputError::StateBytes),
    };
    Ok(PluginState {
        id: PluginId::new(format, class_id),
        name,
        vendor,
        version,
        component: component.to_vec(),
        controller: controller.map(<[u8]>::to_vec),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::{PluginInfo, PluginKind};

    fn state(format: PluginFormat, controller: Option<Vec<u8>>) -> PluginState {
        let mut info = PluginInfo::new(
            PluginId::new(format, "84e8de5f92554f5396fae4133c935a18"),
            "Gain",
            PluginKind::Effect,
        );
        info.vendor = "Steinberg Media Technologies".into();
        info.version = "3.7.0".into();
        PluginState::new(&info, vec![1, 2, 3], controller)
    }

    /// The version 1 bytes of a VST3 state with an empty controller state. Once released, these
    /// must always read the same.
    const VERSION_1: &[u8] = &[
        b'P', b'H', b'S', b'T', 1, 0, 0, 0, // magic, version
        116, 0, 0, 0, 0, 0, 0, 0, // the length of the sections
        b'I', b'D', b'E', b'N', 77, 0, 0, 0, 0, 0, 0, 0, 1, // identity, VST3
        32, 0, 0, 0, 0, 0, 0, 0, b'8', b'4', b'E', b'8', b'D', b'E', b'5', b'F', b'9', b'2', b'5',
        b'5', b'4', b'F', b'5', b'3', b'9', b'6', b'F', b'A', b'E', b'4', b'1', b'3', b'3', b'C',
        b'9', b'3', b'5', b'A', b'1', b'8', //
        4, 0, 0, 0, 0, 0, 0, 0, b'G', b'a', b'i', b'n', //
        3, 0, 0, 0, 0, 0, 0, 0, b'S', b'M', b'T', //
        5, 0, 0, 0, 0, 0, 0, 0, b'3', b'.', b'7', b'.', b'0', //
        b'C', b'O', b'M', b'P', 3, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3, //
        b'C', b'O', b'N', b'T', 0, 0, 0, 0, 0, 0, 0, 0,
    ];

    #[test]
    fn released_bytes_keep_reading_the_same() {
        let state = PluginState::from_bytes(VERSION_1).unwrap();
        assert_eq!(
            state.id(),
            &PluginId::new(PluginFormat::Vst3, "84E8DE5F92554F5396FAE4133C935A18")
        );
        assert_eq!(
            (state.name(), state.vendor(), state.version()),
            ("Gain", "SMT", "3.7.0")
        );
        assert_eq!(state.component(), [1, 2, 3]);
        assert_eq!(state.controller(), Some(&[][..]));
        assert_eq!(state.to_bytes(), VERSION_1);
    }

    /// The version 1 bytes of a CLAP state, whose payload is one blob.
    const VERSION_1_BLOB: &[u8] = &[
        b'P', b'H', b'S', b'T', 1, 0, 0, 0, // magic, version
        73, 0, 0, 0, 0, 0, 0, 0, // the length of the sections
        b'I', b'D', b'E', b'N', 45, 0, 0, 0, 0, 0, 0, 0, 3, // identity, CLAP
        8, 0, 0, 0, 0, 0, 0, 0, b'c', b'o', b'm', b'.', b'g', b'a', b'i', b'n', //
        4, 0, 0, 0, 0, 0, 0, 0, b'G', b'a', b'i', b'n', //
        0, 0, 0, 0, 0, 0, 0, 0, // no vendor
        0, 0, 0, 0, 0, 0, 0, 0, // no version
        b'B', b'L', b'O', b'B', 4, 0, 0, 0, 0, 0, 0, 0, 9, 8, 7, 6,
    ];

    #[test]
    fn released_blob_bytes_keep_reading_the_same() {
        let state = PluginState::from_bytes(VERSION_1_BLOB).unwrap();
        assert_eq!(state.id(), &PluginId::new(PluginFormat::Clap, "com.gain"));
        assert_eq!(
            (state.name(), state.vendor(), state.version()),
            ("Gain", "", "")
        );
        assert_eq!(state.component(), [9, 8, 7, 6]);
        assert_eq!(state.controller(), None);
        assert_eq!(state.to_bytes(), VERSION_1_BLOB);
    }

    #[test]
    fn states_read_back_unchanged() {
        for state in [
            state(PluginFormat::Vst3, Some(vec![4, 5])),
            // A controller without state of its own is no section, not an empty one.
            state(PluginFormat::Vst3, None),
            state(PluginFormat::AudioUnit, None),
            state(PluginFormat::Clap, None),
        ] {
            let bytes = state.to_bytes();
            let read = PluginState::from_bytes(&bytes).unwrap();
            assert_eq!(read, state);
            assert_eq!(read.to_bytes(), bytes);
        }
    }

    #[test]
    fn every_truncation_and_malformed_container_is_refused() {
        let bytes = state(PluginFormat::Vst3, Some(vec![4, 5])).to_bytes();
        for length in 0..bytes.len() {
            assert!(
                PluginState::from_bytes(&bytes[..length]).is_err(),
                "{length}"
            );
        }
        let with = |edit: &dyn Fn(&mut Vec<u8>)| {
            let mut edited = bytes.clone();
            edit(&mut edited);
            PluginState::from_bytes(&edited)
        };
        // A section appended with the length of the rest updated.
        let appended = |tag: &[u8; 4]| {
            with(&|b: &mut Vec<u8>| {
                b.extend_from_slice(tag);
                b.extend_from_slice(&[0; 8]);
                let rest = (b.len() - HEADER) as u64;
                b[8..HEADER].copy_from_slice(&rest.to_le_bytes());
            })
        };
        let refused = Err(InputError::StateBytes);
        assert_eq!(with(&|b| b[0] = b'X'), refused);
        assert_eq!(with(&|b| b[4] = 2), refused);
        assert_eq!(with(&|b| b.push(0)), refused);
        // An unknown section, a repeated one, and a blob in a VST3 state.
        assert_eq!(appended(b"XTRA"), refused);
        assert_eq!(appended(b"CONT"), refused);
        assert_eq!(appended(b"BLOB"), refused);
        // An unknown format byte, and text that is not UTF-8.
        assert_eq!(with(&|b| b[HEADER + 12] = 9), refused);
        assert_eq!(with(&|b| b[HEADER + 21] = 0xFF), refused);
    }

    #[test]
    fn lengths_over_the_limit_are_refused_before_reading() {
        let header = |bytes: &mut Vec<u8>| {
            let rest = (bytes.len() - HEADER) as u64;
            bytes[8..HEADER].copy_from_slice(&rest.to_le_bytes());
        };
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(&VERSION.to_le_bytes());
        bytes.extend_from_slice(&[0; 8]);
        bytes.extend_from_slice(COMPONENT);
        bytes.extend_from_slice(&(MAX_STATE_BYTES as u64 + 1).to_le_bytes());
        header(&mut bytes);
        assert_eq!(PluginState::from_bytes(&bytes), Err(InputError::StateSize));
        bytes.truncate(HEADER + 4);
        bytes.extend_from_slice(&u64::MAX.to_le_bytes());
        header(&mut bytes);
        assert_eq!(PluginState::from_bytes(&bytes), Err(InputError::StateSize));
    }
}
