//! Core Graphics cursor functions a plugin calls move the helper's own mouse and hide the
//! plugin's own cursor, not the machine's cursor that every session shares. Each image that is neither the helper's executable nor one
//! of the system's shared cache, which leaves the plugins and the libraries they bring, has its
//! symbol pointers to those functions pointed at replacements as it loads, plugins opened later
//! included. The pointers are found through the image's indirect symbol table. Interposing with
//! `DYLD_INSERT_LIBRARIES` is not open to the helper: the Hardened Runtime drops the variable.
//!
//! A plugin that looks the functions up by name (`dlsym`) still reaches the real ones.

use std::ffi::{CStr, c_char, c_void};
use std::sync::Once;

use objc2_core_foundation::CGPoint;

use super::mouse;
use crate::editor;

const LC_SEGMENT_64: u32 = 0x19;
const LC_SYMTAB: u32 = 0x2;
const LC_DYSYMTAB: u32 = 0xb;
const MH_EXECUTE: u32 = 0x2;
const MH_DYLIB_IN_CACHE: u32 = 0x8000_0000;
const SECTION_TYPE: u32 = 0xff;
const S_NON_LAZY_SYMBOL_POINTERS: u32 = 0x6;
const S_LAZY_SYMBOL_POINTERS: u32 = 0x7;
const INDIRECT_SYMBOL_LOCAL: u32 = 0x8000_0000;
const INDIRECT_SYMBOL_ABS: u32 = 0x4000_0000;

#[repr(C)]
struct MachHeader64 {
    magic: u32,
    cputype: i32,
    cpusubtype: i32,
    filetype: u32,
    ncmds: u32,
    sizeofcmds: u32,
    flags: u32,
    reserved: u32,
}

#[repr(C)]
struct LoadCommand {
    cmd: u32,
    cmdsize: u32,
}

#[repr(C)]
struct SegmentCommand64 {
    cmd: u32,
    cmdsize: u32,
    segname: [c_char; 16],
    vmaddr: u64,
    vmsize: u64,
    fileoff: u64,
    filesize: u64,
    maxprot: i32,
    initprot: i32,
    nsects: u32,
    flags: u32,
}

#[repr(C)]
struct Section64 {
    sectname: [c_char; 16],
    segname: [c_char; 16],
    addr: u64,
    size: u64,
    offset: u32,
    align: u32,
    reloff: u32,
    nreloc: u32,
    flags: u32,
    /// For symbol pointer sections, the first entry's index in the indirect symbol table.
    reserved1: u32,
    reserved2: u32,
    reserved3: u32,
}

#[repr(C)]
struct SymtabCommand {
    cmd: u32,
    cmdsize: u32,
    symoff: u32,
    nsyms: u32,
    stroff: u32,
    strsize: u32,
}

#[repr(C)]
struct DysymtabCommand {
    cmd: u32,
    cmdsize: u32,
    ilocalsym: u32,
    nlocalsym: u32,
    iextdefsym: u32,
    nextdefsym: u32,
    iundefsym: u32,
    nundefsym: u32,
    tocoff: u32,
    ntoc: u32,
    modtaboff: u32,
    nmodtab: u32,
    extrefsymoff: u32,
    nextrefsyms: u32,
    indirectsymoff: u32,
    nindirectsyms: u32,
    extreloff: u32,
    nextrel: u32,
    locreloff: u32,
    nlocrel: u32,
}

#[repr(C)]
struct Nlist64 {
    n_strx: u32,
    n_type: u8,
    n_sect: u8,
    n_desc: u16,
    n_value: u64,
}

unsafe extern "C" {
    fn _dyld_register_func_for_add_image(added: extern "C" fn(*const MachHeader64, isize));
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGDisplayBounds(display: u32) -> objc2_core_foundation::CGRect;
}

/// `CGWarpMouseCursorPosition`: a point in global display coordinates, down from the main
/// display's top.
extern "C" fn warp(point: CGPoint) -> i32 {
    mouse::move_to(objc2_foundation::NSPoint::new(
        point.x,
        editor::main_height() - point.y,
    ));
    0
}

/// `CGAssociateMouseAndMouseCursorPosition`: the helper's mouse moves only with the session's
/// input, whether the plugin couples it to the cursor or not.
extern "C" fn associate(_connected: i32) -> i32 {
    0
}

/// `CGDisplayMoveCursorToPoint`: a point from the display's top-left corner.
extern "C" fn move_to_point(display: u32, point: CGPoint) -> i32 {
    // SAFETY: Core Graphics answers for any display identifier.
    let origin = unsafe { CGDisplayBounds(display) }.origin;
    warp(CGPoint::new(origin.x + point.x, origin.y + point.y))
}

/// `CGDisplayHideCursor`: hides the plugin's cursor, not the machine's.
extern "C" fn hide_cursor(_display: u32) -> i32 {
    super::cursor::hide();
    0
}

/// `CGDisplayShowCursor`.
extern "C" fn show_cursor(_display: u32) -> i32 {
    super::cursor::show();
    0
}

/// `LSOpenCFURLRef`: tells the session the plugin asked to open the URL, which opens nothing on
/// the machine.
extern "C" fn open_url(
    url: *const objc2_core_foundation::CFURL,
    launched: *mut *const c_void,
) -> i32 {
    // SAFETY: the URL, if given, is one.
    if let Some(url) = unsafe { url.as_ref() } {
        super::isolation::open(url.string().to_string());
    }
    if !launched.is_null() {
        // SAFETY: the caller gave a place for the URL that opened, which none did.
        unsafe { *launched = std::ptr::null() };
    }
    0
}

/// The functions replaced, by their symbol names without the leading underscore.
fn replacement(name: &[u8]) -> Option<*mut c_void> {
    let replacement: *mut c_void = match name {
        b"CGWarpMouseCursorPosition" => warp as *mut c_void,
        b"CGAssociateMouseAndMouseCursorPosition" => associate as *mut c_void,
        b"CGDisplayMoveCursorToPoint" => move_to_point as *mut c_void,
        b"CGDisplayHideCursor" => hide_cursor as *mut c_void,
        b"CGDisplayShowCursor" => show_cursor as *mut c_void,
        b"LSOpenCFURLRef" => open_url as *mut c_void,
        _ => return None,
    };
    Some(replacement)
}

/// Rebinds the images loaded now and each one loaded later, for the rest of the process's life.
pub fn install() {
    static INSTALLED: Once = Once::new();
    // SAFETY: the callback reads only the images dyld hands it.
    INSTALLED.call_once(|| unsafe { _dyld_register_func_for_add_image(added) });
}

extern "C" fn added(header: *const MachHeader64, slide: isize) {
    // SAFETY: dyld hands a loaded image's header, and the slide it was loaded at.
    unsafe { rebind_image(header, slide) };
}

/// The load commands of `header`.
unsafe fn commands(header: *const MachHeader64) -> impl Iterator<Item = *const LoadCommand> {
    // SAFETY: the commands follow the header, each `cmdsize` bytes long.
    let count = unsafe { (*header).ncmds };
    let mut command = unsafe { header.add(1) }.cast::<LoadCommand>();
    (0..count).map(move |_| {
        let current = command;
        command = unsafe { command.byte_add((*current).cmdsize as usize) };
        current
    })
}

unsafe fn rebind_image(header: *const MachHeader64, slide: isize) {
    // SAFETY: the header is a loaded image's; its commands and the tables they locate are mapped.
    unsafe {
        if (*header).filetype == MH_EXECUTE || (*header).flags & MH_DYLIB_IN_CACHE != 0 {
            return;
        }
        let (mut linkedit, mut symtab, mut dysymtab) = (None, None, None);
        for command in commands(header) {
            match (*command).cmd {
                LC_SEGMENT_64 => {
                    let segment = command.cast::<SegmentCommand64>();
                    if name(&(*segment).segname) == b"__LINKEDIT" {
                        linkedit = Some(segment);
                    }
                }
                LC_SYMTAB => symtab = Some(command.cast::<SymtabCommand>()),
                LC_DYSYMTAB => dysymtab = Some(command.cast::<DysymtabCommand>()),
                _ => {}
            }
        }
        let (Some(linkedit), Some(symtab), Some(dysymtab)) = (linkedit, symtab, dysymtab) else {
            return;
        };
        if (*dysymtab).nindirectsyms == 0 {
            return;
        }
        let base = slide as usize + (*linkedit).vmaddr as usize - (*linkedit).fileoff as usize;
        let tables = Tables {
            symbols: (base + (*symtab).symoff as usize) as *const Nlist64,
            strings: (base + (*symtab).stroff as usize) as *const c_char,
            indirect: (base + (*dysymtab).indirectsymoff as usize) as *const u32,
        };
        for command in commands(header) {
            if (*command).cmd != LC_SEGMENT_64 {
                continue;
            }
            let segment = command.cast::<SegmentCommand64>();
            let read_only = match name(&(*segment).segname) {
                b"__DATA" => false,
                b"__DATA_CONST" => true,
                _ => continue,
            };
            let first = segment.add(1).cast::<Section64>();
            for index in 0..(*segment).nsects as usize {
                let section = first.add(index);
                let kind = (*section).flags & SECTION_TYPE;
                if kind == S_LAZY_SYMBOL_POINTERS || kind == S_NON_LAZY_SYMBOL_POINTERS {
                    rebind_section(&*section, slide, &tables, read_only);
                }
            }
        }
    }
}

struct Tables {
    symbols: *const Nlist64,
    strings: *const c_char,
    indirect: *const u32,
}

/// A segment or section name, up to its first NUL.
fn name(raw: &[c_char; 16]) -> &[u8] {
    // SAFETY: `c_char` and `u8` have the same layout.
    let bytes: &[u8; 16] = unsafe { &*std::ptr::from_ref(raw).cast() };
    let end = bytes.iter().position(|&byte| byte == 0).unwrap_or(16);
    &bytes[..end]
}

unsafe fn rebind_section(section: &Section64, slide: isize, tables: &Tables, read_only: bool) {
    let pointers = (slide as usize + section.addr as usize) as *mut *mut c_void;
    let count = section.size as usize / size_of::<*mut c_void>();
    let mut writable = false;
    for index in 0..count {
        // SAFETY: the section's entries index the indirect symbol table from `reserved1`, and
        // each symbol names a string of the string table.
        let replacement = unsafe {
            let symbol = *tables.indirect.add(section.reserved1 as usize + index);
            if symbol & (INDIRECT_SYMBOL_LOCAL | INDIRECT_SYMBOL_ABS) != 0 {
                continue;
            }
            let name = CStr::from_ptr(
                tables
                    .strings
                    .add((*tables.symbols.add(symbol as usize)).n_strx as usize),
            );
            let Some(name) = name.to_bytes().strip_prefix(b"_") else {
                continue;
            };
            replacement(name)
        };
        let Some(replacement) = replacement else {
            continue;
        };
        if read_only && !writable {
            // `__DATA_CONST` is read-only once dyld bound it.
            if !protect(pointers.cast(), section.size as usize, true) {
                return;
            }
            writable = true;
        }
        // SAFETY: the entry is within the section, writable now.
        unsafe { *pointers.add(index) = replacement };
    }
    if writable {
        protect(pointers.cast(), section.size as usize, false);
    }
}

/// Makes the pages holding `size` bytes from `start` writable, or read-only again.
fn protect(start: *mut c_void, size: usize, writable: bool) -> bool {
    // SAFETY: `sysconf` reads a constant; `mprotect` changes only the image's own pages.
    unsafe {
        let page = libc::sysconf(libc::_SC_PAGESIZE) as usize;
        let first = start as usize & !(page - 1);
        let length = start as usize + size - first;
        let protection = if writable {
            libc::PROT_READ | libc::PROT_WRITE
        } else {
            libc::PROT_READ
        };
        libc::mprotect(first as *mut c_void, length, protection) == 0
    }
}
