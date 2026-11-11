//! macOS specific helpers.

use std::ffi::c_void;
use std::os::raw::{c_char, c_long};

type CFStringRef = *const c_void;
type CFIndex = c_long;

const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
const K_CF_STRING_NORMALIZATION_FORM_D: CFIndex = 0;

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFStringCreateWithBytes(
        alloc: *const c_void,
        bytes: *const u8,
        num_bytes: CFIndex,
        encoding: u32,
        is_external_representation: u8,
    ) -> CFStringRef;
    fn CFStringCreateMutableCopy(
        alloc: *const c_void,
        max_length: CFIndex,
        string: CFStringRef,
    ) -> CFStringRef;
    fn CFStringNormalize(string: CFStringRef, form: CFIndex);
    fn CFStringGetLength(string: CFStringRef) -> CFIndex;
    fn CFStringGetMaximumSizeForEncoding(length: CFIndex, encoding: u32) -> CFIndex;
    fn CFStringGetCString(
        string: CFStringRef,
        buffer: *mut c_char,
        buffer_size: CFIndex,
        encoding: u32,
    ) -> u8;
    fn CFRelease(cf: *const c_void);
}

/// Unicode NFD (canonical decomposition) of an UTF-8 file name, `None` for non UTF-8 names.
///
/// macOS presents file names decomposed, while archives usually contain them composed, so names
/// have to be compared in a normalized form.
pub fn normalize_nfd(name: &[u8]) -> Option<Vec<u8>> {
    unsafe {
        let string = CFStringCreateWithBytes(
            std::ptr::null(),
            name.as_ptr(),
            name.len() as CFIndex,
            K_CF_STRING_ENCODING_UTF8,
            0,
        );
        if string.is_null() {
            return None;
        }
        let normalized = CFStringCreateMutableCopy(std::ptr::null(), 0, string);
        CFRelease(string);
        if normalized.is_null() {
            return None;
        }
        CFStringNormalize(normalized, K_CF_STRING_NORMALIZATION_FORM_D);

        let size = CFStringGetMaximumSizeForEncoding(
            CFStringGetLength(normalized),
            K_CF_STRING_ENCODING_UTF8,
        ) + 1;
        let mut buffer = vec![0u8; size as usize];
        let ok = CFStringGetCString(
            normalized,
            buffer.as_mut_ptr() as *mut c_char,
            size,
            K_CF_STRING_ENCODING_UTF8,
        );
        CFRelease(normalized);
        if ok == 0 {
            return None;
        }

        let len = buffer.iter().position(|&b| b == 0).unwrap_or(buffer.len());
        buffer.truncate(len);
        Some(buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_nfd;

    #[test]
    fn test_normalize_nfd() {
        let composed = "\u{fc}ml\u{e4}ut".as_bytes();
        let decomposed = "u\u{308}mla\u{308}ut".as_bytes();
        assert_eq!(normalize_nfd(composed).unwrap(), decomposed);
        assert_eq!(normalize_nfd(decomposed).unwrap(), decomposed);
        assert_eq!(normalize_nfd(b"ascii").unwrap(), b"ascii");
        assert_eq!(normalize_nfd(b"\xff"), None);
    }
}
